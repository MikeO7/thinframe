//! Control plane (TASK 10, docs/decisions.md "Two planes"): one outbound WebSocket to
//! ws://{SERVER_URL}/api/frames/{FRAME_ID}/ws. Out: hello, heartbeat (60 s), ack. In: config
//! (apply, persist, signal the kiosk), action (do, ack), hello reply (set clock + TZ).
//! Down: reconnect with backoff 1..60 s, poll GET /config over HTTP meanwhile; after 3
//! consecutive failures the kiosk shows FALLBACK_URL until the server answers again.
//! Runs as a thread inside `tfctl kiosk`; `tfctl agent` runs it alone for debugging.
//! Without Wi-Fi it shows the setup page first (wifisetup.rs); without FRAME_TOKEN it pairs
//! next (pair.rs).
use crate::config::Config;
use crate::http::{self, Url};
use crate::json::{self, Value};
use crate::kiosk::{KioskCmd, SCREEN_ON};
use crate::update::{self, Artifacts, Paths};
use crate::{status, sys, sysupdate, ws};
use std::io;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HEARTBEAT: Duration = Duration::from_secs(60);
const PING: Duration = Duration::from_secs(30);
const DEAD: Duration = Duration::from_secs(90);
const FALLBACK_AFTER: u32 = 3;
const STABLE_SESSION: Duration = Duration::from_secs(30);
/// A slot U-Boot still counts tries for that has not reached the server this long after the
/// agent started is rebooted, so U-Boot spends a try (and falls back) without a power cycle.
pub const UNCONFIRMED_REBOOT_S: u64 = 10 * 60;

/// Reboot only while uEnv names the running slot with tries left (tries=0 is a confirmed slot;
/// another slot means U-Boot already fell back), and only once both the process and the last
/// server contact (or the start, with none) are `UNCONFIRMED_REBOOT_S` old.
pub fn should_reboot_unconfirmed(
    tries: u32,
    slot_is_running: bool,
    secs_since_start: u64,
    secs_since_last_contact: u64,
) -> bool {
    tries > 0
        && slot_is_running
        && secs_since_start >= UNCONFIRMED_REBOOT_S
        && secs_since_last_contact >= UNCONFIRMED_REBOOT_S
}

/// Failures in a row between two LAN discoveries (with the backoff, about a minute at first).
const REDISCOVER_EVERY: u32 = 3;

pub fn run(cfg: &Config, kiosk: Sender<KioskCmd>) -> io::Result<()> {
    serve(Agent::new(cfg, kiosk, &cfg.path)?)
}

/// The connect / session / poll loop for an agent the caller built (the kiosk prepares it first).
pub fn serve(mut a: Agent) -> io::Result<()> {
    let mut backoff = 1u64;
    loop {
        if sys::term_requested() {
            return Ok(());
        }
        a.maybe_reboot_unconfirmed();
        // Wi-Fi first, then pairing, then the server (TASK 17).
        if a.needs_wifi() {
            thread::sleep(a.wifi_step());
            continue;
        }
        if a.needs_pairing() {
            thread::sleep(a.pair_step());
            continue;
        }
        let ws_url = a.api(&format!("/api/frames/{}/ws", a.cfg.frame_id));
        match ws::Client::connect(&ws_url, &a.cfg.token, CONNECT_TIMEOUT) {
            Ok(mut c) => {
                eprintln!("tfctl: agent: connected to {}", a.cfg.server_url);
                a.server_ok();
                a.on_connect();
                let started = Instant::now();
                match a.session(&mut c) {
                    Ok(()) => return Ok(()),
                    Err(e) => eprintln!("tfctl: agent: socket: {e}"),
                }
                c.close();
                // A session that held for a while earns a quick reconnect; a server that
                // accepts and drops us right away does not.
                if started.elapsed() >= STABLE_SESSION {
                    backoff = 1;
                }
            }
            Err(e) => {
                eprintln!("tfctl: agent: connect {}: {e}", a.cfg.server_url);
                match a.http_poll() {
                    Ok(()) if a.exit => return Ok(()),
                    Ok(()) => a.server_ok(),
                    Err(e) => {
                        eprintln!("tfctl: agent: http poll: {e}");
                        a.server_failed();
                        a.rediscover();
                    }
                }
            }
        }
        thread::sleep(Duration::from_secs(backoff));
        backoff = (backoff * 2).min(60);
    }
}

