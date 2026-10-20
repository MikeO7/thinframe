//! Run the browser and keep it alive. This is the OpenRC service's `command`.
//!
//! cog on DRM (`cog --platform=drm --platform-params=renderer=gles,rotation=N URL`, or `cage -- cog
//! --platform=wl URL`), restarted 3 s after it exits, 30 s after 5 fast failures. Commands from
//! the agent thread arrive over an mpsc channel; the night-mode schedule is evaluated here once
//! a minute so it keeps working through a server outage. SIGTERM is forwarded to cog.
use crate::config::Config;
use crate::update::{self, Paths, StartCheck};
use crate::{agent, screen, splash, sys};
use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

/// Current screen state, read by the agent for heartbeats.
pub static SCREEN_ON: AtomicBool = AtomicBool::new(true);

#[derive(Debug, Clone, PartialEq)]
pub enum KioskCmd {
    Restart,
    Reload,
    SetUrl(String),
    SetRotation(u8),
    Screen(bool),
    Brightness(u8),
    Schedule(String, String),
    /// Stop cog and return from `supervise`: a new tfctl was installed, init respawns us.
    Exit,
}

/// Kill what is left of cog's process group (WPEWebProcess, WPENetworkProcess) and wait up to
/// 5 s for it to empty. A web process that outlives cog keeps a dup of cog's DRM file, which
/// holds DRM master; the next cog would open the device without master and fail every mode set
/// with EACCES (seen under QEMU, 2026-10-20).
fn reap_group(pgid: u32) {
    let _ = sys::send_group(pgid, sys::SIGKILL);
    let deadline = Instant::now() + Duration::from_secs(5);
    while sys::send_group(pgid, 0).is_ok() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
    }
}

/// The dashboard's dark background (server page.css `--bg`).
const BG_COLOR: &str = "#0b0d10";

const RESTART_DELAY: Duration = Duration::from_secs(3);
const BACKOFF_DELAY: Duration = Duration::from_secs(30);
const FAST_FAIL: Duration = Duration::from_secs(60);
const FAST_FAIL_LIMIT: u32 = 5;
/// No cog binary: hold the splash instead of looping on spawn errors.
const MISSING_DELAY: Duration = Duration::from_secs(3600);

pub fn run(cfg: &Config) -> io::Result<()> {
    sys::trap_term();
    // TASK 11 rollback: two fast crashes after an update put tfctl.prev back; exit so the
    // respawn runs it.
    if let StartCheck::RolledBack(v) = update::record_start(&Paths::system()) {
        eprintln!("tfctl: kiosk: software {v} rolled back, exiting for respawn");
        return Ok(());
    }
    let (tx, rx) = mpsc::channel();
    let mut k = Kiosk::new(cfg);
    if !cfg.server_url.is_empty() {
        match agent::Agent::new(cfg, tx.clone(), &cfg.path) {
            Ok(mut a) => {
                // Written before cog starts, so cog opens on "joining…"/"reaching…" (TASK 15).
                if !cfg.fallback_url.is_empty() {
                    a.start_on_status_page();
                    k.url = cfg.fallback_url.clone();
                }
                thread::Builder::new().name("agent".into()).spawn(move || {
                    if let Err(e) = agent::serve(a) {
                        eprintln!("tfctl: agent: {e}");
                    }
                })?;
            }
            Err(e) => eprintln!("tfctl: agent: {e}"),
        }
    }
    k.supervise(&rx)
}

pub struct Kiosk {
    url: String,
    platform: String,
    renderer: String,
    rotation: u8,
    extra_args: Vec<String>,
    user: String,
    brightness: u8,
    screen_off: String,
    screen_on: String,
    child: Option<Child>,
    stopped: bool,
    fast_fails: u32,
    last_spawn: Instant,
    next_spawn: Instant,
    sched_state: Option<bool>,
    last_sched: Instant,
    splash_bg: u32,
    splash_text: String,
    splash_style: String,
    /// Held from start until cog is spawned (TASK 15); shown again when cog is missing.
    splash: Option<splash::Splash>,
    exit: bool,
}

