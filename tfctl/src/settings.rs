//! On-frame Settings, so the frame is never stuck. Hold a finger on the screen for HOLD on any
//! page (the dashboard, the status page, a broken page, no server at all) and cog shows this
//! page, served by tfctl on 127.0.0.1:PORT: change Wi-Fi (also while connected), find the server
//! on the network or type its address, see the frame's addresses and versions, restart.
//! Like wifisetup.rs it binds loopback only, checks the Host header and answers only while on
//! screen; it closes itself after IDLE untouched.
use crate::kiosk::KioskCmd;
use crate::pair::esc;
use crate::wifi::{self, Net, Radio};
use crate::wifisetup::{
    field, file, html, not_found, pct, read_req, see_other, write_resp, Req, Resp, KEYBOARD_JS,
    POPPINS,
};
use std::io;
use std::net::TcpListener;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

pub const PORT: u16 = 8092;
/// A finger held down this long opens Settings.
pub const HOLD: Duration = Duration::from_secs(5);
/// Untouched this long, Settings closes and the frame goes back to its page.
pub const IDLE: Duration = Duration::from_secs(180);
const RESCAN: Duration = Duration::from_secs(20);
const OPEN_EVERY: Duration = Duration::from_secs(10);
/// An address typed without a port: the server's default (discover::PORT), the old default,
/// then plain HTTP.
const PORTS: [u16; 3] = [crate::discover::PORT, 8080, 80];
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Something takes server changes from Settings: the agent, or the kiosk's LAN search when
/// the frame has no server yet. Without either (SERVER_URL unusable) tfctl restarts instead.
pub static LISTENING: AtomicBool = AtomicBool::new(false);
static SERVER: Mutex<Option<String>> = Mutex::new(None);

/// The server address Settings saved, once.
pub fn take_server() -> Option<String> {
    SERVER.lock().ok()?.take()
}

fn server_pending() -> bool {
    SERVER.lock().map(|s| s.is_some()).unwrap_or(false)
}

/// Sleep `d`, or less when Settings saved a server meanwhile (the agent's backoff is up to 60 s).
pub fn nap(d: Duration) {
    let end = Instant::now() + d;
    loop {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() || server_pending() || crate::sys::term_requested() {
            return;
        }
        thread::sleep(left.min(Duration::from_millis(250)));
    }
}

#[derive(Default)]
pub struct State {
    /// On screen; the server answers 404 otherwise.
    pub active: bool,
    /// The last request a person made (not an automatic refresh), for IDLE.
    pub last_used: Option<Instant>,
    nets: Vec<Net>,
    scanned: Option<Instant>,
    scanning: bool,
    joining: Option<String>,
    /// Shown once at the top of the next page: (good news, text).
    note: Option<(bool, String)>,
    /// A server the LAN search found: (url, it proved it knows this frame's token).
    found: Option<(String, bool)>,
    /// A typed address that did not answer, offered as "use it anyway".
    unanswered: Option<String>,
    /// When a page last opened Settings with POST /settings/open.
    opened: Option<Instant>,
}

pub type Shared = Arc<Mutex<State>>;

pub fn lock(st: &Shared) -> MutexGuard<'_, State> {
    st.lock().unwrap_or_else(|e| e.into_inner())
}

/// Open on screen: answers from now on, IDLE counts from now.
pub fn open(st: &Shared) {
    let mut s = lock(st);
    s.active = true;
    s.last_used = Some(Instant::now());
    s.note = None;
    s.found = None;
    s.unanswered = None;
}

pub fn close(st: &Shared) {
    lock(st).active = false;
}

pub fn idle(st: &Shared) -> bool {
    let s = lock(st);
    s.active && s.last_used.is_some_and(|t| t.elapsed() >= IDLE)
}

pub fn url(path: &str) -> String {
    format!("http://127.0.0.1:{PORT}{path}")
}

/// What a typed address may mean, in the order to try: "nas", "nas:41007", "http://10.0.0.5/".
/// Err is the text to show.
pub fn candidates(input: &str) -> Result<Vec<String>, String> {
    let s = input.trim();
    if s.starts_with("https://") {
        return Err("The frame speaks plain http only.".into());
    }
    let s = s.strip_prefix("http://").unwrap_or(s);
    let hostport = s.split('/').next().unwrap_or("");
    let ok = |c: char| c.is_ascii_alphanumeric() || ".-:[]".contains(c);
    if hostport.is_empty() || !hostport.chars().all(ok) {
        return Err("Type the server's name or address, like 192.168.1.5 or nas.local.".into());
    }
    let has_port = match hostport.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(':') && !p.contains(']') => {
            if p.parse::<u16>().is_err() || h.is_empty() {
                return Err(format!("{p:?} is not a port."));
            }
            true
        }
        _ => false,
    };
    let c: Vec<String> = if has_port {
        vec![format!("http://{hostport}")]
    } else {
        PORTS
            .iter()
            .map(|p| {
                if *p == 80 {
                    format!("http://{hostport}")
                } else {
                    format!("http://{hostport}:{p}")
                }
            })
            .collect()
    };
    if c.iter().all(|u| crate::http::Url::parse(u).is_some()) {
        Ok(c)
    } else {
        Err("That address does not look right.".into())
    }
}

/// A thinframe server answers there (GET /healthz).
fn probe(url: &str, timeout: Duration) -> bool {
    crate::http::Url::parse(url)
        .map(|u| u.with_path("/healthz"))
        .and_then(|u| crate::http::get(&u, "", timeout).ok())
        .is_some_and(|r| r.status == 200)
}

struct Ctx {
    st: Shared,
    radio: Arc<dyn Radio>,
    kiosk: Sender<KioskCmd>,
    conf: String,
    port: u16,
}

