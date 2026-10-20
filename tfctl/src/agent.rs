//! Control plane (TASK 10, docs/decisions.md "Two planes"): one outbound WebSocket to
//! ws://{SERVER_URL}/api/frames/{FRAME_ID}/ws. Out: hello, heartbeat (60 s), ack. In: config
//! (apply, persist, signal the kiosk), action (do, ack), hello reply (set clock + TZ).
//! Down: reconnect with backoff 1..60 s, poll GET /config over HTTP meanwhile; after 3
//! consecutive failures the kiosk shows FALLBACK_URL until the server answers again.
//! Runs as a thread inside `tfctl kiosk`; `tfctl agent` runs it alone for debugging.
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

pub fn run(cfg: &Config, kiosk: Sender<KioskCmd>) -> io::Result<()> {
    let mut a = Agent::new(cfg, kiosk, &cfg.path)?;
    let mut backoff = 1u64;
    loop {
        if sys::term_requested() {
            return Ok(());
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
    base: Url,
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

    fn api(&self, path: &str) -> Url {
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

    pub(crate) fn server_ok(&mut self) {
        self.failures = 0;
        if self.on_fallback {
            self.on_fallback = false;
            eprintln!("tfctl: agent: server back, leaving fallback page");
            let _ = self.send(KioskCmd::SetUrl(self.cfg.url.clone()));
        }
    }

    pub(crate) fn server_failed(&mut self) {
        self.failures += 1;
        if self.failures >= FALLBACK_AFTER && !self.on_fallback && !self.cfg.fallback_url.is_empty()
        {
            self.on_fallback = true;
            eprintln!(
                "tfctl: agent: {} failures, showing fallback page",
                self.failures
            );
            self.write_status_page();
            let _ = self.send(KioskCmd::SetUrl(self.cfg.fallback_url.clone()));
        }
    }

    /// Refresh the local status page with the reason and our IP (TASKS 14/15).
    fn write_status_page(&self) {
        let st = crate::status::collect();
        let mut details = vec![format!("frame {}", self.cfg.frame_id)];
        if let Some(ip) = st.ips.first() {
            details.push(format!("IP {ip}"));
        }
        details.push("retrying every few seconds".into());
        let path = std::env::var("TFCTL_STATUS_PAGE")
            .unwrap_or_else(|_| crate::statuspage::DEFAULT_PATH.into());
        if let Err(e) = crate::statuspage::write(
            &path,
            &format!("cannot reach {}", self.cfg.server_url),
            &details,
        ) {
            eprintln!("tfctl: agent: status page: {e}");
        }
    }

    fn http_poll(&mut self) -> io::Result<()> {
        let url = self.api(&format!("/api/frames/{}/config", self.cfg.frame_id));
        let r = http::get(&url, &self.cfg.token, CONNECT_TIMEOUT)?;
        if r.status != 200 {
            return Err(io::Error::other(format!("GET config: HTTP {}", r.status)));
        }
        let text = String::from_utf8_lossy(&r.body);
        let v = json::parse(&text).map_err(io::Error::other)?;
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
                c.send_text(&self.status_json("heartbeat").to_string())?;
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
                c.send_text(&ack.to_string())?;
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
            if !tz.is_empty() && !tz.contains("..") && tz != self.cfg.tz {
                std::env::set_var("TZ", &tz);
                let zi = format!("/usr/share/zoneinfo/{tz}");
                if std::path::Path::new(&zi).is_file() {
                    let _ = std::fs::copy(&zi, "/etc/localtime");
                }
                self.cfg.tz = tz.clone();
                self.persist(&[("TZ", &tz)]);
            }
        }
    }
}

/// `date -s @N` (busybox); only when off by more than 2 s. No NTP on the frame.
fn set_clock(t: i64) {
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
