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
        if !host_ok || !lock(&self.st).active {
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
                    "<p class=\"dim\">Back in about a minute.</p>",
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
        let wifi_line = match (wifi::connected_ssid(), ips.first()) {
            (Some(n), Some(ip)) => format!("<span class=\"ok\">{}</span> &middot; {ip}", esc(&n)),
            (Some(n), None) => format!(
                "{} &middot; <span class=\"dim\">getting an address</span>",
                esc(&n)
            ),
            (None, Some(ip)) => format!("<span class=\"ok\">online</span> &middot; {ip}"),
            (None, None) => "<span class=\"bad\">not connected</span>".into(),
        };
        let server_line = if cfg.server_url.is_empty() {
            "<span class=\"bad\">not set</span>".to_string()
        } else if probe(&cfg.server_url, Duration::from_millis(1500)) {
            format!(
                "<span class=\"ok\">answering</span> &middot; {}",
                esc(&cfg.server_url)
            )
        } else {
            format!(
                "<span class=\"bad\">not answering</span> &middot; {}",
                esc(&cfg.server_url)
            )
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
            "{note}<div class=\"card\"><div class=\"t\"><b>Wi-Fi</b><div class=\"v\">{wifi_line}</div></div>\
             <a class=\"btn\" href=\"/settings/wifi\">Change</a></div>\
             <div class=\"card\"><div class=\"t\"><b>Server</b><div class=\"v\">{server_line}</div></div>\
             <a class=\"btn\" href=\"/settings/server\">Change</a></div>\
             <div class=\"card\"><div class=\"t\"><b>This frame</b><div class=\"v small\">{id} &middot; \
             software {sw} &middot; system {sys} (slot {slot}) &middot; up {up}</div></div>\
             <form method=\"post\" action=\"/settings/reboot\"><button class=\"btn2\">Restart</button></form></div>\
             <p class=\"dim small\">Hold the screen for 5 seconds on any page to come back here.</p>",
            note = self.take_note(),
            id = esc(&cfg.frame_id),
            sw = opt(&st.software_version),
            sys = opt(&st.system_version),
            slot = opt(&st.slot),
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
                "<span class=\"ok\">connected</span>"
            } else if n.security == "open" {
                "<span class=\"dim\">open</span>"
            } else {
                ""
            };
            if n.supported() {
                body.push_str(&format!(
                    "<li><a href=\"/settings/wifi/net?ssid={}&amp;sec={}\"><span class=\"name\">{}</span>{tag}\
                     <span class=\"dim\">{}</span></a></li>",
                    pct(&n.ssid),
                    pct(&n.security),
                    esc(&n.ssid),
                    "▮".repeat(n.bars() as usize)
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
html,body{margin:0;height:100%;overflow:hidden;background:#0b0d10;color:#e8e6e1;\
font:3.9vh P,'DejaVu Sans',sans-serif;user-select:none;-webkit-user-select:none;touch-action:manipulation}\
header{display:flex;align-items:center;gap:2vw;padding:3vh 4vw 2vh}h1{flex:1;margin:0;font-size:6.4vh;font-weight:400}\
main{padding:0 4vw;height:80vh;overflow-y:auto}\
.card{background:#161a20;border-radius:2.4vh;padding:2.8vh 2.8vw;margin:0 0 2.4vh;display:flex;align-items:center;gap:2vw}\
.card.col{display:block}.t{flex:1;min-width:0}\
b{display:block;font-weight:400;color:#8d96a0;font-size:2.8vh;letter-spacing:.08em;text-transform:uppercase;margin-bottom:.4vh}\
.v{font-size:4.2vh;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.small{font-size:3.2vh}\
.ok{color:#5cc98a}.bad{color:#f07a6a}.dim{color:#8d96a0}\
button,.btn{font:3.9vh P,'DejaVu Sans',sans-serif;color:#0b0d10;background:#f2b84b;border:0;border-radius:999px;\
padding:2.2vh 3.2vw;text-decoration:none;display:inline-block;white-space:nowrap}\
.btn2{background:#2a3038;color:#e8e6e1}button:disabled{opacity:.35}\
button:active,.btn:active{transform:scale(.96);opacity:.8}form{margin:0}\
.row{display:flex;gap:1.6vw;align-items:center}\
.note{border-radius:1.6vh;padding:1.6vh 2vw;margin:0 0 2vh}.good{background:#16352a}.badn{background:#3a1d1a}\
ul{list-style:none;margin:1vh 0 2vh;padding:0}li,li a{display:flex;align-items:center;gap:1.6vw}\
li{border-bottom:1px solid #232830}li:last-child{border:0}li a{flex:1;color:inherit;text-decoration:none;padding:2.4vh .6vw}\
li>.name{padding:1.4vh .6vw}.name{flex:1;overflow:hidden;white-space:nowrap;text-overflow:ellipsis}\
.top{display:flex;align-items:center;gap:2vw}\
.field{flex:1;font:4.2vh 'DejaVu Sans Mono',monospace;background:#161a20;border:1px solid #2a3038;\
border-radius:1.6vh;padding:.3em .6em;white-space:pre;overflow:hidden}.hint{color:#5b636c}\
#kb{margin:2vh 0 0}#kb div{display:flex;justify-content:center;gap:.6vw;margin-bottom:1.2vh}\
#kb button{font:4.2vh 'DejaVu Sans Mono',monospace;min-width:7.4vw;height:9.4vh;padding:0;border-radius:1.6vh;\
background:#1d2229;color:#e8e6e1}#kb button.k2{background:#2a3038;min-width:10.5vw;font:3vh P,sans-serif}\
#kb button.on{background:#f2b84b;color:#0b0d10}#kb button.sp{min-width:44vw;font:3vh P,sans-serif}";

/// A page: title, a back link (empty: none), the body, an optional self-refresh in seconds
/// (marked `auto` so it does not count as use).
fn page(title: &str, back: &str, body: &str, refresh: Option<u32>) -> String {
    let meta = refresh
        .map(|r| format!("<meta http-equiv=\"refresh\" content=\"{r};url=?auto=1\">"))
        .unwrap_or_default();
    let back = if back.is_empty() {
        String::new()
    } else {
        format!("<a class=\"btn btn2\" href=\"{back}\">Back</a>")
    };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">{meta}<title>Settings</title>\
         <style>{CSS}</style></head><body><header>{back}<h1>{}</h1>\
         <form method=\"post\" action=\"/settings/close\"><button>Done</button></form></header>\
         <main>{body}</main></body></html>",
        esc(title)
    )
}

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
        assert!(body.contains("den") && body.contains("not set"), "{body}");
        assert_eq!(get(port, "/settings", "evil.example").0, 404, "rebinding");
        assert_eq!(get(port, "/settings/server/edit", &me).0, 200);
        // Done asks the kiosk to close.
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(
            s,
            "POST /settings/close HTTP/1.1\r\nHost: {me}\r\nContent-Length: 0\r\n\r\n"
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