pub struct Agent {
    pub(crate) cfg: Config,
    pub(crate) kiosk: Sender<KioskCmd>,
    pub(crate) base: Url,
    pub(crate) conf_path: String,
    failures: u32,
    pub(crate) on_fallback: bool,
    /// Latest `software_version`, `system_version` and `artifacts` from the server config.
    pub(crate) software_version: String,
    pub(crate) system_version: String,
    pub(crate) artifacts: Artifacts,
    pub(crate) paths: Paths,
    /// Events queued for the socket (kind, text); the system-update thread appends too.
    pub(crate) events: Arc<Mutex<Vec<(String, String)>>>,
    /// A system update is running in its thread.
    pub(crate) busy: Arc<AtomicBool>,
    /// Leave the loop (and the process) once the current ack is out: a new tfctl is installed.
    pub(crate) exit: bool,
    /// Versions an automatic update already failed on; retried only after a new config push.
    pub(crate) tried: Vec<String>,
    /// Open pairing request (pair.rs): the nonce the PAIR button sends back.
    pub(crate) pair_nonce: Option<String>,
    /// The pairing page is on screen.
    pub(crate) pair_page_up: bool,
    /// Debug mode: one report per process, on the first server contact.
    pub(crate) reported: bool,
    /// Wi-Fi first (wifisetup.rs): the radio, the setup page's state once it ran, since when the
    /// station is down, whether setup (not just waiting) is due, and whether its page is up.
    pub(crate) radio: Arc<dyn crate::wifi::Radio>,
    pub(crate) wifi_ui: Option<crate::wifisetup::Shared>,
    pub(crate) wifi_down_since: Option<Instant>,
    pub(crate) wifi_setup: bool,
    pub(crate) wifi_page_up: bool,
    /// Process start and the last answer from the server, for `maybe_reboot_unconfirmed`.
    pub(crate) started: Instant,
    pub(crate) last_contact: Option<Instant>,
    pub(crate) reboot_requested: bool,
}

impl Agent {
    pub fn new(cfg: &Config, kiosk: Sender<KioskCmd>, conf_path: &str) -> io::Result<Agent> {
        let base = Url::parse(&cfg.server_url).ok_or_else(|| {
            io::Error::other(format!(
                "SERVER_URL '{}' is not http://host[:port]",
                cfg.server_url
            ))
        })?;
        Ok(Agent {
            cfg: cfg.clone(),
            kiosk,
            base,
            conf_path: conf_path.to_string(),
            failures: 0,
            on_fallback: false,
            software_version: String::new(),
            system_version: String::new(),
            artifacts: Artifacts::default(),
            paths: Paths::system(),
            events: Arc::new(Mutex::new(Vec::new())),
            busy: Arc::new(AtomicBool::new(false)),
            exit: false,
            tried: Vec::new(),
            pair_nonce: None,
            pair_page_up: false,
            reported: false,
            radio: Arc::new(crate::wifi::Iwctl),
            wifi_ui: None,
            wifi_down_since: None,
            wifi_setup: false,
            wifi_page_up: false,
            started: Instant::now(),
            last_contact: None,
            reboot_requested: false,
        })
    }

    pub(crate) fn event(&self, kind: &str, text: &str) {
        eprintln!("tfctl: agent: event {kind}: {text}");
        if let Ok(mut q) = self.events.lock() {
            q.push((kind.to_string(), text.to_string()));
        }
    }

    /// Right after a connect: the slot we booted is good, a pending software update has
    /// survived, and anything queued while offline (a rollback at start) gets reported.
    fn on_connect(&mut self) {
        let mut found = Vec::new();
        found.extend(sysupdate::confirm_slot(&self.paths));
        found.extend(update::confirm_software(&self.paths));
        found.extend(update::drain_events(&self.paths));
        for (k, t) in found {
            self.event(&k, &t);
        }
        if crate::dbg::on() && !self.reported {
            self.reported = true;
            self.send_report();
        }
    }