/// Bind `addr` (127.0.0.1:PORT on the frame, :0 in tests) and serve in a thread; returns the port.
pub fn start(
    st: Shared,
    radio: Arc<dyn Radio>,
    kiosk: Sender<KioskCmd>,
    conf: String,
    addr: &str,
) -> io::Result<u16> {
    let l = TcpListener::bind(addr)?;
    let port = l.local_addr()?.port();
    thread::Builder::new()
        .name("settings".into())
        .spawn(move || {
            let cx = Ctx {
                st,
                radio,
                kiosk,
                conf,
                port,
            };
            for mut s in l.incoming().flatten() {
                let resp = match read_req(&mut s) {
                    Ok(req) => cx.respond(&req),
                    Err(_) => continue,
                };
                let _ = write_resp(&mut s, &resp);
            }
        })?;
    Ok(port)
}

impl Ctx {
    fn cfg(&self) -> crate::config::Config {
        crate::config::load(&self.conf).unwrap_or_default()
    }

    fn note(&self, good: bool, text: impl Into<String>) {
        lock(&self.st).note = Some((good, text.into()));
    }

    fn respond(&self, req: &Req) -> Resp {
        // DNS rebinding: only our own name for loopback.
        let host_ok = [
            format!("127.0.0.1:{}", self.port),
            format!("localhost:{}", self.port),
        ]
        .contains(&req.host);
        if !host_ok {
            return not_found();
        }
        // The gear on the dashboard and the button on the status page: a form posted to the
        // top window. Opens Settings even while closed; the kiosk remembers the page to go back to.
        if req.method == "POST" && req.path == "/settings/open" {
            // Once per OPEN_EVERY: a page posting this in a loop cannot hold Settings on screen.
            let mut s = lock(&self.st);
            if s.active || s.opened.is_some_and(|t| t.elapsed() < OPEN_EVERY) {
                return see_other("/settings");
            }
            s.opened = Some(Instant::now());
            drop(s);
            open(&self.st);
            let _ = self.kiosk.send(KioskCmd::SettingsShown("/settings".into()));
            return see_other("/settings");
        }
        if !lock(&self.st).active {
            return not_found();
        }
        // Every other form must come from Settings itself. Opening is allowed from any page on
        // the frame (the dashboard, an app, the status page); restarting, resetting or changing
        // Wi-Fi and the server is not: an app could post /settings/open and then those.
        let own = [
            format!("http://127.0.0.1:{}", self.port),
            format!("http://localhost:{}", self.port),
        ];
        if req.method == "POST" && !own.contains(&req.origin) {
            eprintln!("tfctl: settings: refused {} from {}", req.path, req.origin);
            return not_found();
        }
        if field(&req.query, "auto").is_none() {
            lock(&self.st).last_used = Some(Instant::now());
        }
        match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/settings") => html(self.home()),
            ("GET", "/settings/wifi") => {
                self.scan_if_stale();
                html(self.wifi_page())
            }
            ("POST", "/settings/wifi/scan") => {
                lock(&self.st).scanned = None;
                self.scan_if_stale();
                see_other("/settings/wifi")
            }
            ("GET", "/settings/wifi/net") => match field(&req.query, "ssid") {
                Some(ssid) => html(self.password_page(ssid, field(&req.query, "sec"))),
                None => see_other("/settings/wifi"),
            },
            ("POST", "/settings/wifi/join") => {
                self.join(req);
                see_other("/settings/wifi")
            }
            ("POST", "/settings/wifi/forget") => {
                if let Some(ssid) = field(&req.form, "ssid").filter(|s| wifi::valid_ssid(s)) {
                    wifi::forget(ssid);
                    eprintln!("tfctl: settings: forgot Wi-Fi {ssid}");
                    self.note(true, format!("Forgot {ssid}."));
                }
                see_other("/settings/wifi")
            }
            ("GET", "/settings/server") => html(self.server_page()),
            ("POST", "/settings/server/find") => {
                self.find();
                see_other("/settings/server")
            }
            ("GET", "/settings/server/edit") => html(self.address_page()),
            ("POST", "/settings/server/set") => {
                self.set_address(req);
                see_other("/settings/server")
            }
            ("POST", "/settings/server/use") => {
                match field(&req.form, "url").and_then(|u| candidates(u).ok()) {
                    Some(c) => self.use_server(&c[0]),
                    None => self.note(false, "That address does not look right."),
                }
                see_other("/settings/server")
            }
            ("POST", "/settings/reboot") => {
                eprintln!("tfctl: settings: restart asked on the screen");
                crate::sysupdate::reboot_soon();
                html(page(
                    "Restarting",
                    "",
                    "<div class=\"card center\"><div class=\"spin\"></div><div class=\"v\">Restarting</div>\
                     <div class=\"sub\">Back in about a minute.</div></div>",
                    None,
                ))
            }
            ("GET", "/settings/reset") => html(page(
                "Reset",
                "/settings",
                &format!(
                    "<div class=\"card col warn\"><div class=\"row\"><span class=\"ic reset\">{ICON_RESET}</span>\
                     <div class=\"t\"><div class=\"v\">Start this frame over?</div><div class=\"sub\">It restarts, \
                     asks for Wi-Fi, finds the server on the network and shows a pairing code.</div></div></div>\
                     <div class=\"cols\"><div><b>Forgets</b><ul class=\"plain\"><li>Every saved Wi-Fi network</li>\
                     <li>The server address</li><li>The pairing with the server</li></ul></div>\
                     <div><b>Keeps</b><ul class=\"plain\"><li>Screen and touch settings</li><li>SSH keys</li>\
                     <li>The installed software</li></ul></div></div>\
                     <div class=\"row end\"><a class=\"btn btn2\" href=\"/settings\">Cancel</a>\
                     <form method=\"post\" action=\"/settings/reset/yes\"><button class=\"solid-danger\">\
                     {ICON_RESET}Reset frame</button></form></div></div>"
                ),
                None,
            )),
            ("POST", "/settings/reset/yes") => {
                eprintln!("tfctl: settings: reset asked on the screen");
                if let Err(e) = reset(&self.conf, Path::new(wifi::IWD_DIR), wifi::forget) {
                    self.note(false, format!("Reset failed: {e}"));
                    return see_other("/settings");
                }
                crate::sysupdate::reboot_soon();
                html(page(
                    "Restarting",
                    "",
                    "<div class=\"card center\"><div class=\"spin\"></div><div class=\"v\">Reset done</div>\
                     <div class=\"sub\">Back in about a minute, asking for Wi-Fi.</div></div>",
                    None,
                ))
            }
            ("POST", "/settings/close") => {
                let _ = self.kiosk.send(KioskCmd::CloseSettings);
                html(page("Settings", "", "<p class=\"dim\">Closing…</p>", None))
            }
            ("GET", "/settings/poppins.ttf") => file(POPPINS, "font/ttf"),
            _ => not_found(),
        }
    }

    fn scan_if_stale(&self) {
        let mut s = lock(&self.st);
        if s.scanning || s.joining.is_some() || s.scanned.is_some_and(|t| t.elapsed() < RESCAN) {
            return;
        }
        s.scanning = true;
        drop(s);
        let (st, radio) = (self.st.clone(), self.radio.clone());
        let _ = thread::Builder::new()
            .name("settings-scan".into())
            .spawn(move || {
                let nets = radio.scan();
                let mut s = lock(&st);
                if !nets.is_empty() || s.nets.is_empty() {
                    s.nets = nets;
                }
                s.scanned = Some(Instant::now());
                s.scanning = false;
            });
    }

    /// Join in a thread (up to 40 s). A failed join leaves the saved networks as they were, so
    /// iwd goes back to the old one.
    fn join(&self, req: &Req) {
        let ssid = field(&req.form, "ssid").unwrap_or("").to_string();
        let psk = field(&req.form, "psk").unwrap_or("").to_string();
        let open = psk.is_empty() && field(&req.form, "sec") == Some("open");
        let mut s = lock(&self.st);
        if !wifi::valid_ssid(&ssid) {
            s.note = Some((false, "Pick a network first.".into()));
            return;
        }
        if !open && !wifi::valid_psk(&psk) {
            s.note = Some((false, "Wi-Fi passwords are 8 to 63 characters.".into()));
            return;
        }
        if s.joining.is_some() {
            return;
        }
        s.joining = Some(ssid.clone());
        s.note = None;
        drop(s);
        let (st, radio) = (self.st.clone(), self.radio.clone());
        let _ = thread::Builder::new()
            .name("settings-join".into())
            .spawn(move || {
                let r = radio.join(&ssid, (!open).then_some(psk.as_str()));
                let mut s = lock(&st);
                s.joining = None;
                s.note = Some(match r {
                    Ok(()) => {
                        eprintln!("tfctl: settings: joined Wi-Fi {ssid}");
                        (true, format!("Connected to {ssid}."))
                    }
                    Err(e) => {
                        eprintln!("tfctl: settings: join {ssid}: {e}");
                        (
                            false,
                            format!("Could not join {ssid} ({e}). Check the password."),
                        )
                    }
                });
            });
    }

    /// Ask the network: first for a server that knows this frame, then for any.
    fn find(&self) {
        let cfg = self.cfg();
        let mut targets = crate::discover::lan_targets();
        targets.push(crate::discover::targets()[1]);
        let ask = |token: Option<&str>| {
            let nonce = crate::pair::nonce().ok()?;
            let who = crate::discover::Asker {
                id: &cfg.frame_id,
                token,
            };
            crate::discover::find(&targets, crate::discover::WAIT, &who, &nonce)
                .ok()
                .flatten()
        };
        let token = Some(cfg.token.as_str()).filter(|t| !t.is_empty());
        let found = match token.and_then(|t| ask(Some(t))) {
            Some(u) => Some((u, true)),
            None => ask(None).map(|u| (u, token.is_none())),
        };
        let mut s = lock(&self.st);
        s.unanswered = None;
        match found {
            Some(f) => {
                eprintln!("tfctl: settings: found a server at {}", f.0);
                s.found = Some(f);
                s.note = None;
            }
            None => {
                s.found = None;
                s.note = Some((
                    false,
                    "No thinframe server answered on this network.".into(),
                ));
            }
        }
    }

    fn set_address(&self, req: &Req) {
        let raw = field(&req.form, "addr").unwrap_or("");
        let c = match candidates(raw) {
            Ok(c) => c,
            Err(e) => return self.note(false, e),
        };
        match c.iter().find(|u| probe(u, PROBE_TIMEOUT)) {
            Some(u) => self.use_server(u),
            None => {
                let mut s = lock(&self.st);
                s.found = None;
                s.unanswered = Some(c[0].clone());
                s.note = Some((
                    false,
                    format!("No thinframe server answered at {}.", raw.trim()),
                ));
            }
        }
    }

    /// Save `url` as SERVER_URL (and the dashboard address with it when it lived on the old
    /// server), then hand it to whoever runs the connection.
    fn use_server(&self, url: &str) {
        let url = url.trim_end_matches('/');
        let cfg = self.cfg();
        let old = cfg.server_url.trim_end_matches('/');
        let page = crate::config::rebase(&cfg.url, old, url);
        let mut kv = vec![("SERVER_URL", url)];
        if let Some(p) = &page {
            kv.push(("FRAME_URL", p));
        }
        if let Err(e) = crate::config::save_overrides(&self.conf, &kv) {
            return self.note(false, format!("Could not save: {e}"));
        }
        eprintln!("tfctl: settings: server set to {url} (was {old:?})");
        {
            let mut s = lock(&self.st);
            s.found = None;
            s.unanswered = None;
            s.note = Some((true, format!("Server set to {url}. Connecting…")));
        }
        if LISTENING.load(Ordering::SeqCst) {
            if let Ok(mut s) = SERVER.lock() {
                *s = Some(url.to_string());
            }
        } else {
            // Nothing runs a connection (SERVER_URL was unusable): start again with the new one.
            let _ = self.kiosk.send(KioskCmd::Exit);
        }
    }

    // ---- pages ----

    fn home(&self) -> String {
        let cfg = self.cfg();
        let st = crate::status::collect();
        let ips: Vec<&String> = st
            .ips
            .iter()
            .filter(|ip| *ip != crate::statuspage::USB_IP)
            .collect();
        let (wifi_name, wifi_state) = match (wifi::connected_ssid(), ips.first()) {
            (Some(n), Some(ip)) => (
                esc(&n),
                format!("{} &middot; {ip}", pill(true, "Connected")),
            ),
            (Some(n), None) => (esc(&n), pill_dim("Getting an address")),
            (None, Some(ip)) => (
                "Online".into(),
                format!("{} &middot; {ip}", pill(true, "Connected")),
            ),
            (None, None) => ("No network".into(), pill(false, "Not connected")),
        };
        let (server_name, server_state) = if cfg.server_url.is_empty() {
            ("Not set".to_string(), pill(false, "Not set"))
        } else {
            let name = esc(cfg.server_url.trim_start_matches("http://"));
            if probe(&cfg.server_url, Duration::from_millis(1500)) {
                (name, pill(true, "Answering"))
            } else {
                (name, pill(false, "Not answering"))
            }
        };
        let up = st.uptime_s.map_or("?".into(), |s| {
            if s >= 86400 {
                format!("{}d {}h", s / 86400, s % 86400 / 3600)
            } else {
                format!("{}h {}m", s / 3600, s % 3600 / 60)
            }
        });
        let opt = |v: &Option<String>| esc(v.as_deref().unwrap_or("?").trim());
        let body = format!(
            "{note}<div class=\"grid2\">\
             <a class=\"card tile\" href=\"/settings/wifi\"><span class=\"ic wifi\">{ic_wifi}</span>\
             <div class=\"t\"><b>Wi-Fi</b><div class=\"v\">{wifi_name}</div><div class=\"sub\">{wifi_state}</div></div>\
             <span class=\"chev\">{ic_chev}</span></a>\
             <a class=\"card tile\" href=\"/settings/server\"><span class=\"ic server\">{ic_server}</span>\
             <div class=\"t\"><b>Server</b><div class=\"v\">{server_name}</div><div class=\"sub\">{server_state}</div></div>\
             <span class=\"chev\">{ic_chev}</span></a></div>\
             <div class=\"card\"><span class=\"ic frame\">{ic_frame}</span><div class=\"t\"><b>This frame</b>\
             <div class=\"v\">{id}</div><div class=\"sub\">Software {sw} &middot; up {up}</div>\
             <div class=\"sub\">System {sys} &middot; slot {slot}</div></div>\
             <form method=\"post\" action=\"/settings/reboot\"><button class=\"btn2\">{ic_restart}Restart</button></form>\
             <a class=\"btn danger\" href=\"/settings/reset\">{ic_reset}Reset</a></div>\
             <p class=\"tip\">{ic_hand}Hold the screen for 5 seconds anywhere, or tap the gear on the dashboard, to come back here.</p>",
            note = self.take_note(),
            id = esc(&cfg.frame_id),
            sw = opt(&st.software_version),
            sys = opt(&st.system_version),
            slot = opt(&st.slot),
            ic_wifi = ICON_WIFI,
            ic_server = ICON_SERVER,
            ic_frame = ICON_FRAME,
            ic_chev = ICON_CHEV,
            ic_restart = ICON_RESTART,
            ic_reset = ICON_RESET,
            ic_hand = ICON_HAND,
        );
        page("Settings", "", &body, Some(15))
    }

    fn take_note(&self) -> String {
        match lock(&self.st).note.take() {
            Some((good, t)) => format!(
                "<div class=\"note {}\">{}</div>",
                if good { "good" } else { "badn" },
                esc(&t)
            ),
            None => String::new(),
        }
    }

    fn wifi_page(&self) -> String {
        let (nets, scanning, joining) = {
            let s = lock(&self.st);
            (s.nets.clone(), s.scanning, s.joining.clone())
        };
        if let Some(j) = joining {
            let body = format!(
                "<div class=\"card\"><div class=\"t\"><b>Joining</b><div class=\"v\">{}…</div>\
                 <p class=\"dim\">This can take half a minute.</p></div></div>",
                esc(&j)
            );
            return page("Wi-Fi", "/settings", &body, Some(2));
        }
        let current = wifi::connected_ssid();
        let saved = wifi::configured_ssids(Path::new(wifi::IWD_DIR));
        let mut body = self.take_note();
        body.push_str("<div class=\"card col\"><b>Networks in range</b><ul>");
        if nets.is_empty() {
            body.push_str("<li class=\"dim\">Looking for networks…</li>");
        }
        for n in nets.iter().take(12) {
            let tag = if current.as_deref() == Some(n.ssid.as_str()) {
                "<span class=\"pill ok\">Connected</span>"
            } else if n.security == "open" {
                "<span class=\"pill\">Open</span>"
            } else {
                ""
            };
            if n.supported() {
                body.push_str(&format!(
                    "<li><a href=\"/settings/wifi/net?ssid={}&amp;sec={}\"><span class=\"name\">{}</span>{tag}\
                     {}</a></li>",
                    pct(&n.ssid),
                    pct(&n.security),
                    esc(&n.ssid),
                    bars(n.bars())
                ));
            } else {
                body.push_str(&format!(
                    "<li class=\"dim\"><span class=\"name\">{}</span> {} (not supported)</li>",
                    esc(&n.ssid),
                    esc(&n.security)
                ));
            }
        }
        body.push_str("</ul><form method=\"post\" action=\"/settings/wifi/scan\"><button class=\"btn2\">Scan again</button></form></div>");
        let others: Vec<&String> = saved
            .iter()
            .filter(|s| current.as_deref() != Some(s.as_str()))
            .collect();
        if !others.is_empty() {
            body.push_str("<div class=\"card col\"><b>Saved networks</b><ul>");
            for s in others {
                body.push_str(&format!(
                    "<li><span class=\"name\">{}</span><form method=\"post\" action=\"/settings/wifi/forget\">\
                     <input type=\"hidden\" name=\"ssid\" value=\"{}\"><button class=\"btn2\">Forget</button></form></li>",
                    esc(s),
                    esc(s)
                ));
            }
            body.push_str("</ul></div>");
        }
        let refresh = if scanning || nets.is_empty() { 3 } else { 20 };
        page("Wi-Fi", "/settings", &body, Some(refresh))
    }

    fn password_page(&self, ssid: &str, sec: Option<&str>) -> String {
        let open = sec == Some("open");
        let head = format!(
            "<form method=\"post\" action=\"/settings/wifi/join\" class=\"row\">\
             <input type=\"hidden\" name=\"ssid\" value=\"{}\"><input type=\"hidden\" name=\"sec\" value=\"{}\">\
             <input type=\"hidden\" name=\"psk\" id=\"psk\">",
            esc(ssid),
            if open { "open" } else { "psk" }
        );
        if open {
            let body = format!(
                "<div class=\"card\"><div class=\"t\"><b>Open network</b><div class=\"v\">{}</div></div>\
                 {head}<button>Join</button></form></div>",
                esc(ssid)
            );
            return page("Wi-Fi", "/settings/wifi", &body, None);
        }
        let body = format!(
            "<div class=\"top\"><div class=\"field\" id=\"pw\"></div>{head}\
             <button id=\"go\" disabled>Join {}</button></form></div><div id=\"kb\" data-min=\"8\"></div>{KEYBOARD_JS}",
            esc(ssid)
        );
        page("Wi-Fi password", "/settings/wifi", &body, None)
    }

    fn server_page(&self) -> String {
        let cfg = self.cfg();
        let (found, unanswered) = {
            let s = lock(&self.st);
            (s.found.clone(), s.unanswered.clone())
        };
        let mut body = self.take_note();
        let now = if cfg.server_url.is_empty() {
            "not set".to_string()
        } else {
            esc(&cfg.server_url)
        };
        body.push_str(&format!(
            "<div class=\"card\"><div class=\"t\"><b>Now</b><div class=\"v\">{now}</div></div></div>"
        ));
        if let Some((url, knows)) = found {
            let what = if knows {
                "<span class=\"ok\">knows this frame</span>"
            } else {
                "<span class=\"dim\">new to this frame: it will show PAIR</span>"
            };
            body.push_str(&format!(
                "<div class=\"card\"><div class=\"t\"><b>Found</b><div class=\"v\">{} &middot; {what}</div></div>\
                 <form method=\"post\" action=\"/settings/server/use\"><input type=\"hidden\" name=\"url\" value=\"{}\">\
                 <button>Use it</button></form></div>",
                esc(&url),
                esc(&url)
            ));
        }
        if let Some(url) = unanswered {
            body.push_str(&format!(
                "<div class=\"card\"><div class=\"t\"><b>Not answering</b><div class=\"v\">{}</div>\
                 <p class=\"dim\">Use it anyway if the server is only off for now.</p></div>\
                 <form method=\"post\" action=\"/settings/server/use\"><input type=\"hidden\" name=\"url\" value=\"{}\">\
                 <button class=\"btn2\">Use anyway</button></form></div>",
                esc(&url),
                esc(&url)
            ));
        }
        body.push_str(
            "<div class=\"row\"><form method=\"post\" action=\"/settings/server/find\">\
             <button>Find on the network</button></form>\
             <a class=\"btn btn2\" href=\"/settings/server/edit\">Type an address</a></div>",
        );
        page("Server", "/settings", &body, None)
    }

    fn address_page(&self) -> String {
        let body = format!(
            "<div class=\"top\"><div class=\"field\" id=\"pw\" data-hint=\"192.168.1.5 or nas.local:41007\"></div>\
             <form method=\"post\" action=\"/settings/server/set\" class=\"row\">\
             <input type=\"hidden\" name=\"addr\" id=\"psk\"><button id=\"go\" disabled>Connect</button></form></div>\
             <div id=\"kb\" data-min=\"1\"></div>{KEYBOARD_JS}"
        );
        page("Server address", "/settings/server", &body, None)
    }
}

