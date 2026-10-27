//! Run the browser and keep it alive. This is the OpenRC service's `command`.
//!
//! cog on DRM (`cog --platform=drm --platform-params=renderer=gles,rotation=N URL`, or `cage -- cog
//! --platform=wl URL`), restarted 3 s after it exits, 30 s after 5 fast failures. Commands from
//! the agent thread arrive over an mpsc channel; the night-mode schedule is evaluated here once
//! a minute so it keeps working through a server outage. The light sensor turns the screen off
//! in a dark room (light.rs); a touch while off wakes it for a minute. SIGTERM is forwarded to cog.
use crate::config::Config;
use crate::update::{self, Paths, StartCheck};
use crate::{agent, light, screen, sense, splash, sys};
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::FileExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

/// Current screen state, read by the agent for heartbeats.
pub static SCREEN_ON: AtomicBool = AtomicBool::new(true);
/// The last light reading (0..63), 255 = no sensor; for heartbeats.
pub static LIGHT: AtomicU8 = AtomicU8::new(255);
/// Events for the server (kind, text), sent by the agent on its next pass (light: wave, motion).
pub static EVENTS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

fn push_event(kind: &str, text: &str) {
    if let Ok(mut q) = EVENTS.lock() {
        if q.len() < 50 {
            q.push((kind.into(), text.into()));
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum KioskCmd {
    Restart,
    Reload,
    SetUrl(String),
    SetRotation(u8),
    Screen(bool),
    /// Turn an off screen on for this many seconds, like a touch (a doorbell ring); then the
    /// schedule and the light decide again. An on screen stays as it is.
    Wake(u64),
    Brightness(u8),
    /// What the light sensor may do besides "dark = off" (sense.rs).
    Sense {
        auto_brightness: bool,
        wave: bool,
        motion_wake: bool,
    },
    Schedule(String, String),
    /// Stop cog and return from `supervise`: a new tfctl was installed, init respawns us.
    Exit,
    /// Show Settings (settings.rs) at this path; the page on screen comes back on close.
    OpenSettings(String),
    /// cog is already on Settings at this path (a form on the page went there): remember the
    /// page to come back to, without loading Settings a second time.
    SettingsShown(String),
    CloseSettings,
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
/// WebKit's own memory limit for the WebProcess (cog `--web-mem-limit`, MiB). Above a third of it
/// WebKit drops caches, above half it collects hard (cog's default thresholds). At 256 the page
/// (about 100 MB) sat above the third, so WebKit threw away decoded pictures and painted tiles
/// every 30 s ("Memory pressure relief" in the log) and painted them again on the next tab
/// switch [V 2026-10-26]. 512 puts the third at 170 MB; the frame has 1 GB and nothing else big.
/// No kill threshold: a hung page is the server watchdog's job (restart-kiosk).
const WEB_MEM_LIMIT_MIB: u32 = 512;
/// Mesa's compiled shaders, kept across cog restarts and boots (see command()).
const SHADER_CACHE: &str = "/data/thinframe/shaders";
/// cog's WebKit cache and storage (`$XDG_CACHE_HOME/cog`, `$XDG_DATA_HOME/cog`), see command().
const WEB_CACHE: &str = "/data/thinframe/web-cache";
const WEB_DATA: &str = "/data/thinframe/web-data";

/// At boot cog starts at once on the local status page, so its 2.5 s start (WebKit's processes,
/// GL) overlaps the wait for Wi-Fi and the server; the dashboard then opens in the same cog over
/// the bus (`go`). Before the bus a page change was a second cog start, so boot held cog back up
/// to 12 s for the server instead [V 2026-10-22]. A page asked for while cog is still starting
/// (not on the bus yet) is retried for this long before cog is restarted on it.
const BUS_WAIT: Duration = Duration::from_secs(8);
const RESTART_DELAY: Duration = Duration::from_secs(3);
const BACKOFF_DELAY: Duration = Duration::from_secs(30);
const FAST_FAIL: Duration = Duration::from_secs(60);
const FAST_FAIL_LIMIT: u32 = 5;
/// A touch while the screen is off turns it on for this long (decisions.md "Night mode").
const TOUCH_WAKE: Duration = Duration::from_secs(60);
/// The light value goes to the log this often, to tune light.rs's thresholds at the frame.
const LIGHT_LOG: Duration = Duration::from_secs(900);
/// A private D-Bus session bus for cog, so a new page opens in the running cog (`cogctl open`)
/// instead of a restart: ending cog took 2.2 s and the splash over the new start 3.9 s
/// [V 2026-10-26]; in place the old page stays up until the new one paints.
const BUS: &str = "unix:path=/run/thinframe/cog-bus";
const COGCTL_TIMEOUT: Duration = Duration::from_secs(3);
/// No cog binary: hold the splash instead of looping on spawn errors.
const MISSING_DELAY: Duration = Duration::from_secs(3600);

/// Where inittab sends our stderr and cog's (`>>`, O_APPEND). /var/log is the 64 MB tmpfs.
pub const LOG: &str = "/var/log/thinframe.log";
const LOG_MAX: u64 = 1 << 20;
const LOG_KEEP: u64 = 256 << 10;

/// Over `max` bytes: keep the last `keep` in place. Truncated, not replaced: the inittab shell's
/// fd and cog's stay on this inode, and with O_APPEND their next write lands at the new end.
pub fn cap_log(path: &str, max: u64, keep: u64) -> io::Result<()> {
    let mut f = match fs::OpenOptions::new().read(true).append(true).open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let len = f.metadata()?.len();
    if len <= max {
        return Ok(());
    }
    let keep = keep.min(len);
    let mut tail = vec![0u8; keep as usize];
    f.read_exact_at(&mut tail, len - keep)?;
    f.set_len(0)?;
    f.write_all(&tail)
}

pub fn run(cfg: &Config) -> io::Result<()> {
    crate::logstamp::start();
    sys::trap_term();
    crate::watchdog::start();
    // TASK 11 rollback: two fast crashes after an update put tfctl.prev back; exit so the
    // respawn runs it.
    if let StartCheck::RolledBack(v) = update::record_start(&Paths::system()) {
        eprintln!("tfctl: kiosk: software {v} rolled back, exiting for respawn");
        return Ok(());
    }
    let (tx, rx) = mpsc::channel();
    let mut k = Kiosk::new(cfg);
    let st: crate::settings::Shared = Default::default();
    match crate::settings::start(
        st.clone(),
        std::sync::Arc::new(crate::wifi::Iwctl),
        tx.clone(),
        cfg.path.clone(),
        &format!("127.0.0.1:{}", crate::settings::PORT),
    ) {
        Ok(_) => k.settings = Some(st),
        Err(e) => eprintln!("tfctl: settings: listen on {}: {e}", crate::settings::PORT),
    }
    crate::logmirror::start(LOG, crate::logmirror::DIR);
    crate::boot::save_seed_when_ready();
    crate::wifitrace::start();
    crate::gpuwatch::start(tx.clone());
    crate::clock::start();
    if cfg.server_url.is_empty() {
        // Flashed without a server: find one on the LAN, then carry on as if it was set.
        let (cfg, tx) = (cfg.clone(), tx.clone());
        thread::Builder::new()
            .name("discover".into())
            .spawn(move || {
                if let Some(cfg) = discover_server(&cfg, &tx) {
                    start_agent(&cfg, &tx, None);
                }
            })?;
    } else {
        start_agent(cfg, &tx, Some(&mut k));
    }
    k.supervise(&rx)
}

/// Probe the LAN every 10 s until a server answers or one is typed in Settings; persist it as
/// SERVER_URL and return the config reloaded with it. None only when tfctl is asked to stop.
/// Without Wi-Fi for wifisetup::GRACE it opens Settings on Wi-Fi: the agent, which shows the
/// Wi-Fi setup page otherwise, runs only once a server is known.
fn discover_server(cfg: &Config, tx: &mpsc::Sender<KioskCmd>) -> Option<Config> {
    use crate::wifi::Radio;
    let token = Some(cfg.token.as_str()).filter(|t| !t.is_empty());
    let who = crate::discover::Asker {
        id: &cfg.frame_id,
        token,
    };
    crate::settings::LISTENING.store(true, Ordering::SeqCst);
    let started = Instant::now();
    let mut wifi_shown = false;
    loop {
        if crate::sys::term_requested() {
            return None;
        }
        if let Some(url) = crate::settings::take_server() {
            eprintln!("tfctl: kiosk: no SERVER_URL; {url} set in Settings");
            return Some(crate::config::load(&cfg.path).unwrap_or_else(|_| {
                let mut c = cfg.clone();
                c.server_url = url.clone();
                c.url = format!("{url}/frame/{}", c.frame_id);
                c
            }));
        }
        let radio = crate::wifi::Iwctl;
        if !wifi_shown
            && radio.present()
            && !radio.connected()
            && started.elapsed() >= crate::wifisetup::GRACE
        {
            wifi_shown = true;
            let _ = tx.send(KioskCmd::OpenSettings("/settings/wifi".into()));
        }
        let nonce = crate::pair::nonce().unwrap_or_default();
        if let Ok(Some(url)) = crate::discover::find(
            &crate::discover::targets(),
            crate::discover::WAIT,
            &who,
            &nonce,
        ) {
            eprintln!("tfctl: kiosk: no SERVER_URL; found a server on the LAN at {url}");
            if let Err(e) = crate::config::save_overrides(&cfg.path, &[("SERVER_URL", &url)]) {
                eprintln!("tfctl: kiosk: cannot write {}: {e}", cfg.path);
            }
            let mut c = crate::config::load(&cfg.path).unwrap_or_else(|_| cfg.clone());
            if c.server_url.is_empty() {
                c.server_url = url.clone();
                c.url = format!("{url}/frame/{}", c.frame_id);
            }
            return Some(c);
        }
        crate::settings::nap(Duration::from_secs(10));
    }
}

/// Start the agent thread. With the kiosk not supervising yet (`k`), it opens on the status
/// page; once it runs, the agent's first server answer switches it to the dashboard.
fn start_agent(cfg: &Config, tx: &mpsc::Sender<KioskCmd>, k: Option<&mut Kiosk>) {
    {
        match agent::Agent::new(cfg, tx.clone(), &cfg.path) {
            Ok(mut a) => {
                // Written before cog starts, so cog opens on "joining…"/"reaching…" (TASK 15).
                if !cfg.fallback_url.is_empty() {
                    a.start_on_status_page();
                    match k {
                        Some(k) => {
                            k.url = cfg.fallback_url.clone();
                            k.next_spawn = Instant::now();
                        }
                        None => {
                            let _ = tx.send(KioskCmd::SetUrl(cfg.fallback_url.clone()));
                        }
                    }
                }
                let r = thread::Builder::new().name("agent".into()).spawn(move || {
                    if let Err(e) = agent::serve(a) {
                        eprintln!("tfctl: agent: {e}");
                    }
                });
                if let Err(e) = r {
                    eprintln!("tfctl: agent: thread: {e}");
                }
            }
            Err(e) => eprintln!("tfctl: agent: {e}"),
        }
    }
}

/// The styles whose splash redraws itself (splash::show).
fn animated(style: &str) -> bool {
    matches!(style, "dawn" | "seventies")
}

pub struct Kiosk {
    url: String,
    platform: String,
    renderer: String,
    rotation: u8,
    extra_args: Vec<String>,
    cog_env: Vec<(String, String)>,
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
    /// DARK_OFF: the light sensor may turn the screen off.
    dark_off: bool,
    /// The PF1007L sensor; None elsewhere (QEMU, sim, Mac) or with DARK_OFF=0.
    light: Option<light::Sensor>,
    dark: light::Dark,
    last_light: Instant,
    last_light_log: Instant,
    touch: light::Touch,
    /// What the schedule and the light last asked for. They act only when this changes, so a
    /// server `screen` action holds until the next change.
    auto_off: bool,
    /// A touch woke the screen; the schedule and the light wait until then.
    wake_until: Option<Instant>,
    /// Wave, motion and auto-brightness (sense.rs), and which of them the server turned on.
    sense: sense::Sense,
    auto_brightness: bool,
    wave: bool,
    motion_wake: bool,
    /// The backlight now; auto-brightness keeps it at or below `brightness`.
    backlight: u8,
    splash_bg: u32,
    splash_text: String,
    splash_style: String,
    /// Held from start until cog is spawned (TASK 15); shown again when cog is missing.
    splash: Option<splash::Splash>,
    exit: bool,
    /// Settings (settings.rs), when its listener started; and the page to go back to while it
    /// is on screen (pages the agent asks for meanwhile replace it).
    settings: Option<crate::settings::Shared>,
    back_to: Option<String>,
    /// The session bus cog listens on (`BUS`); None where dbus-daemon is missing.
    bus: Option<Child>,
    /// A page asked for while cog was still starting; `go` tries it again each pass.
    pending: Option<String>,
}

impl Kiosk {
    pub fn new(cfg: &Config) -> Kiosk {
        Kiosk {
            url: cfg.url.clone(),
            platform: cfg.platform.clone(),
            renderer: cfg.renderer.clone(),
            rotation: cfg.rotation,
            extra_args: cfg.extra_args.clone(),
            cog_env: cfg.cog_env.clone(),
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
            dark_off: cfg.dark_off,
            light: None,
            dark: light::Dark::new(),
            last_light: Instant::now(),
            last_light_log: Instant::now() - LIGHT_LOG,
            touch: light::Touch::closed(),
            auto_off: false,
            wake_until: None,
            sense: sense::Sense::new(),
            auto_brightness: cfg.auto_brightness,
            wave: cfg.wave,
            motion_wake: cfg.motion_wake,
            backlight: cfg.brightness,
            splash_bg: cfg.splash_bg,
            splash_text: cfg.splash_text.clone(),
            splash_style: cfg.splash_style.clone(),
            splash: None,
            exit: false,
            settings: None,
            back_to: None,
            bus: None,
            pending: None,
        }
    }

    fn show_splash(&mut self, status: &str) {
        let look = splash::Look {
            style: &self.splash_style,
            bg: self.splash_bg,
            title: &self.splash_text,
            status,
            rotation: self.rotation,
            // The address changes every boot (random MAC), so a first frame with it is never in
            // the render cache and costs ~4 s before cog starts; the animation adds it within 2 s.
            ip: if animated(&self.splash_style) {
                None
            } else {
                splash::lan_ip()
            },
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
        c.arg(format!("--bg-color={BG_COLOR}"))
            .arg(format!("--web-mem-limit={WEB_MEM_LIMIT_MIB}"));
        c.args(&self.extra_args).arg(&self.url);
        let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
            .unwrap_or_else(|_| format!("/run/user/{}", uid_of(&self.user).unwrap_or(1000)));
        c.env("XDG_RUNTIME_DIR", runtime_dir)
            .env("LIBSEAT_BACKEND", "seatd")
            .env("DBUS_SESSION_BUS_ADDRESS", BUS)
            .stdin(Stdio::null());
        // Mesa keeps compiled shaders under $HOME/.cache, which is on the read-only root, so it
        // compiled them all again at every cog start. WebKit's HTTP cache and storage were in
        // memory only for the same reason (and to spare an SD card): every boot and restart
        // fetched the fonts and app files again. Both live on /data now (eMMC); the dashboard
        // itself is no-store, so the cache only holds the few files that may be cached.
        if Path::new(SHADER_CACHE).parent().is_some_and(Path::is_dir) {
            c.env("MESA_SHADER_CACHE_DIR", SHADER_CACHE)
                .env("MESA_SHADER_CACHE_MAX_SIZE", "32M")
                .env("XDG_CACHE_HOME", WEB_CACHE)
                .env("XDG_DATA_HOME", WEB_DATA);
        }
        // Paint every tile on the GPU. WPE's default "hybrid" mode paints a tile on the CPU when
        // a CPU worker is free and on the GPU otherwise, so neighbouring tiles, and one tile from
        // one redraw to the next, came from two rasterizers whose text, gradients and pictures
        // differ slightly: seams at the 512 px tile edges and content that jumped while a touch
        // redrew the page. GPU-only measured as fast as hybrid on the frame; CPU-only was 2x
        // slower (docs/decisions.md "Touch first").
        c.env("WEBKIT_SKIA_CPU_PAINTING_THREADS", "0");
        for (k, v) in &self.cog_env {
            c.env(k, v);
        }
        if crate::dbg::on() {
            c.env("G_MESSAGES_DEBUG", "all");
            let args: Vec<String> = c
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            crate::dbg::log("cog", &args.join(" "));
        }
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
        if let Err(e) = cap_log(LOG, LOG_MAX, LOG_KEEP) {
            eprintln!("tfctl: kiosk: log cap: {e}");
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
            if let Err(e) = screen::set_brightness(self.backlight) {
                eprintln!("tfctl: backlight: {e}");
            }
            // The paused page carries on where it was: its picture is back with the backlight,
            // and page.js sees the gap in its timers and reconnects its stream, which replays every
            // source. A restart here cost ~7 s of splash and reload per wake [V 2026-10-27].
            if self.stopped {
                if let Some(c) = &self.child {
                    let _ = sys::send(c.id(), sys::SIGCONT);
                }
                self.stopped = false;
            }
            if self.child.is_none() {
                self.next_spawn = Instant::now();
            }
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
            // The server's page watchdog sees the dashboard go quiet while Settings is up: the
            // person at the screen comes first, the dashboard reloads when Settings closes.
            KioskCmd::Restart | KioskCmd::Reload if self.back_to.is_some() => {
                eprintln!("tfctl: kiosk: Settings open, not restarting");
            }
            // Paused on purpose (screen off): the server's page watchdog sees the pings stop, but
            // the page is not hung, and a restart would throw away the instant wake.
            KioskCmd::Restart | KioskCmd::Reload if self.stopped => {
                eprintln!("tfctl: kiosk: screen off, page paused, not restarting");
            }
            KioskCmd::Restart | KioskCmd::Reload => self.restart(),
            KioskCmd::SetUrl(u) if self.back_to.is_some() => self.back_to = Some(u),
            KioskCmd::SetUrl(u) => self.go(u),
            KioskCmd::SetRotation(r) => {
                if r != self.rotation {
                    self.rotation = r;
                    self.restart();
                }
            }
            KioskCmd::Screen(on) => self.set_screen(on),
            KioskCmd::Wake(secs) => self.wake(Duration::from_secs(secs), "wake"),
            KioskCmd::Sense {
                auto_brightness,
                wave,
                motion_wake,
            } => {
                eprintln!("tfctl: light: auto-brightness {auto_brightness}, wave {wave}, motion wake {motion_wake}");
                (self.auto_brightness, self.wave, self.motion_wake) =
                    (auto_brightness, wave, motion_wake);
                if !auto_brightness && self.backlight != self.brightness {
                    self.backlight = self.brightness;
                    if SCREEN_ON.load(Ordering::SeqCst) {
                        let _ = screen::set_brightness(self.backlight);
                    }
                }
            }
            KioskCmd::Brightness(b) => {
                self.brightness = b;
                self.backlight = if self.auto_brightness {
                    self.backlight.min(b)
                } else {
                    b
                };
                if SCREEN_ON.load(Ordering::SeqCst) {
                    if let Err(e) = screen::set_brightness(self.backlight) {
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
            KioskCmd::OpenSettings(path) => {
                let Some(st) = &self.settings else {
                    return;
                };
                crate::settings::open(st);
                if self.back_to.is_none() {
                    self.back_to = Some(self.url.clone());
                }
                if !SCREEN_ON.load(Ordering::SeqCst) {
                    self.set_screen(true);
                }
                self.go(crate::settings::url(&path));
            }
            KioskCmd::SettingsShown(path) => {
                if self.back_to.is_none() {
                    self.back_to = Some(self.url.clone());
                }
                self.url = crate::settings::url(&path);
            }
            KioskCmd::CloseSettings => {
                if let Some(st) = &self.settings {
                    crate::settings::close(st);
                }
                if let Some(u) = self.back_to.take() {
                    self.go(u);
                }
            }
        }
    }

    /// Show `u`: in the running cog when it answers on the bus, else by a restart. A cog that
    /// started moments ago is not on the bus yet: `u` waits (`pending`) instead of a restart.
    fn go(&mut self, u: String) {
        self.pending = None;
        if u == self.url {
            return;
        }
        if self.open_in_place(&u) {
            eprintln!("tfctl: kiosk: opened {u} in place");
            self.url = u;
            return;
        }
        if self.child.is_some()
            && !self.stopped
            && self.bus.is_some()
            && self.last_spawn.elapsed() < BUS_WAIT
        {
            self.pending = Some(u);
            return;
        }
        self.url = u;
        self.restart();
    }

    fn open_in_place(&mut self, u: &str) -> bool {
        if self.child.is_none() || self.stopped || self.bus.is_none() {
            return false;
        }
        if self
            .bus
            .as_mut()
            .is_some_and(|b| !matches!(b.try_wait(), Ok(None)))
        {
            self.bus = None;
            return false;
        }
        let child = Command::new("cogctl")
            .args(["open", u])
            .env("DBUS_SESSION_BUS_ADDRESS", BUS)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut child) = child else {
            return false;
        };
        let end = Instant::now() + COGCTL_TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(st)) => return st.success(),
                Ok(None) if Instant::now() < end => thread::sleep(Duration::from_millis(20)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
            }
        }
    }

    /// cog's session bus, once: dbus-daemon in the foreground as our child.
    fn start_bus(&mut self) {
        if self.platform == "wl" {
            return;
        }
        let path = BUS.trim_start_matches("unix:path=");
        let _ = fs::remove_file(path);
        match Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--nopidfile"])
            .arg(format!("--address={BUS}"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
        {
            Ok(c) => {
                // cog must find the socket at its start, or it runs without the bus.
                let end = Instant::now() + Duration::from_secs(1);
                while !std::path::Path::new(path).exists() && Instant::now() < end {
                    thread::sleep(Duration::from_millis(10));
                }
                self.bus = Some(c);
            }
            Err(e) => eprintln!("tfctl: kiosk: no session bus ({e}); pages change by restart"),
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
            self.apply_auto("schedule");
        }
    }

    /// Off when the schedule says so or the room is dark.
    fn wants_off(&self) -> bool {
        self.sched_state == Some(true) || (self.dark_off && self.dark.is_dark())
    }

    fn apply_auto(&mut self, why: &str) {
        if self.wake_until.is_some() {
            return; // the end of the touch hold looks again
        }
        let off = self.wants_off();
        if off == self.auto_off {
            return;
        }
        self.auto_off = off;
        if off == SCREEN_ON.load(Ordering::SeqCst) {
            eprintln!("tfctl: {why}: screen {}", if off { "off" } else { "on" });
            self.set_screen(!off);
        }
    }

    /// An off screen on for `d` (a touch, a wave, a doorbell); one that a wake already turned on
    /// stays on longer. The schedule and the light decide again when it ends.
    fn wake(&mut self, d: Duration, why: &str) {
        let until = Instant::now() + d;
        if !SCREEN_ON.load(Ordering::SeqCst) {
            eprintln!("tfctl: {why}: screen on for {} s", d.as_secs());
            self.set_screen(true);
            self.wake_until = Some(until);
        } else if self.wake_until.is_some_and(|t| t < until) {
            self.wake_until = Some(until);
        }
    }

    /// Every pass (10 a second with a sensor): the sensor's fast features (sense.rs) and touch.
    /// Once a second: the dark rule and auto-brightness.
    fn tick_light(&mut self) {
        let now = Instant::now();
        let mut fresh = None;
        if let Some(s) = &mut self.light {
            let v = s.read();
            LIGHT.store(v, Ordering::Relaxed);
            fresh = Some(v);
            if let Some(g) = self.sense.step(v, now) {
                self.gesture(g, v);
            }
        }
        if self.last_light.elapsed() >= Duration::from_secs(1) {
            self.last_light = now;
            if let Some(v) = fresh {
                if self.last_light_log.elapsed() >= LIGHT_LOG {
                    self.last_light_log = now;
                    eprintln!("tfctl: light: {v}");
                }
                if let Some(dark) = self.dark.step(v, now) {
                    eprintln!(
                        "tfctl: light: room {} ({v})",
                        if dark { "dark" } else { "lit" }
                    );
                    self.apply_auto("light");
                }
                self.tick_auto_brightness(v, now);
            }
        }
        self.tick_touch(now);
    }

    fn gesture(&mut self, g: sense::Gesture, v: u8) {
        let on = SCREEN_ON.load(Ordering::SeqCst);
        match g {
            sense::Gesture::Wave if self.wave && self.back_to.is_none() => {
                if on {
                    // The page moves to the next tab (the server relays it).
                    eprintln!("tfctl: light: wave");
                    push_event("light", "wave");
                } else {
                    self.wake(TOUCH_WAKE, "light: wave");
                }
            }
            sense::Gesture::Motion if self.motion_wake => {
                eprintln!("tfctl: light: motion ({v})");
                push_event("light", &format!("motion {v}"));
                if !self.dark.is_dark() {
                    self.wake(TOUCH_WAKE, "light: motion");
                }
            }
            _ => {}
        }
    }

    /// Auto-brightness: the backlight follows the room light, a few points a second.
    fn tick_auto_brightness(&mut self, v: u8, now: Instant) {
        if !self.auto_brightness || !SCREEN_ON.load(Ordering::SeqCst) {
            return;
        }
        let light = self.sense.base(now).unwrap_or(v);
        let target = (self.brightness as u32 * sense::auto_pct(light) as u32 / 100).max(5) as u8;
        let next = sense::approach(self.backlight, target.min(self.brightness));
        if next != self.backlight {
            self.backlight = next;
            if let Err(e) = screen::set_brightness(next) {
                eprintln!("tfctl: backlight: {e}");
            }
        }
    }

    fn tick_touch(&mut self, now: Instant) {
        let touched = self.touch.touched();
        if self.touch.long_press(crate::settings::HOLD) && self.back_to.is_none() {
            eprintln!(
                "tfctl: touch: held {} s, opening Settings",
                crate::settings::HOLD.as_secs()
            );
            self.handle(KioskCmd::OpenSettings("/settings".into()));
        }
        if self.settings.as_ref().is_some_and(crate::settings::idle) {
            eprintln!("tfctl: settings: idle, closing");
            self.handle(KioskCmd::CloseSettings);
        }
        if touched {
            if !SCREEN_ON.load(Ordering::SeqCst) {
                self.wake(TOUCH_WAKE, "touch");
            } else if self.wake_until.is_some() {
                self.wake_until = Some(now + TOUCH_WAKE);
            }
        }
        if self.wake_until.is_some_and(|t| now >= t) {
            self.wake_until = None;
            self.auto_off = self.wants_off();
            if self.auto_off && SCREEN_ON.load(Ordering::SeqCst) {
                eprintln!("tfctl: touch: hold over, screen off");
                self.set_screen(false);
            }
        }
    }

    /// The sensor and the touch panel, on the frame only. The touch module loads after tfctl
    /// starts (touchfw.rs), so the touch device is looked for again until found.
    fn open_inputs(&mut self) {
        if light::on_pf1007l() {
            match light::Sensor::open() {
                Ok(s) => {
                    eprintln!(
                        "tfctl: light: sensor on LRADC0, 10 reads a second{}",
                        if self.dark_off {
                            ", screen off in a dark room"
                        } else {
                            ""
                        }
                    );
                    self.light = Some(s);
                }
                Err(e) => eprintln!("tfctl: light: {e}"),
            }
        }
    }

    pub fn supervise(&mut self, rx: &Receiver<KioskCmd>) -> io::Result<()> {
        let _ = screen::set_brightness(self.backlight);
        self.show_splash("warming up");
        // After the splash: its picture is a 4 MB read, which waited behind the prewarm's
        // 464 MB on the card [V 2026-10-27: 0.9 s].
        crate::prewarm::start();
        self.open_inputs();
        self.start_bus();
        loop {
            crate::watchdog::beat();
            if sys::term_requested() || self.exit {
                eprintln!("tfctl: kiosk: stopping cog");
                self.kill_child();
                if let Some(mut b) = self.bus.take() {
                    let _ = b.kill();
                    let _ = b.wait();
                }
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
            if let Some(u) = self.pending.take() {
                self.go(u);
            }
            if self.child.is_none()
                && !self.stopped
                && SCREEN_ON.load(Ordering::SeqCst)
                && Instant::now() >= self.next_spawn
            {
                self.spawn();
            }
            self.tick_schedule();
            self.tick_light();
            // With a light sensor the loop runs 10 times a second (a wave lasts 0.15 to 1.5 s).
            let pass = if self.light.is_some() {
                Duration::from_millis(100)
            } else {
                Duration::from_secs(1)
            };
            match rx.recv_timeout(pass) {
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
    use crate::config;

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
                "--web-mem-limit=512",
                "--x",
                "http://s/frame/k"
            ]
        );
        assert!(c.get_envs().any(|(k, _)| k == "LIBSEAT_BACKEND"));
        let env = |c: &Command, name: &str| {
            c.get_envs()
                .filter(|(k, _)| *k == name)
                .last()
                .and_then(|(_, v)| v.map(|v| v.to_str().unwrap().to_string()))
        };
        assert_eq!(
            env(&c, "WEBKIT_SKIA_CPU_PAINTING_THREADS").as_deref(),
            Some("0")
        );
        // COG_ENV comes last, so it can undo a default.
        cfg.cog_env = vec![("WEBKIT_SKIA_CPU_PAINTING_THREADS".into(), "2".into())];
        let c = Kiosk::new(&cfg).command();
        assert_eq!(
            env(&c, "WEBKIT_SKIA_CPU_PAINTING_THREADS").as_deref(),
            Some("2")
        );
        cfg.cog_env = vec![];
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
    fn wake_resumes_the_paused_page_and_ignores_restarts_while_off() {
        let mut k = Kiosk::new(&Config::default());
        k.child = Some(Command::new("sleep").arg("30").spawn().unwrap());
        let pid = k.child.as_ref().unwrap().id();
        k.set_screen(false);
        assert!(k.stopped);
        k.handle(KioskCmd::Restart);
        assert_eq!(
            k.child.as_ref().map(Child::id),
            Some(pid),
            "restart while off"
        );
        k.set_screen(true);
        assert!(!k.stopped);
        assert_eq!(
            k.child.as_ref().map(Child::id),
            Some(pid),
            "same cog after wake"
        );
        k.kill_child();
    }

    #[test]
    fn a_page_asked_for_while_cog_starts_waits_for_the_bus() {
        let mut k = Kiosk::new(&Config::default());
        k.child = Some(Command::new("sleep").arg("30").spawn().unwrap());
        k.bus = Some(Command::new("sleep").arg("30").spawn().unwrap());
        k.last_spawn = Instant::now();
        let (pid, was) = (k.child.as_ref().unwrap().id(), k.url.clone());
        // No cog on this bus answers (cogctl fails or is missing): wait, do not restart.
        k.handle(KioskCmd::SetUrl("http://server/frame/x".into()));
        assert_eq!(k.pending.as_deref(), Some("http://server/frame/x"));
        assert_eq!(k.url, was);
        assert_eq!(k.child.as_ref().map(Child::id), Some(pid));
        // Too long: restart on it.
        k.last_spawn = Instant::now() - BUS_WAIT;
        let u = k.pending.take().unwrap();
        k.go(u);
        assert!(k.pending.is_none());
        assert_eq!(k.url, "http://server/frame/x");
        assert!(k.child.is_none(), "restarted");
        if let Some(mut b) = k.bus.take() {
            let _ = b.kill();
            let _ = b.wait();
        }
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
    fn log_is_cut_to_its_tail_in_place() {
        let p = std::env::temp_dir().join(format!("tfctl-cap-{}.log", std::process::id()));
        let path = p.to_str().unwrap();
        assert!(cap_log(path, 10, 4).is_ok(), "missing file is fine");
        // The writer holds an O_APPEND fd across the cut, like inittab's `>>`.
        let mut w = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&p)
            .unwrap();
        w.write_all(b"0123456789").unwrap();
        cap_log(path, 10, 4).unwrap();
        assert_eq!(
            fs::read(&p).unwrap(),
            b"0123456789",
            "at the limit: untouched"
        );
        w.write_all(b"abcdef").unwrap();
        cap_log(path, 10, 4).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"cdef");
        w.write_all(b"!").unwrap();
        assert_eq!(
            fs::read(&p).unwrap(),
            b"cdef!",
            "writer appends at the new end"
        );
        fs::remove_file(&p).unwrap();
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
    fn dark_room_turns_the_screen_off_and_light_or_a_touch_back_on() {
        // The only test that touches SCREEN_ON.
        let mut k = Kiosk::new(&Config::default());
        let t0 = Instant::now();
        let feed = |k: &mut Kiosk, v: u8, from: u64, secs: u64| {
            for s in from..from + secs {
                if k.dark.step(v, t0 + Duration::from_secs(s)).is_some() {
                    k.apply_auto("light");
                }
            }
        };
        feed(&mut k, 0, 0, 200);
        assert!(!SCREEN_ON.load(Ordering::SeqCst), "dark: off");
        feed(&mut k, 60, 200, 10);
        assert!(SCREEN_ON.load(Ordering::SeqCst), "lit: on");
        // Schedule off: daylight does not turn it on; the schedule's end does not while dark.
        k.sched_state = Some(true);
        k.apply_auto("schedule");
        assert!(!SCREEN_ON.load(Ordering::SeqCst));
        feed(&mut k, 0, 210, 200);
        k.sched_state = Some(false);
        k.apply_auto("schedule");
        assert!(!SCREEN_ON.load(Ordering::SeqCst), "still dark");
        // A touch holds it on; when the hold ends in the dark it goes off again.
        k.set_screen(true);
        k.wake_until = Some(Instant::now());
        feed(&mut k, 60, 410, 10);
        assert!(SCREEN_ON.load(Ordering::SeqCst), "lit during the hold");
        feed(&mut k, 0, 420, 200);
        assert!(SCREEN_ON.load(Ordering::SeqCst), "the hold wins");
        k.tick_touch(Instant::now());
        assert!(
            !SCREEN_ON.load(Ordering::SeqCst),
            "hold over in the dark: off"
        );
        // DARK_OFF=0: the light is read for the log only.
        let mut k = Kiosk::new(&config::parse("DARK_OFF=0\n"));
        feed(&mut k, 0, 0, 200);
        assert!(k.dark.is_dark() && !k.wants_off());
        SCREEN_ON.store(true, Ordering::SeqCst);
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