impl Kiosk {
    pub fn new(cfg: &Config) -> Kiosk {
        Kiosk {
            url: cfg.url.clone(),
            platform: cfg.platform.clone(),
            renderer: cfg.renderer.clone(),
            rotation: cfg.rotation,
            extra_args: cfg.extra_args.clone(),
            user: cfg.user.clone(),
            brightness: cfg.brightness,
            screen_off: cfg.screen_off.clone(),
            screen_on: cfg.screen_on.clone(),
            child: None,
            stopped: false,
            fast_fails: 0,
            last_spawn: Instant::now(),
            next_spawn: Instant::now(),
            sched_state: None,
            last_sched: Instant::now() - Duration::from_secs(60),
            splash_bg: cfg.splash_bg,
            splash_text: cfg.splash_text.clone(),
            splash_style: cfg.splash_style.clone(),
            splash: None,
            exit: false,
        }
    }

    fn show_splash(&mut self, status: &str) {
        let look = splash::Look {
            style: &self.splash_style,
            bg: self.splash_bg,
            title: &self.splash_text,
            status,
            rotation: self.rotation,
            ip: splash::lan_ip(),
        };
        match splash::show(&look) {
            Ok(s) => self.splash = Some(s),
            Err(e) => eprintln!("tfctl: splash: {e}"),
        }
    }

    pub fn command(&self) -> Command {
        let mut c = if self.platform == "wl" {
            let mut c = Command::new("cage");
            c.arg("--").arg("cog").arg("--platform=wl");
            c
        } else {
            let mut c = Command::new("cog");
            c.arg("--platform=drm")
                // Only the GLES renderer rotates; modeset ignores rotation (cog 0.18.4). COG_RENDERER
                // picks: gles is the default, modeset is the escape hatch if gles fails on a GPU.
                .arg(format!(
                    "--platform-params=renderer={},rotation={}",
                    self.renderer, self.rotation
                ));
            c
        };
        // Dark until the page paints: the default is a white flash at every (re)start.
        c.arg(format!("--bg-color={BG_COLOR}"));
        c.args(&self.extra_args).arg(&self.url);
        let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
            .unwrap_or_else(|_| format!("/run/user/{}", uid_of(&self.user).unwrap_or(1000)));
        c.env("XDG_RUNTIME_DIR", runtime_dir)
            .env("LIBSEAT_BACKEND", "seatd")
            .stdin(Stdio::null());
        // Own process group: WebKit's helper processes join it, so a restart can kill them all.
        c.process_group(0);
        c
    }

    fn spawn(&mut self) {
        // The splash covers every (re)start: cog takes seconds to its first frame, and the panel
        // would be dark meanwhile. Keep the picture up but give up DRM master so cog gets it on
        // open; the loop frees the buffer once cog's frame replaced it. If master cannot be
        // dropped, release the device entirely as before.
        if self.splash.is_none() {
            self.show_splash("tuning in to the dashboard");
        }
        if !self.splash.as_mut().is_some_and(|s| s.hand_over()) {
            self.splash = None;
        }
        match self.command().spawn() {
            Ok(child) => {
                eprintln!("tfctl: kiosk: started pid {} -> {}", child.id(), self.url);
                self.child = Some(child);
                self.last_spawn = Instant::now();
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                eprintln!("tfctl: kiosk: spawn: {e}; holding splash");
                self.show_splash("cog missing");
                self.child = None;
                self.next_spawn = Instant::now() + MISSING_DELAY;
            }
            Err(e) => {
                eprintln!("tfctl: kiosk: spawn: {e}");
                self.exited();
            }
        }
    }