const CSS: &str = "@font-face{font-family:P;src:url(/settings/poppins.ttf)}\
*{box-sizing:border-box}\
html,body{margin:0;height:100%;overflow:hidden;color:#eef1f7;font:3.6vh P,'DejaVu Sans',sans-serif;user-select:none;-webkit-user-select:none;touch-action:manipulation}\
body{background:radial-gradient(70vw 70vh at 92% -18%,rgba(124,92,255,.30),transparent 62%),radial-gradient(60vw 60vh at -8% 112%,rgba(56,140,255,.22),transparent 60%),#070a14}\
svg{fill:none;stroke:currentColor;stroke-width:1.9;stroke-linecap:round;stroke-linejoin:round}\
header{display:flex;align-items:center;gap:2.2vw;padding:4vh 4vw 2.6vh}\
.ttl{flex:1}.ttl small{display:block;font-size:2.2vh;letter-spacing:.24em;text-transform:uppercase;color:#7d87a0}\
h1{margin:0;font-size:6.6vh;font-weight:400;letter-spacing:-.01em;line-height:1.1}\
.back{display:flex;align-items:center;justify-content:center;flex:none;width:8.4vh;height:8.4vh;border-radius:50%;background:rgba(255,255,255,.07);box-shadow:inset 0 0 0 1px rgba(255,255,255,.1);color:#eef1f7}\
.back svg{width:4vh;height:4vh;stroke-width:2.2}\
main{padding:0 4vw 4vh;height:78vh;overflow-y:auto}\
.card{position:relative;display:flex;align-items:center;gap:2.2vw;padding:3vh 2.6vw;margin:0 0 2.4vh;border-radius:3.2vh;background:linear-gradient(180deg,rgba(255,255,255,.075),rgba(255,255,255,.035));box-shadow:inset 0 0 0 1px rgba(255,255,255,.09),inset 0 1px 0 rgba(255,255,255,.08);color:inherit;text-decoration:none}\
.card.col{display:block}.card.center{flex-direction:column;justify-content:center;text-align:center;padding:9vh 4vw;gap:1.2vh}\
.grid2{display:grid;grid-template-columns:1fr 1fr;gap:2vw;margin:0 0 2.4vh}.grid2 .card{margin:0}\
.tile:active{background:rgba(255,255,255,.12)}\
.t{flex:1;min-width:0}\
b{display:block;font-weight:400;color:#8f99ad;font-size:2.2vh;letter-spacing:.18em;text-transform:uppercase;margin-bottom:.8vh}\
.v{font-size:4.4vh;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;line-height:1.25}\
.sub{margin-top:1vh;font-size:2.8vh;color:#9aa4b8;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}\
.small{font-size:3vh}.dim{color:#8f99ad}.ok{color:#5fe3a1}.bad{color:#ff7a6e}\
.ic{flex:none;display:flex;align-items:center;justify-content:center;width:10vh;height:10vh;border-radius:2.8vh;color:#fff;box-shadow:inset 0 1px 0 rgba(255,255,255,.3)}\
.ic svg{width:5vh;height:5vh;stroke-width:1.8}\
.ic.wifi{background:linear-gradient(140deg,#38bdf8,#4f6bff)}.ic.server{background:linear-gradient(140deg,#a78bfa,#6d4aff)}\
.ic.frame{background:linear-gradient(140deg,#6b7a90,#323d4f)}.ic.reset{background:linear-gradient(140deg,#ff8a7a,#e5484d)}\
.chev{color:#5d6678;display:flex}.chev svg{width:4.4vh;height:4.4vh;stroke-width:2.2}\
.pill{display:inline-flex;align-items:center;gap:.9vh;padding:.5vh 1.6vh;border-radius:99px;font-size:2.5vh;background:rgba(255,255,255,.08);color:#c6cdda;vertical-align:.3vh}\
.pill.ok{background:rgba(95,227,161,.14);color:#7ef0b5}.pill.bad{background:rgba(255,122,110,.15);color:#ff9b91}\
.pill.ok::before,.pill.bad::before{content:'';width:1.1vh;height:1.1vh;border-radius:50%;background:currentColor}\
.chips{display:flex;flex-wrap:wrap;gap:1vh}.chips span{padding:.7vh 1.6vh;border-radius:99px;background:rgba(255,255,255,.06);box-shadow:inset 0 0 0 1px rgba(255,255,255,.07);font-size:2.5vh;color:#c6cdda;white-space:nowrap}\
button,.btn{display:inline-flex;align-items:center;justify-content:center;gap:1.1vh;font:3.2vh P,'DejaVu Sans',sans-serif;color:#fff;background:linear-gradient(135deg,#7c5cff,#4f8cff);border:0;border-radius:99px;padding:2vh 3vw;text-decoration:none;white-space:nowrap;box-shadow:0 .8vh 2.4vh rgba(92,108,255,.3),inset 0 1px 0 rgba(255,255,255,.25)}\
button svg,.btn svg{width:3.2vh;height:3.2vh;stroke-width:2.1}\
.btn2{background:rgba(255,255,255,.08);color:#eef1f7;box-shadow:inset 0 0 0 1px rgba(255,255,255,.12)}\
.danger{background:rgba(255,90,80,.12);color:#ff9b91;box-shadow:inset 0 0 0 1px rgba(255,110,100,.32)}\
.solid-danger{background:linear-gradient(135deg,#ff6b5e,#e5484d);box-shadow:0 .8vh 2.4vh rgba(229,72,77,.32),inset 0 1px 0 rgba(255,255,255,.25)}\
button:disabled{opacity:.35;box-shadow:none}\
button:active,.btn:active,.back:active{transform:scale(.96)}\
form{margin:0}.row{display:flex;gap:1.6vw;align-items:center}.row.end{justify-content:flex-end;margin-top:3.4vh}\
.note{border-radius:2.4vh;padding:2vh 2.4vw;margin:0 0 2.4vh;font-size:3vh}\
.good{background:rgba(95,227,161,.10);box-shadow:inset 0 0 0 1px rgba(95,227,161,.3);color:#a6f5cb}\
.badn{background:rgba(255,122,110,.10);box-shadow:inset 0 0 0 1px rgba(255,122,110,.3);color:#ffb3ab}\
ul{list-style:none;margin:1vh 0 2.4vh;padding:0}li,li a{display:flex;align-items:center;gap:1.6vw}\
li{border-bottom:1px solid rgba(255,255,255,.07)}li:last-child{border:0}\
li a{flex:1;color:inherit;text-decoration:none;padding:2.4vh .8vw;border-radius:1.6vh}li a:active{background:rgba(255,255,255,.07)}\
li>.name{padding:1.6vh .8vw}.name{flex:1;overflow:hidden;white-space:nowrap;text-overflow:ellipsis}\
ul.plain{margin:0}ul.plain li{border:0;padding:.6vh 0;font-size:3vh;color:#c6cdda}\
ul.plain li::before{content:'';width:.9vh;height:.9vh;border-radius:50%;background:#8b74ff;margin-right:1.4vh;flex:none}\
.cols{display:grid;grid-template-columns:1fr 1fr;gap:3vw;margin-top:3.4vh}\
.warn .v{white-space:normal}.warn .sub{white-space:normal}\
.bars{width:4vh;height:3.2vh;fill:#eef1f7;stroke:none;flex:none}.bars .off{fill:rgba(255,255,255,.18)}\
.tip{display:flex;align-items:center;gap:1.4vh;color:#7d87a0;font-size:2.6vh;margin:1.6vh .6vw 0}.tip svg{width:3.4vh;height:3.4vh;flex:none}\
.spin{width:7vh;height:7vh;border-radius:50%;border:.7vh solid rgba(255,255,255,.12);border-top-color:#8b74ff;animation:r 1s linear infinite;margin-bottom:1.6vh}\
@keyframes r{to{transform:rotate(360deg)}}\
.top{display:flex;align-items:center;gap:2vw}\
.field{flex:1;font:4.2vh 'DejaVu Sans Mono',monospace;background:rgba(255,255,255,.06);box-shadow:inset 0 0 0 1px rgba(124,92,255,.6),0 0 0 .5vh rgba(124,92,255,.14);border-radius:2vh;padding:.45em .7em;white-space:pre;overflow:hidden}.hint{color:#5d6678}\
#kb{margin:2.4vh 0 0}#kb div{display:flex;justify-content:center;gap:.6vw;margin-bottom:1.2vh}\
#kb button{font:4vh 'DejaVu Sans Mono',monospace;min-width:7.4vw;height:9.4vh;padding:0;border-radius:1.8vh;background:rgba(255,255,255,.075);color:#eef1f7;box-shadow:inset 0 0 0 1px rgba(255,255,255,.07),inset 0 1px 0 rgba(255,255,255,.06)}\
#kb button.k2{background:rgba(255,255,255,.035);min-width:10.5vw;font:2.8vh P,sans-serif}\
#kb button.on{background:linear-gradient(135deg,#7c5cff,#4f8cff);color:#fff}#kb button.sp{min-width:44vw;font:2.8vh P,sans-serif}";