    /// A new slot that never reaches the server is never confirmed; without this it would keep
    /// running until two more power cycles. Real frame only. "Start" is the system's boot
    /// (/proc/uptime), not this process: a new slot whose tfctl crashes and is respawned by
    /// init every few minutes must still be rebooted. Any earlier contact on this boot has
    /// already confirmed the slot (tries=0), so only this process's contact time matters.
    fn maybe_reboot_unconfirmed(&mut self) {
        if !self.paths.is_real() || self.reboot_requested {
            return;
        }
        let since_start = uptime_s(&std::fs::read_to_string("/proc/uptime").unwrap_or_default())
            .unwrap_or(0)
            .max(self.started.elapsed().as_secs());
        let since_contact = self
            .last_contact
            .map_or(since_start, |t| t.elapsed().as_secs());
        if since_start < UNCONFIRMED_REBOOT_S || since_contact < UNCONFIRMED_REBOOT_S {
            return;
        }
        let Ok(uenv) = sysupdate::uenv(&self.paths) else {
            return;
        };
        let running = sysupdate::running_slot(&self.paths);
        let is_running = running.is_some() && uenv.slot == running;
        if !should_reboot_unconfirmed(uenv.tries, is_running, since_start, since_contact) {
            return;
        }
        self.reboot_requested = true;
        let text = format!(
            "slot {} not confirmed (tries={}) and no server contact for {since_contact} s; rebooting",
            running.map_or("?", |s| s.name()),
            uenv.tries
        );
        eprintln!("tfctl: agent: {text}");
        update::queue_event(&self.paths, "slot-unconfirmed-reboot", &text);
        sysupdate::reboot_soon();
    }

    fn flush_events(&self, c: &mut ws::Client) -> io::Result<()> {
        let queued: Vec<(String, String)> = match self.events.lock() {
            Ok(mut q) => q.drain(..).collect(),
            Err(_) => return Ok(()),
        };
        for (kind, text) in queued {
            let v = Value::obj()
                .set("type", "event")
                .set("kind", kind)
                .set("text", text);
            c.send_text(&v.to_string())?;
        }
        Ok(())
    }

    pub(crate) fn api(&self, path: &str) -> Url {
        self.base.with_path(path)
    }

    pub(crate) fn send(&self, cmd: KioskCmd) -> io::Result<()> {
        self.kiosk
            .send(cmd)
            .map_err(|_| io::Error::other("kiosk thread gone"))
    }

    pub(crate) fn page_url(&self) -> &str {
        if self.on_fallback {
            &self.cfg.fallback_url
        } else {
            &self.cfg.url
        }
    }

    /// Every REDISCOVER_EVERY failures in a row, ask the LAN where the server went
    /// (discover.rs) and move there when a server proves it knows our token.
    pub(crate) fn rediscover(&mut self) {
        if self.failures == 0 || !self.failures.is_multiple_of(REDISCOVER_EVERY) {
            return;
        }
        let Ok(nonce) = crate::pair::nonce() else {
            return;
        };
        let token = Some(self.cfg.token.as_str()).filter(|t| !t.is_empty());
        let who = crate::discover::Asker {
            id: &self.cfg.frame_id,
            token,
        };
        match crate::discover::find(
            &crate::discover::targets(),
            crate::discover::WAIT,
            &who,
            &nonce,
        ) {
            Ok(Some(url)) => self.move_server(&url),
            Ok(None) => eprintln!("tfctl: agent: discovery: no server answered"),
            Err(e) => eprintln!("tfctl: agent: discovery: {e}"),
        }
    }

    /// Point the agent (and the dashboard URL, unless FRAME_URL pinned it) at `url`, persisted.
    pub(crate) fn move_server(&mut self, url: &str) {
        let old = self.cfg.server_url.trim_end_matches('/').to_string();
        let Some(base) = Url::parse(url) else {
            return;
        };
        if url == old {
            return;
        }
        let id = self.cfg.frame_id.clone();
        if self.cfg.url == format!("{old}/frame/{id}") {
            self.cfg.url = format!("{url}/frame/{id}");
        }
        self.base = base;
        self.cfg.server_url = url.to_string();
        self.persist(&[("SERVER_URL", url)]);
        eprintln!("tfctl: agent: server moved: {old} -> {url} (found on the LAN)");
        self.event(
            "server",
            &format!("server found on the LAN at {url} (was {old})"),
        );
        self.failures = 0;
        if !self.on_fallback {
            let _ = self.send(KioskCmd::SetUrl(self.cfg.url.clone()));
        }
    }