    /// Bookkeeping after cog ended (or failed to start): schedule the next spawn.
    fn exited(&mut self) {
        self.child = None;
        if self.last_spawn.elapsed() < FAST_FAIL {
            self.fast_fails += 1;
        } else {
            self.fast_fails = 0;
        }
        let delay = if self.fast_fails >= FAST_FAIL_LIMIT {
            BACKOFF_DELAY
        } else {
            RESTART_DELAY
        };
        self.next_spawn = Instant::now() + delay;
    }

    fn kill_child(&mut self) {
        if let Some(mut c) = self.child.take() {
            if self.stopped {
                let _ = sys::send(c.id(), sys::SIGCONT);
            }
            let _ = c.kill();
            let _ = c.wait();
            reap_group(c.id());
        }
        self.stopped = false;
    }

    fn restart(&mut self) {
        self.kill_child();
        self.fast_fails = 0;
        self.next_spawn = Instant::now();
    }

    fn set_screen(&mut self, on: bool) {
        SCREEN_ON.store(on, Ordering::SeqCst);
        if on {
            if let Err(e) = screen::set_brightness(self.brightness) {
                eprintln!("tfctl: backlight: {e}");
            }
            if self.stopped {
                if let Some(c) = &self.child {
                    let _ = sys::send(c.id(), sys::SIGCONT);
                }
                self.stopped = false;
            }
            self.restart(); // "on restores both and reloads the page"
        } else {
            if let Err(e) = screen::set_brightness(0) {
                eprintln!("tfctl: backlight: {e}");
            }
            if let Some(c) = &self.child {
                if !self.stopped && sys::send(c.id(), sys::SIGSTOP).is_ok() {
                    self.stopped = true;
                }
            }
        }
    }

    pub fn handle(&mut self, cmd: KioskCmd) {
        eprintln!("tfctl: kiosk: {cmd:?}");
        match cmd {
            KioskCmd::Restart | KioskCmd::Reload => self.restart(),
            KioskCmd::SetUrl(u) => {
                if u != self.url {
                    self.url = u;
                    self.restart();
                }
            }
            KioskCmd::SetRotation(r) => {
                if r != self.rotation {
                    self.rotation = r;
                    self.restart();
                }
            }
            KioskCmd::Screen(on) => self.set_screen(on),
            KioskCmd::Brightness(b) => {
                self.brightness = b;
                if SCREEN_ON.load(Ordering::SeqCst) {
                    if let Err(e) = screen::set_brightness(b) {
                        eprintln!("tfctl: backlight: {e}");
                    }
                }
            }
            KioskCmd::Schedule(off, on) => {
                self.screen_off = off;
                self.screen_on = on;
                self.sched_state = None;
                self.last_sched = Instant::now() - Duration::from_secs(60);
            }
            KioskCmd::Exit => {
                self.kill_child();
                self.exit = true;
            }
        }
    }

    fn tick_schedule(&mut self) {
        if self.last_sched.elapsed() < Duration::from_secs(60) {
            return;
        }
        self.last_sched = Instant::now();
        let want_off = screen::schedule_wants_off(&self.screen_off, &self.screen_on);
        if want_off.is_some() && want_off != self.sched_state {
            self.sched_state = want_off;
            let on = !want_off.unwrap_or(false);
            if on != SCREEN_ON.load(Ordering::SeqCst) {
                eprintln!("tfctl: schedule: screen {}", if on { "on" } else { "off" });
                self.set_screen(on);
            }
        }
    }