/// A page: title, a back link (empty: none), the body, an optional self-refresh in seconds
/// (marked `auto` so it does not count as use).
/// Reset network and server: forget every saved Wi-Fi network (`forget` per SSID found in
/// `iwd_dir`) and blank SERVER_URL, FRAME_URL and the pairing token in `conf`. The rest of the
/// config (rotation, splash, touch) stays.
fn reset(conf: &str, iwd_dir: &Path, forget: impl Fn(&str)) -> io::Result<()> {
    // The config first: if it cannot be saved, Wi-Fi stays and nothing is half reset.
    crate::config::save_overrides(
        conf,
        &[("SERVER_URL", ""), ("FRAME_URL", ""), ("FRAME_TOKEN", "")],
    )?;
    for ssid in wifi::configured_ssids(iwd_dir) {
        forget(&ssid);
    }
    Ok(())
}

fn page(title: &str, back: &str, body: &str, refresh: Option<u32>) -> String {
    let meta = refresh
        .map(|r| format!("<meta http-equiv=\"refresh\" content=\"{r};url=?auto=1\">"))
        .unwrap_or_default();
    let back = if back.is_empty() {
        String::new()
    } else {
        format!("<a class=\"back\" href=\"{back}\" aria-label=\"Back\">{ICON_BACK}</a>")
    };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">{meta}<title>Settings</title>\
         <style>{CSS}</style></head><body><header>{back}<div class=\"ttl\"><small>thinframe</small>\
         <h1>{}</h1></div><form method=\"post\" action=\"/settings/close\"><button>Done</button></form></header>\
         <main>{body}</main></body></html>",
        esc(title)
    )
}