    pub(crate) fn server_ok(&mut self) {
        self.failures = 0;
        self.last_contact = Some(Instant::now());
        if self.on_fallback {
            self.on_fallback = false;
            eprintln!("tfctl: agent: server answered, showing the dashboard");
            let _ = self.send(KioskCmd::SetUrl(self.cfg.url.clone()));
        }
    }

    /// Boot (TASK 15): the kiosk opens on the local status page, not on a dashboard that cannot
    /// load yet; the first answer from the server switches to the dashboard (`server_ok`).
    pub(crate) fn start_on_status_page(&mut self) {
        if !self.cfg.fallback_url.is_empty() {
            self.on_fallback = true;
            self.write_status_page();
        }
    }

    pub(crate) fn server_failed(&mut self) {
        self.failures += 1;
        if self.on_fallback {
            self.write_status_page();
        } else if self.failures >= FALLBACK_AFTER && !self.cfg.fallback_url.is_empty() {
            self.on_fallback = true;
            eprintln!(
                "tfctl: agent: {} failures, showing fallback page",
                self.failures
            );
            self.write_status_page();
            let _ = self.send(KioskCmd::SetUrl(self.cfg.fallback_url.clone()));
        }
    }

    /// Refresh the local status page with what we wait for and our IP (TASKS 14/15).
    pub(crate) fn write_status_page(&self) {
        let st = crate::status::collect();
        let ssid = crate::wifi::configured_ssid(std::path::Path::new(crate::wifi::IWD_DIR));
        let headline = crate::statuspage::headline(
            &self.cfg.server_url,
            ssid.as_deref(),
            &st.ips,
            self.failures,
            FALLBACK_AFTER,
        );
        let mut details = vec![format!("frame {}", self.cfg.frame_id)];
        let lan: Vec<&String> = st
            .ips
            .iter()
            .filter(|ip| *ip != crate::statuspage::USB_IP)
            .collect();
        if let Some(ip) = lan.first() {
            details.push(format!("IP {ip}"));
        }
        if self.failures > 0 {
            details.push("retrying every few seconds".into());
        }
        let path = std::env::var("TFCTL_STATUS_PAGE")
            .unwrap_or_else(|_| crate::statuspage::DEFAULT_PATH.into());
        let bg_path = std::path::Path::new(&path).with_file_name("backdrop.png");
        let backdrop = crate::splash::write_backdrop(&self.cfg, &bg_path.to_string_lossy())
            .unwrap_or_else(|e| {
                eprintln!("tfctl: agent: status backdrop: {e}");
                None
            });
        if let Err(e) = crate::statuspage::write(
            &path,
            &headline,
            &details,
            backdrop.as_deref(),
            &self.cfg.splash_style,
        ) {
            eprintln!("tfctl: agent: status page: {e}");
        }
    }

    fn http_poll(&mut self) -> io::Result<()> {
        let url = self.api(&format!("/api/frames/{}/config", self.cfg.frame_id));
        let r = http::get(&url, &self.cfg.token, CONNECT_TIMEOUT)?;
        if r.status == 401 {
            self.unauthorized();
        }
        if r.status != 200 {
            return Err(io::Error::other(format!("GET config: HTTP {}", r.status)));
        }
        let text = String::from_utf8_lossy(&r.body);
        let v = json::parse(&text).map_err(io::Error::other)?;
        // The server answered: this slot and binary work, the same as a WebSocket connect.
        // Without it a frame that only ever polls would stay unconfirmed, and two power cuts
        // later U-Boot would revert a healthy update.
        self.on_connect();
        self.apply_config(&v);
        self.maybe_auto_update();
        if self.exit {
            let _ = self.send(KioskCmd::Exit);
        }
        Ok(())
    }

