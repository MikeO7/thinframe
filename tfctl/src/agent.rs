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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HEARTBEAT: Duration = Duration::from_secs(60);
const PING: Duration = Duration::from_secs(30);
const DEAD: Duration = Duration::from_secs(90);
/// The Wi-Fi link down this long ends a server session at once. Without it the agent noticed a
/// lost access point only when the socket went quiet for DEAD, and rejoined 91 s after the drop
/// [V 2026-10-28]; now the rejoin follows the link (`kick_wifi`, KICK_AFTER).
const LINK_LOST: Duration = Duration::from_secs(3);
const FALLBACK_AFTER: u32 = 3;
const STABLE_SESSION: Duration = Duration::from_secs(30);
/// Failures in a row between two LAN discoveries (with the backoff, about a minute at first).
const REDISCOVER_EVERY: u32 = 3;
/// How often a frame on the USB-C server address looks for the same server over Wi-Fi.
const LEAVE_USB_EVERY: Duration = Duration::from_secs(60);

pub fn run(cfg: &Config, kiosk: Sender<KioskCmd>) -> io::Result<()> {
    serve(Agent::new(cfg, kiosk, &cfg.path)?)
}

/// The connect / session / poll loop for an agent the caller built (the kiosk prepares it first).
pub fn serve(mut a: Agent) -> io::Result<()> {
    crate::settings::LISTENING.store(true, Ordering::SeqCst);
    let mut backoff = 1u64;
    loop {
        if sys::term_requested() {
            return Ok(());
        }
        if a.take_settings_server() {
            backoff = 1;
        }
        // Wi-Fi first, then pairing, then the server (TASK 17).
        if a.needs_wifi() {
            thread::sleep(a.wifi_step());
            continue;
        }
        if a.needs_pairing() {
            thread::sleep(a.pair_step());
            continue;
        }
        a.leave_usb_link();
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
                a.saved_page = false;
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
                        let was = a.cfg.server_url.clone();
                        a.rediscover();
                        if a.cfg.server_url != was {
                            // Found it elsewhere: connect now, not after the backoff (8 s).
                            backoff = 1;
                            continue;
                        }
                    }
                }
            }
        }
        crate::settings::nap(Duration::from_secs(backoff));
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
    /// cog shows the saved dashboard (offline.rs) while the agent waits for the server.
    pub(crate) on_cached: bool,
    /// The saved dashboard was refreshed during this connection to the server.
    saved_page: bool,
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
    /// Last time the agent asked iwd to rejoin the saved network (`wifisetup::kick_due`).
    pub(crate) wifi_kicked: Option<Instant>,
    /// Last status page write while waiting for Wi-Fi (every STEP, not every WAIT_STEP).
    pub(crate) status_written: Option<Instant>,
    /// Last look for the server over the USB-C link while Wi-Fi is down.
    pub(crate) cable_asked: Option<Instant>,
    pub(crate) wifi_setup: bool,
    pub(crate) wifi_page_up: bool,
    /// Last look for the server over Wi-Fi while on the USB-C address (`leave_usb_link`).
    pub(crate) usb_checked: Option<Instant>,
    /// Last probe of the server without Wi-Fi (USB-C link to a computer): when, and the answer.
    pub(crate) direct: Option<(Instant, bool)>,
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
            on_cached: false,
            saved_page: false,
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
            wifi_kicked: None,
            status_written: None,
            cable_asked: None,
            wifi_setup: false,
            wifi_page_up: false,
            usb_checked: None,
            direct: None,
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

    fn flush_events(&self, c: &mut ws::Client) -> io::Result<()> {
        let mut queued: Vec<(String, String)> = match self.events.lock() {
            Ok(mut q) => q.drain(..).collect(),
            Err(_) => return Ok(()),
        };
        if let Ok(mut q) = crate::kiosk::EVENTS.lock() {
            queued.extend(q.drain(..));
        }
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

    /// On the USB-C cable's server address: look for the same server on the
    /// network and move there, so pulling the cable does not cut the frame off. At most every
    /// LEAVE_USB_EVERY, before a connect.
    pub(crate) fn leave_usb_link(&mut self) {
        let on_usb = self.base.host.parse().is_ok_and(crate::discover::is_usb);
        // No Wi-Fi check: iwd's view can lag the address, and with no network the broadcast
        // simply finds nothing.
        if !on_usb {
            return;
        }
        if self
            .usb_checked
            .is_some_and(|t| t.elapsed() < LEAVE_USB_EVERY)
        {
            return;
        }
        self.usb_checked = Some(Instant::now());
        let Ok(nonce) = crate::pair::nonce() else {
            return;
        };
        let token = Some(self.cfg.token.as_str()).filter(|t| !t.is_empty());
        let who = crate::discover::Asker {
            id: &self.cfg.frame_id,
            token,
        };
        let lan = crate::discover::lan_targets();
        if let Ok(Some(url)) = crate::discover::find(&lan, crate::discover::WAIT, &who, &nonce) {
            let usb = Url::parse(&url)
                .and_then(|u| u.host.parse().ok())
                .is_some_and(crate::discover::is_usb);
            if !usb {
                self.move_server(&url);
            }
        }
    }

    /// Point the agent (and the dashboard URL, when it lives on the old server) at `url`,
    /// persisted. `how`: "found on the LAN", "set on the frame".
    pub(crate) fn move_server_how(&mut self, url: &str, how: &str) {
        let url = url.trim_end_matches('/');
        let old = self.cfg.server_url.trim_end_matches('/').to_string();
        let Some(base) = Url::parse(url) else {
            return;
        };
        if url == old {
            return;
        }
        let page = crate::config::rebase(&self.cfg.url, &old, url);
        let mut kv = vec![("SERVER_URL", url.to_string())];
        if let Some(p) = &page {
            self.cfg.url = p.clone();
            kv.push(("FRAME_URL", p.clone()));
        }
        self.base = base;
        self.cfg.server_url = url.to_string();
        let kv: Vec<(&str, &str)> = kv.iter().map(|(k, v)| (*k, v.as_str())).collect();
        self.persist(&kv);
        eprintln!("tfctl: agent: server moved: {old} -> {url} ({how})");
        self.event("server", &format!("server {how}: {url} (was {old})"));
        self.failures = 0;
        self.pair_nonce = None;
        if !self.on_fallback {
            let _ = self.send(KioskCmd::SetUrl(self.cfg.url.clone()));
        }
    }

    pub(crate) fn move_server(&mut self, url: &str) {
        self.move_server_how(url, "found on the LAN");
    }

    /// A server address the person entered in Settings (settings.rs): switch to it now.
    pub(crate) fn take_settings_server(&mut self) -> bool {
        match crate::settings::take_server() {
            Some(url) => {
                self.move_server_how(&url, "set on the frame");
                true
            }
            None => false,
        }
    }

    pub(crate) fn server_ok(&mut self) {
        self.failures = 0;
        crate::watchdog::contact();
        if self.on_fallback {
            self.on_fallback = false;
            self.on_cached = false;
            eprintln!("tfctl: agent: server answered, showing the dashboard");
            let _ = self.send(KioskCmd::SetUrl(self.cfg.url.clone()));
        }
        if !self.saved_page {
            self.saved_page = true;
            self.save_page();
        }
    }

    /// Keep the dashboard as it is now for the next boot (offline.rs).
    pub(crate) fn save_page(&self) {
        crate::offline::save_in_background(
            self.cfg.url.clone(),
            self.cfg.server_url.clone(),
            self.cfg.token.clone(),
        );
    }

    /// From the saved dashboard to the status page, which says what the frame waits for.
    pub(crate) fn leave_cached(&mut self, why: &str) {
        if self.on_cached && !self.cfg.fallback_url.is_empty() {
            self.on_cached = false;
            // On the status page now: the next answer from the server must bring the dashboard
            // back, or after a re-pair (on_fallback already false) it stayed until a reboot.
            self.on_fallback = true;
            eprintln!("tfctl: agent: {why}, showing the status page");
            self.write_status_page();
            let _ = self.send(KioskCmd::SetUrl(self.cfg.fallback_url.clone()));
        }
    }

    /// Boot (TASK 15): the kiosk opens on the local status page, not on a dashboard that cannot
    /// load yet; the first answer from the server switches to the dashboard (`server_ok`).
    /// The first page for cog: the saved dashboard when there is one, else the status page.
    pub(crate) fn start_on_status_page(&mut self) -> String {
        self.on_fallback = true;
        self.write_status_page();
        match crate::offline::url() {
            Some(u) if !self.cfg.server_url.is_empty() => {
                self.on_cached = true;
                u
            }
            _ => self.cfg.fallback_url.clone(),
        }
    }

    pub(crate) fn server_failed(&mut self) {
        self.failures += 1;
        if self.on_fallback && self.failures >= FALLBACK_AFTER {
            self.leave_cached("no answer from the server");
        }
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
        let ips = crate::status::ips();
        let ssid = crate::wifi::configured_ssid(std::path::Path::new(crate::wifi::IWD_DIR));
        let headline = crate::statuspage::headline(
            &self.cfg.server_url,
            ssid.as_deref(),
            &ips,
            self.failures,
            FALLBACK_AFTER,
        );
        let mut details = vec![format!("frame {}", self.cfg.frame_id)];
        let lan: Vec<&String> = ips
            .iter()
            .filter(|ip| *ip != crate::statuspage::USB_IP)
            .collect();
        if let Some(ip) = lan.first() {
            details.push(format!("IP {ip}"));
        }
        if self.failures > 0 {
            details.push("retrying every few seconds".into());
        }
        details.push("hold the screen 5 s for settings".into());
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

    /// GET the config and apply it (versions, artifacts), nothing else.
    pub(crate) fn refresh_config(&mut self) -> io::Result<()> {
        let url = self.api(&format!("/api/frames/{}/config", self.cfg.frame_id));
        let r = http::get(&url, &self.cfg.token, CONNECT_TIMEOUT)?;
        if r.status != 200 {
            return Err(io::Error::other(format!("GET config: HTTP {}", r.status)));
        }
        let v = json::parse(&String::from_utf8_lossy(&r.body)).map_err(io::Error::other)?;
        self.apply_config(&v);
        Ok(())
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
            .set(
                "light",
                match crate::kiosk::LIGHT.load(Ordering::Relaxed) {
                    255 => None,
                    v => Some(u64::from(v)),
                },
            )
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
        let mut link_down_since: Option<Instant> = None;
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
            if self.take_settings_server() {
                c.close();
                return Err(io::Error::other("server changed in Settings, reconnecting"));
            }
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
                // On the USB-C address: Wi-Fi may have come up since connect (and is asked to).
                let was = self.cfg.server_url.clone();
                if Url::parse(&was)
                    .and_then(|u| u.host.parse().ok())
                    .is_some_and(crate::discover::is_usb)
                {
                    self.kick_wifi();
                }
                self.leave_usb_link();
                if self.cfg.server_url != was {
                    c.close();
                    return Err(io::Error::other(
                        "server moved to the network, reconnecting",
                    ));
                }
            }
            if last_ping.elapsed() >= PING {
                c.ping()?;
                last_ping = Instant::now();
            }
            if self.link_lost(&mut link_down_since) {
                c.close();
                return Err(io::Error::other("Wi-Fi link down, leaving the session"));
            }
            if last_rx.elapsed() >= DEAD {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "no traffic for 90 s",
                ));
            }
        }
    }

    /// The Wi-Fi link has been down for LINK_LOST while the server is on a network address
    /// (over the USB-C link Wi-Fi does not matter). The down time counts toward the rejoin.
    fn link_lost(&mut self, since: &mut Option<Instant>) -> bool {
        if !self.radio.link_down()
            || Url::parse(&self.cfg.server_url)
                .and_then(|u| u.host.parse().ok())
                .is_some_and(crate::discover::is_usb)
        {
            *since = None;
            return false;
        }
        let t = *since.get_or_insert_with(Instant::now);
        if !link_lost_for(t.elapsed()) {
            return false;
        }
        self.wifi_down_since.get_or_insert(t);
        true
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
/// Whether a Wi-Fi link down for `down_for` ends the server session.
fn link_lost_for(down_for: Duration) -> bool {
    down_for >= LINK_LOST
}

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

/// The server's hello carries the time (clock.rs keeps the RTC and NTP).
fn set_clock(t: i64) {
    crate::clock::set(t, "server");
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_lost_wifi_link_ends_the_session_within_seconds() {
        assert!(!link_lost_for(Duration::from_secs(2)), "a blip");
        assert!(link_lost_for(LINK_LOST));
        assert!(LINK_LOST < DEAD / 10, "far sooner than a quiet socket");
    }

    use super::*;

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
}