/// A status chip: green (good) or red, with a dot.
fn pill(good: bool, text: &str) -> String {
    format!(
        "<span class=\"pill {}\">{text}</span>",
        if good { "ok" } else { "bad" }
    )
}

fn pill_dim(text: &str) -> String {
    format!("<span class=\"pill\">{text}</span>")
}

/// Four signal bars, `n` of them lit.
fn bars(n: u8) -> String {
    let mut svg = String::from("<svg class=\"bars\" viewBox=\"0 0 20 16\">");
    for i in 0..4u8 {
        let h = 4 + i * 4;
        svg.push_str(&format!(
            "<rect x=\"{}\" y=\"{}\" width=\"3.4\" height=\"{h}\" rx=\"1\"{}/>",
            i * 5,
            16 - h,
            if i < n { "" } else { " class=\"off\"" }
        ));
    }
    svg.push_str("</svg>");
    svg
}

const ICON_WIFI: &str = "<svg viewBox=\"0 0 24 24\"><path d=\"M2 8.8a15 15 0 0 1 20 0M5 12.6a10 10 0 0 1 14 0M8.5 16.4a5 5 0 0 1 7 0\"/><circle cx=\"12\" cy=\"19.6\" r=\"1.1\" fill=\"currentColor\"/></svg>";
const ICON_SERVER: &str = "<svg viewBox=\"0 0 24 24\"><rect x=\"3.5\" y=\"4\" width=\"17\" height=\"6.5\" rx=\"2\"/><rect x=\"3.5\" y=\"13.5\" width=\"17\" height=\"6.5\" rx=\"2\"/><path d=\"M7.5 7.25h.01M7.5 16.75h.01M11 7.25h5.5M11 16.75h5.5\"/></svg>";
const ICON_FRAME: &str = "<svg viewBox=\"0 0 24 24\"><rect x=\"2.5\" y=\"4.5\" width=\"19\" height=\"13\" rx=\"2.2\"/><path d=\"M8.5 21h7M12 17.5V21\"/></svg>";
const ICON_CHEV: &str = "<svg viewBox=\"0 0 24 24\"><path d=\"M9 5l7 7-7 7\"/></svg>";
const ICON_BACK: &str = "<svg viewBox=\"0 0 24 24\"><path d=\"M15 5l-7 7 7 7\"/></svg>";
const ICON_RESTART: &str =
    "<svg viewBox=\"0 0 24 24\"><path d=\"M20 12a8 8 0 1 1-2.34-5.66M20 4v4.5h-4.5\"/></svg>";