    pub fn supervise(&mut self, rx: &Receiver<KioskCmd>) -> io::Result<()> {
        let _ = screen::set_brightness(self.brightness);
        self.show_splash("warming up");
        loop {
            if sys::term_requested() || self.exit {
                eprintln!("tfctl: kiosk: stopping cog");
                self.kill_child();
                self.splash = None;
                return Ok(());
            }
            if let Some(c) = &mut self.child {
                if let Ok(Some(status)) = c.try_wait() {
                    eprintln!("tfctl: kiosk: cog exited: {status}");
                    reap_group(c.id());
                    self.exited();
                }
            }
            if self.splash.as_ref().is_some_and(|s| s.replaced()) {
                eprintln!("tfctl: kiosk: cog is on screen, splash released");
                self.splash = None;
            }
            if self.child.is_none()
                && !self.stopped
                && SCREEN_ON.load(Ordering::SeqCst)
                && Instant::now() >= self.next_spawn
            {
                self.spawn();
            }
            self.tick_schedule();
            match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(cmd) => self.handle(cmd),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => thread::sleep(Duration::from_secs(1)),
            }
        }
    }
}

fn uid_of(user: &str) -> Option<u32> {
    std::fs::read_to_string("/etc/passwd")
        .ok()?
        .lines()
        .filter_map(|l| {
            let mut f = l.split(':');
            Some((f.next()?, f.nth(1)?))
        })
        .find(|(name, _)| *name == user)
        .and_then(|(_, uid)| uid.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_cog_command_lines() {
        let mut cfg = Config {
            url: "http://s/frame/k".into(),
            rotation: 1,
            extra_args: vec!["--x".into()],
            ..Config::default()
        };
        let k = Kiosk::new(&cfg);
        let c = k.command();
        let args: Vec<_> = c.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(c.get_program(), "cog");
        assert_eq!(
            args,
            [
                "--platform=drm",
                "--platform-params=renderer=gles,rotation=1",
                "--bg-color=#0b0d10",
                "--x",
                "http://s/frame/k"
            ]
        );
        assert!(c.get_envs().any(|(k, _)| k == "LIBSEAT_BACKEND"));
        cfg.platform = "wl".into();
        let c = Kiosk::new(&cfg).command();
        assert_eq!(c.get_program(), "cage");
        assert_eq!(c.get_args().next().unwrap(), "--");
        cfg.platform = "drm".into();
        cfg.renderer = "modeset".into();
        let c = Kiosk::new(&cfg).command();
        assert_eq!(
            c.get_args().nth(1).unwrap(),
            "--platform-params=renderer=modeset,rotation=1"
        );
    }

    #[test]
    fn reap_group_kills_helpers_that_outlive_the_child() {
        // Like cog: the child exits (killed) while a grandchild in its group keeps running.
        let mut c = Command::new("sh")
            .args(["-c", "sleep 30 & wait"])
            .process_group(0)
            .spawn()
            .unwrap();
        let pgid = c.id();
        thread::sleep(Duration::from_millis(200));
        c.kill().unwrap();
        c.wait().unwrap();
        assert!(
            sys::send_group(pgid, 0).is_ok(),
            "sleep should still be alive"
        );
        reap_group(pgid);
        assert!(sys::send_group(pgid, 0).is_err(), "group should be empty");
    }

    #[test]
    fn fast_failures_back_off() {
        let mut k = Kiosk::new(&Config::default());
        for _ in 0..4 {
            k.exited();
        }
        assert!(k.next_spawn - Instant::now() <= RESTART_DELAY);
        k.exited();
        assert!(k.next_spawn - Instant::now() > RESTART_DELAY);
        k.restart();
        assert_eq!(k.fast_fails, 0);
        assert!(k.next_spawn <= Instant::now());
    }

    #[test]
    fn set_url_only_restarts_on_change() {
        let mut k = Kiosk::new(&Config::default());
        k.next_spawn = Instant::now() + Duration::from_secs(100);
        k.handle(KioskCmd::SetUrl(k.url.clone()));
        assert!(k.next_spawn > Instant::now());
        k.handle(KioskCmd::SetUrl("http://other/".into()));
        assert!(k.next_spawn <= Instant::now());
        k.handle(KioskCmd::Schedule("23:00".into(), "06:00".into()));
        assert_eq!(k.screen_off, "23:00");
    }
}
