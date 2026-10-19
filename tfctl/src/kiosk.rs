//! Run the browser and keep it alive. This is the OpenRC service's `command`.
//!
//! cog on DRM (`cog --platform=drm --platform-params=rotation=N URL`, or `cage -- cog
//! --platform=wl URL`), restarted 3 s after it exits, 30 s after 5 fast failures. Commands from
//! the agent thread arrive over an mpsc channel; the night-mode schedule is evaluated here once
//! a minute so it keeps working through a server outage. SIGTERM is forwarded to cog.
use crate::config::Config;
use crate::{agent, screen, sys};
use std::io;
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
}

const RESTART_DELAY: Duration = Duration::from_secs(3);
const BACKOFF_DELAY: Duration = Duration::from_secs(30);
const FAST_FAIL: Duration = Duration::from_secs(60);
const FAST_FAIL_LIMIT: u32 = 5;

pub fn run(cfg: &Config) -> io::Result<()> {
    sys::trap_term();
    let (tx, rx) = mpsc::channel();
    if !cfg.server_url.is_empty() {
        let (c, tx) = (cfg.clone(), tx.clone());
        thread::Builder::new().name("agent".into()).spawn(move || {
            if let Err(e) = agent::run(&c, tx) {
                eprintln!("tfctl: agent: {e}");
            }
        })?;
    }
    Kiosk::new(cfg).supervise(&rx)
}

pub struct Kiosk {
    url: String,
    platform: String,
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
}

impl Kiosk {
    pub fn new(cfg: &Config) -> Kiosk {
        Kiosk {
            url: cfg.url.clone(),
            platform: cfg.platform.clone(),
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
                .arg(format!("--platform-params=rotation={}", self.rotation));
            c
        };
        c.args(&self.extra_args).arg(&self.url);
        let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
            .unwrap_or_else(|_| format!("/run/user/{}", uid_of(&self.user).unwrap_or(1000)));
        c.env("XDG_RUNTIME_DIR", runtime_dir)
            .env("LIBSEAT_BACKEND", "seatd")
            .stdin(Stdio::null());
        c
    }

    fn spawn(&mut self) {
        match self.command().spawn() {
            Ok(child) => {
                eprintln!("tfctl: kiosk: started pid {} -> {}", child.id(), self.url);
                self.child = Some(child);
                self.last_spawn = Instant::now();
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
        loop {
            if sys::term_requested() {
                eprintln!("tfctl: kiosk: SIGTERM, stopping cog");
                self.kill_child();
                return Ok(());
            }
            if let Some(c) = &mut self.child {
                if let Ok(Some(status)) = c.try_wait() {
                    eprintln!("tfctl: kiosk: cog exited: {status}");
                    self.exited();
                }
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
                "--platform-params=rotation=1",
                "--x",
                "http://s/frame/k"
            ]
        );
        assert!(c.get_envs().any(|(k, _)| k == "LIBSEAT_BACKEND"));
        cfg.platform = "wl".into();
        let c = Kiosk::new(&cfg).command();
        assert_eq!(c.get_program(), "cage");
        assert_eq!(c.get_args().next().unwrap(), "--");
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