    fn status_json(&self, kind: &str) -> Value {
        Value::obj()
            .set("type", kind)
            .set("frame_id", self.cfg.frame_id.as_str())
            .extend(status::collect().to_json(self.page_url(), SCREEN_ON.load(Ordering::SeqCst)))
    }

    pub fn hello(&self) -> Value {
        let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .ok()
            .map(|s| s.trim().to_string());
        self.status_json("hello")
            .set("boot_id", boot_id)
            .set("page_version", self.cfg.page_version.as_str())
    }

    fn session(&mut self, c: &mut ws::Client) -> io::Result<()> {
        c.set_read_timeout(Duration::from_secs(1))?;
        c.send_text(&self.hello().to_string())?;
        let (mut last_hb, mut last_ping, mut last_rx) =
            (Instant::now(), Instant::now(), Instant::now());
        loop {
            if sys::term_requested() {
                c.close();
                return Ok(());
            }
            match c.recv()? {
                Some(ws::Event::Text(t)) => {
                    last_rx = Instant::now();
                    self.on_message(c, &t)?;
                }
                Some(_) => last_rx = Instant::now(),
                None => {}
            }
            self.flush_events(c)?;
            if self.exit {
                eprintln!("tfctl: agent: new tfctl installed, exiting for respawn");
                c.close();
                let _ = self.send(KioskCmd::Exit);
                return Ok(());
            }
            if last_hb.elapsed() >= HEARTBEAT {
                let hb = self.status_json("heartbeat").to_string();
                crate::dbg::log("ws tx", &hb);
                c.send_text(&hb)?;
                last_hb = Instant::now();
            }
            if last_ping.elapsed() >= PING {
                c.ping()?;
                last_ping = Instant::now();
            }
            if last_rx.elapsed() >= DEAD {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "no traffic for 90 s",
                ));
            }
        }
    }

    fn on_message(&mut self, c: &mut ws::Client, text: &str) -> io::Result<()> {
        crate::dbg::log("ws rx", text);
        let v = match json::parse(text) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("tfctl: agent: bad json ({e}): {text}");
                return Ok(());
            }
        };
        match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "hello" | "welcome" => self.on_hello(&v),
            "config" => {
                self.apply_config(v.get("config").unwrap_or(&v));
                self.maybe_auto_update();
            }
            "action" => {
                let name = v.field_string("name").unwrap_or_default();
                let res = self.action(&name, &v);
                let mut ack = Value::obj()
                    .set("type", "ack")
                    .set("id", v.get("id").cloned().unwrap_or(Value::Null))
                    .set("ok", res.is_ok());
                if let Err(e) = &res {
                    eprintln!("tfctl: agent: action {name}: {e}");
                    ack = ack.set("error", e.to_string());
                }
                let ack = ack.to_string();
                crate::dbg::log("ws tx", &ack);
                c.send_text(&ack)?;
            }
            "ping" => c.send_text(&Value::obj().set("type", "pong").to_string())?,
            other => eprintln!("tfctl: agent: ignoring message type '{other}'"),
        }
        Ok(())
    }

    fn on_hello(&mut self, v: &Value) {
        if let Some(t) = v.get("time_unix").and_then(Value::as_i64) {
            set_clock(t);
        }
        if let Some(tz) = v.field_string("tz") {
            // TZ only, never /etc/localtime: the root is read-only, and that file is Buildroot's
            // symlink into /usr/share/zoneinfo, so a copy onto it would overwrite a zone file.
            // musl reads zone names in TZ from /usr/share/zoneinfo; `main` sets TZ from the
            // config at start, so the zone holds across reboots before the first hello.
            if valid_tz(&tz) {
                if std::env::var("TZ").ok().as_deref() != Some(tz.as_str()) {
                    std::env::set_var("TZ", &tz);
                }
                if tz != self.cfg.tz {
                    self.cfg.tz = tz.clone();
                    self.persist(&[("TZ", &tz)]);
                }
            }
        }
    }
}

/// Whole seconds since boot from the text of /proc/uptime ("123.45 678.90").
pub fn uptime_s(text: &str) -> Option<u64> {
    let first = text.split_whitespace().next()?;
    first.split('.').next()?.parse().ok()
}