const ICON_RESET: &str = "<svg viewBox=\"0 0 24 24\"><path d=\"M4 12a8 8 0 1 0 2.34-5.66M4 4v4.5h4.5\"/><path d=\"M12 8.5v4l2.5 1.5\"/></svg>";
const ICON_HAND: &str = "<svg viewBox=\"0 0 24 24\"><path d=\"M9 11V5.5a1.5 1.5 0 0 1 3 0V10m0-1.5a1.5 1.5 0 0 1 3 0V11m0-1a1.5 1.5 0 0 1 3 0v4.5a6.5 6.5 0 0 1-6.5 6.5h-.6a6 6 0 0 1-4.6-2.2L4.2 15a1.6 1.6 0 0 1 2.4-2.1L9 15\"/></svg>";

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;

    #[test]
    fn typed_addresses_become_urls() {
        assert_eq!(
            candidates(" nas.local ").unwrap(),
            vec![
                "http://nas.local:41007",
                "http://nas.local:8080",
                "http://nas.local"
            ]
        );
        assert_eq!(
            candidates("http://192.168.1.5:9000/frame/x").unwrap(),
            vec!["http://192.168.1.5:9000"]
        );
        assert_eq!(
            candidates("[fd00::5]:41007").unwrap(),
            vec!["http://[fd00::5]:41007"]
        );
        assert!(candidates("https://nas").is_err());
        assert!(candidates("nas:99999").is_err());
        assert!(candidates("").is_err());
        assert!(candidates("nas local").is_err());
        assert!(candidates("<script>").is_err());
    }

    struct Fake(AtomicBool);
    impl Radio for Fake {
        fn present(&self) -> bool {
            true
        }
        fn connected(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }
        fn scan(&self) -> Vec<Net> {
            vec![]
        }
        fn join(&self, _: &str, _: Option<&str>) -> Result<(), String> {
            Ok(())
        }
    }

    fn get(port: u16, path: &str, host: &str) -> (u16, String) {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(s, "GET {path} HTTP/1.1\r\nHost: {host}\r\n\r\n").unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        let code = out[9..12].parse().unwrap();
        (code, out)
    }

    #[test]
    fn answers_only_while_open_and_only_to_its_own_name() {
        let st: Shared = Arc::default();
        let (tx, rx) = mpsc::channel();
        let dir = std::env::temp_dir().join(format!("tf-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let conf = dir.join("thinframe.conf");
        std::fs::write(&conf, "FRAME_ID=\"den\"\n").unwrap();
        let radio: Arc<dyn Radio> = Arc::new(Fake(AtomicBool::new(true)));
        let port = start(
            st.clone(),
            radio,
            tx,
            conf.to_string_lossy().into(),
            "127.0.0.1:0",
        )
        .unwrap();
        let me = format!("127.0.0.1:{port}");
        assert_eq!(get(port, "/settings", &me).0, 404, "closed");
        open(&st);
        let (code, body) = get(port, "/settings", &me);
        assert_eq!(code, 200);
        assert!(body.contains("den") && body.contains("Not set"), "{body}");
        assert_eq!(get(port, "/settings", "evil.example").0, 404, "rebinding");
        assert_eq!(get(port, "/settings/server/edit", &me).0, 200);
        // Done asks the kiosk to close.
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(
            s,
            "POST /settings/close HTTP/1.1\r\nHost: {me}\r\nOrigin: http://{me}\r\nContent-Length: 0\r\n\r\n"
        )
        .unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        assert_eq!(rx.try_recv(), Ok(KioskCmd::CloseSettings));
        // Auto refreshes do not hold it open.
        lock(&st).last_used = Some(Instant::now() - IDLE);
        get(port, "/settings?auto=1", &me);
        assert!(idle(&st));
        get(port, "/settings", &me);
        assert!(!idle(&st));
        close(&st);
        assert_eq!(get(port, "/settings", &me).0, 404);
        // The dashboard's gear and the status page's button open it while closed.
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(
            s,
            "POST /settings/open HTTP/1.1\r\nHost: {me}\r\nContent-Length: 0\r\n\r\n"
        )
        .unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        assert!(out.starts_with("HTTP/1.1 303"), "{out}");
        assert_eq!(
            rx.try_recv(),
            Ok(KioskCmd::SettingsShown("/settings".into()))
        );
        assert_eq!(get(port, "/settings", &me).0, 200);
        // A form from another page (an app on the dashboard) cannot restart or reset.
        let post = |path: &str, origin: &str| {
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(
                s,
                "POST {path} HTTP/1.1\r\nHost: {me}\r\nOrigin: {origin}\r\nContent-Length: 0\r\n\r\n"
            )
            .unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).unwrap();
            out[9..12].parse::<u16>().unwrap()
        };
        assert_eq!(post("/settings/close", "http://server.nox:41007"), 404);
        assert_eq!(post("/settings/close", "null"), 404);
        assert_eq!(rx.try_recv(), Err(mpsc::TryRecvError::Empty));
        assert_eq!(post("/settings/close", &format!("http://{me}")), 200);
        assert_eq!(rx.try_recv(), Ok(KioskCmd::CloseSettings));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reset_forgets_wifi_server_and_pairing_and_keeps_the_rest() {
        let dir = std::env::temp_dir().join(format!("tf-reset-{}", std::process::id()));
        let iwd = dir.join("iwd");
        std::fs::create_dir_all(&iwd).unwrap();
        wifi::write_profile(&iwd, "Home", "secret123").unwrap();
        let conf = dir.join("thinframe.conf");
        std::fs::write(
            &conf,
            "FRAME_ID=\"den\"\nSERVER_URL=\"http://nas:8080\"\nFRAME_URL=\"http://nas:8080/frame/den\"\nCOG_ROTATION=\"3\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("thinframe.token"), "tok\n").unwrap();
        let forgot = Mutex::new(vec![]);
        reset(conf.to_str().unwrap(), &iwd, |s| {
            forgot.lock().unwrap().push(s.to_string())
        })
        .unwrap();
        assert_eq!(*forgot.lock().unwrap(), vec!["Home".to_string()]);
        let c = crate::config::load(conf.to_str().unwrap()).unwrap();
        assert_eq!((c.server_url.as_str(), c.token.as_str()), ("", ""));
        assert_eq!((c.frame_id.as_str(), c.rotation), ("den", 3));
        assert!(!dir.join("thinframe.token").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_saved_server_wakes_the_agent_and_rebases_the_page() {
        let st: Shared = Arc::default();
        let (tx, _rx) = mpsc::channel();
        let dir = std::env::temp_dir().join(format!("tf-settings-use-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let conf = dir.join("thinframe.conf");
        std::fs::write(
            &conf,
            "FRAME_ID=\"den\"\nSERVER_URL=\"http://nas:8080\"\nFRAME_URL=\"http://nas:8080/frame/den\"\n",
        )
        .unwrap();
        let cx = Ctx {
            st,
            radio: Arc::new(Fake(AtomicBool::new(true))),
            kiosk: tx,
            conf: conf.to_string_lossy().into(),
            port: 0,
        };
        LISTENING.store(true, Ordering::SeqCst);
        cx.use_server("http://10.0.0.5:41007/");
        let c = crate::config::load(&cx.conf).unwrap();
        assert_eq!(c.server_url, "http://10.0.0.5:41007");
        assert_eq!(c.url, "http://10.0.0.5:41007/frame/den");
        assert_eq!(take_server().as_deref(), Some("http://10.0.0.5:41007"));
        assert_eq!(take_server(), None);
        let t = Instant::now();
        nap(Duration::from_millis(300));
        assert!(t.elapsed() >= Duration::from_millis(300));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