/// A zone name (`Europe/Berlin`) or POSIX TZ string (`CET-1CEST,M3.5.0,M10.5.0/3`) as it goes
/// into `TZ`, the config file and the `/usr/share/zoneinfo/` path: printable ASCII from a small
/// set, no `..`, not absolute, at most 64 bytes.
pub fn valid_tz(tz: &str) -> bool {
    !tz.is_empty()
        && tz.len() <= 64
        && !tz.starts_with('/')
        && !tz.contains("..")
        && tz
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/_+-,.:<>".contains(&b))
}

/// Seconds since the epoch at 2024-01-01: a hello with an earlier time is not a clock.
const CLOCK_MIN: i64 = 1_704_067_200;

/// `date -s @N` (busybox); only when off by more than 2 s. No NTP on the frame.
fn set_clock(t: i64) {
    if t < CLOCK_MIN {
        eprintln!("tfctl: agent: ignoring time_unix {t}");
        return;
    }
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if (t - now).abs() <= 2 {
        return;
    }
    match Command::new("date").arg("-s").arg(format!("@{t}")).output() {
        Ok(o) if o.status.success() => eprintln!("tfctl: agent: clock set to {t}"),
        Ok(o) => eprintln!(
            "tfctl: agent: date -s: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => eprintln!("tfctl: agent: date -s: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: u64 = UNCONFIRMED_REBOOT_S;

    #[test]
    fn reboots_only_an_unconfirmed_running_slot_after_the_timeout() {
        assert!(should_reboot_unconfirmed(2, true, T, T));
        assert!(should_reboot_unconfirmed(1, true, T + 500, T + 1));
        // Confirmed slot: never.
        assert!(!should_reboot_unconfirmed(0, true, T * 10, T * 10));
        // uEnv names the other slot: U-Boot already fell back, leave it alone.
        assert!(!should_reboot_unconfirmed(2, false, T * 10, T * 10));
        // Too early after start, or the server answered recently.
        assert!(!should_reboot_unconfirmed(2, true, T - 1, T - 1));
        assert!(!should_reboot_unconfirmed(2, true, T * 10, T - 1));
    }

    #[test]
    fn uptime_parses_proc_uptime() {
        assert_eq!(uptime_s("123.45 678.90\n"), Some(123));
        assert_eq!(uptime_s("700 1"), Some(700));
        assert_eq!(uptime_s(""), None);
        assert_eq!(uptime_s("x.1 2"), None);
    }

    #[test]
    fn tz_values_are_zone_names_or_posix_strings() {
        for ok in [
            "UTC",
            "Europe/Berlin",
            "America/Argentina/Buenos_Aires",
            "Etc/GMT+3",
            "CET-1CEST,M3.5.0,M10.5.0/3",
            "<+07>-7",
        ] {
            assert!(valid_tz(ok), "{ok}");
        }
        for bad in [
            "",
            "/etc/passwd",
            "../../etc/passwd",
            "Europe/Berlin\n",
            "a b",
            "x;y",
            &"a".repeat(65),
        ] {
            assert!(!valid_tz(bad), "{bad:?}");
        }
    }

    #[test]
    fn test_paths_never_reboot() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let cfg = crate::config::parse("SERVER_URL=http://nas:8080\nFRAME_ID=k\n");
        let mut a = Agent::new(&cfg, tx, "/dev/null").unwrap();
        let base = update::tests::tmp_base("unconf");
        a.paths = Paths::under(&base);
        std::fs::create_dir_all(&a.paths.boot).unwrap();
        std::fs::write(&a.paths.cmdline, "root=PARTUUID=50463037-03").unwrap();
        std::fs::write(sysupdate::uenv_path(&a.paths), "slot=b\ntries=2\n").unwrap();
        // A CI box that booted less than 20 min ago cannot go back that far; the check holds anyway.
        if let Some(t) = Instant::now().checked_sub(Duration::from_secs(T * 2)) {
            a.started = t;
        }
        a.maybe_reboot_unconfirmed();
        assert!(!a.reboot_requested);
        assert!(update::drain_events(&a.paths).is_empty());
        std::fs::remove_dir_all(&base).unwrap();
    }
}
