//! On-screen Wi-Fi setup (TASK 17): Wi-Fi first, then pairing (pair.rs). When the frame has a
//! radio but no connection (nothing configured, or the configured network has not come up within
//! GRACE), cog shows a touch page served by this module on 127.0.0.1:PORT: the networks in range,
//! then a password page with an on-screen keyboard. The page posts back here; we join with iwctl
//! (wifi.rs). Once the station is connected the agent moves on to pairing or the dashboard.
//! The server binds loopback only, checks the Host header, and answers only while setup is on
//! screen, so a dashboard page can never change the network.
use crate::agent::Agent;
use crate::http::Url;
use crate::kiosk::KioskCmd;
use crate::pair::esc;
use crate::wifi::{self, Net, Radio};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub const PORT: u16 = 8091;
/// How long a configured network gets to come up before the setup page offers another.
pub const GRACE: Duration = Duration::from_secs(45);
const RESCAN: Duration = Duration::from_secs(20);
const STEP: Duration = Duration::from_secs(2);
/// While waiting for an address: one `ip` call each time, and the server is tried the moment
/// DHCP answers instead of up to 2 s later.
const WAIT_STEP: Duration = Duration::from_millis(400);
const FONT: &str = "/usr/share/fonts/thinframe/Shrikhand-Regular.ttf";
pub(crate) const POPPINS: &str = "/usr/share/fonts/thinframe/Poppins-Regular.ttf";
pub(crate) const POPPINS_LIGHT: &str = "/usr/share/fonts/thinframe/Poppins-ExtraLight.ttf";

#[derive(Default)]
pub struct Setup {
    /// On screen; the server answers 404 otherwise.
    pub active: bool,
    pub nets: Vec<Net>,
    pub scanned: Option<Instant>,
    /// A join is running for this network.
    pub joining: Option<String>,
    pub joined: Option<String>,
    /// The last network tried, and why it failed.
    pub last: Option<String>,
    /// The security of the row that was tapped for `last` ("psk", "open").
    pub last_sec: Option<String>,
    pub error: Option<String>,
    /// The configured network that did not come up.
    pub missing: Option<String>,
    pub frame_id: String,
    pub backdrop: Option<String>,
    /// `SPLASH_STYLE=dawn`: the frosted-sheet look (dawnpage.rs) instead of the seventies cards.
    pub dawn: bool,
}

pub type Shared = Arc<Mutex<Setup>>;

fn lock(st: &Shared) -> std::sync::MutexGuard<'_, Setup> {
    st.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the agent should do about Wi-Fi this round.
#[derive(Debug, PartialEq)]
pub enum Phase {
    /// No radio, or connected: carry on with pairing and the server.
    Online,
    /// A configured network is still coming up: wait, the status page says "joining".
    Waiting,
    /// Show the setup page.
    Setup,
}

pub fn phase(
    present: bool,
    connected: bool,
    configured: bool,
    down_for: Duration,
    page_up: bool,
) -> Phase {
    if !present || connected {
        Phase::Online
    } else if page_up || !configured || down_for >= GRACE {
        Phase::Setup
    } else {
        Phase::Waiting
    }
}

// ---- HTTP (loopback, one request per connection) ----

pub struct Req {
    pub method: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub host: String,
    /// The Origin header ("" when absent): which page sent a form.
    pub origin: String,
    pub form: Vec<(String, String)>,
}

pub struct Resp {
    pub status: u16,
    pub ctype: &'static str,
    pub body: Vec<u8>,
    pub location: Option<String>,
}

pub(crate) fn html(body: String) -> Resp {
    Resp {
        status: 200,
        ctype: "text/html; charset=utf-8",
        body: body.into_bytes(),
        location: None,
    }
}

pub(crate) fn see_other(to: &str) -> Resp {
    Resp {
        status: 303,
        ctype: "text/plain",
        body: Vec::new(),
        location: Some(to.into()),
    }
}

pub(crate) fn not_found() -> Resp {
    Resp {
        status: 404,
        ctype: "text/plain",
        body: b"not found".to_vec(),
        location: None,
    }
}

/// application/x-www-form-urlencoded and query strings.
pub fn form_decode(s: &str) -> Vec<(String, String)> {
    let dec = |v: &str| {
        let b = v.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                b'+' => out.push(b' '),
                b'%' if i + 2 < b.len() => {
                    match std::str::from_utf8(&b[i + 1..i + 3])
                        .ok()
                        .and_then(|h| u8::from_str_radix(h, 16).ok())
                    {
                        Some(x) => {
                            out.push(x);
                            i += 2;
                        }
                        None => out.push(b'%'),
                    }
                }
                c => out.push(c),
            }
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    };
    s.split('&')
        .filter(|kv| !kv.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (dec(k), dec(v))
        })
        .collect()
}

/// Percent-encode for a query value.
pub fn pct(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

pub(crate) fn field<'a>(kv: &'a [(String, String)], k: &str) -> Option<&'a str> {
    kv.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str())
}

/// The scanned network `ssid` with security `sec`; without `sec`, a secured entry wins over an
/// open one of the same name, so a typed password is never thrown away.
pub fn find_net(nets: &[Net], ssid: &str, sec: Option<&str>) -> Option<Net> {
    let mut same = nets.iter().filter(|n| n.ssid == ssid);
    match sec {
        Some(sec) => same.find(|n| n.security == sec),
        None => {
            let all: Vec<&Net> = same.collect();
            all.iter()
                .find(|n| n.security != "open")
                .or(all.first())
                .copied()
        }
    }
    .cloned()
}

/// Request line plus headers, in bytes.
const MAX_HEAD: u64 = 8192;
const MAX_HEADERS: usize = 50;
/// The whole request, however slowly it trickles in.
const REQ_DEADLINE: Duration = Duration::from_secs(5);

/// A socket whose reads all end by one deadline: each read gets only the time that is left.
struct Deadline<'a> {
    s: &'a TcpStream,
    end: Instant,
}

impl Read for Deadline<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "request too slow"));
        }
        self.s.set_read_timeout(Some(left))?;
        let mut s = self.s;
        s.read(buf)
    }
}

pub(crate) fn read_req(s: &mut TcpStream) -> io::Result<Req> {
    read_req_within(s, REQ_DEADLINE)
}

fn read_req_within(s: &TcpStream, limit: Duration) -> io::Result<Req> {
    let too_big = || io::Error::new(io::ErrorKind::InvalidData, "request head too large");
    let mut r = BufReader::new(Deadline {
        s,
        end: Instant::now() + limit,
    });
    let mut head = (&mut r).take(MAX_HEAD);
    let mut line = String::new();
    head.read_line(&mut line)?;
    if !line.ends_with('\n') {
        return Err(too_big());
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let (mut host, mut origin, mut len) = (String::new(), String::new(), 0usize);
    let mut count = 0;
    loop {
        let mut h = String::new();
        head.read_line(&mut h)?;
        if head.limit() == 0 && !h.ends_with('\n') {
            return Err(too_big());
        }
        if h.trim().is_empty() {
            break;
        }
        count += 1;
        if count > MAX_HEADERS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "too many headers",
            ));
        }
        if let Some((k, v)) = h.split_once(':') {
            match k.trim().to_ascii_lowercase().as_str() {
                "host" => host = v.trim().to_string(),
                "origin" => origin = v.trim().to_string(),
                "content-length" => len = v.trim().parse().unwrap_or(0),
                _ => {}
            }
        }
    }
    if len > 4096 {
        return Err(io::Error::other("body too large"));
    }
    let mut body = vec![0; len];
    r.read_exact(&mut body)?;
    let (path, q) = target.split_once('?').unwrap_or((&target, ""));
    Ok(Req {
        method,
        path: path.to_string(),
        query: form_decode(q),
        host,
        origin,
        form: form_decode(&String::from_utf8_lossy(&body)),
    })
}

pub(crate) fn write_resp(s: &mut TcpStream, r: &Resp) -> io::Result<()> {
    let reason = match r.status {
        200 => "OK",
        303 => "See Other",
        400 => "Bad Request",
        _ => "Not Found",
    };
    let mut head = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n",
        r.status,
        r.ctype,
        r.body.len()
    );
    if let Some(l) = &r.location {
        head.push_str(&format!("Location: {l}\r\n"));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes())?;
    s.write_all(&r.body)
}

/// Bind `addr` (127.0.0.1:PORT on the frame, :0 in tests) and serve in a thread; returns the port.
pub fn start(st: Shared, radio: Arc<dyn Radio>, addr: &str) -> io::Result<u16> {
    let l = TcpListener::bind(addr)?;
    let port = l.local_addr()?.port();
    thread::Builder::new()
        .name("wifisetup".into())
        .spawn(move || {
            for mut s in l.incoming().flatten() {
                let resp = match read_req(&mut s) {
                    Ok(req) => respond(&st, &radio, port, &req),
                    Err(_) => continue,
                };
                let _ = write_resp(&mut s, &resp);
            }
        })?;
    Ok(port)
}

pub fn respond(st: &Shared, radio: &Arc<dyn Radio>, port: u16, req: &Req) -> Resp {
    // DNS rebinding: only our own name for loopback.
    let host_ok = [format!("127.0.0.1:{port}"), format!("localhost:{port}")].contains(&req.host);
    if !host_ok || !lock(st).active {
        return not_found();
    }
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/wifi") => {
            let mut s = lock(st);
            if field(&req.query, "list").is_some() {
                s.error = None;
            }
            html(match (&s.joining, &s.joined, &s.error, &s.last) {
                (Some(j), _, _, _) => render_joining(&s, j),
                (_, Some(j), _, _) => render_joined(&s, j),
                (_, _, Some(_), Some(l)) => {
                    let net = find_net(&s.nets, l, s.last_sec.as_deref());
                    render_password(&s, l, net.as_ref())
                }
                _ => render_list(&s),
            })
        }
        ("GET", "/wifi/net") => {
            let Some(ssid) = field(&req.query, "ssid") else {
                return see_other("/wifi");
            };
            let mut s = lock(st);
            s.error = None;
            let net = find_net(&s.nets, ssid, field(&req.query, "sec"));
            html(render_password(&s, ssid, net.as_ref()))
        }
        ("POST", "/wifi/scan") => {
            lock(st).scanned = None;
            see_other("/wifi")
        }
        ("POST", "/wifi/join") => {
            let ssid = field(&req.form, "ssid").unwrap_or("").to_string();
            let psk = field(&req.form, "psk").unwrap_or("").to_string();
            let mut s = lock(st);
            // The tapped row's security comes with the form; an old form without it falls back
            // to the scan, where a secured entry wins. A typed password always means psk.
            let sec = match field(&req.form, "sec") {
                Some(v) => Some(v.to_string()),
                None => find_net(&s.nets, &ssid, None).map(|n| n.security),
            };
            let open = psk.is_empty() && sec.as_deref() == Some("open");
            s.last = Some(ssid.clone());
            s.last_sec = Some(if open { "open" } else { "psk" }.into());
            if !wifi::valid_ssid(&ssid) {
                s.error = Some("Pick a network first.".into());
                return see_other("/wifi");
            }
            if !open && !wifi::valid_psk(&psk) {
                s.error = Some("Wi-Fi passwords are 8 to 63 characters.".into());
                return see_other("/wifi");
            }
            if s.joining.is_some() {
                return see_other("/wifi");
            }
            s.joining = Some(ssid.clone());
            s.error = None;
            drop(s);
            let (st, radio) = (st.clone(), radio.clone());
            let _ = thread::Builder::new()
                .name("wifijoin".into())
                .spawn(move || {
                    let r = radio.join(&ssid, (!open).then_some(psk.as_str()));
                    let mut s = lock(&st);
                    s.joining = None;
                    match r {
                        Ok(()) => {
                            eprintln!("tfctl: wifi setup: joined {ssid}");
                            s.joined = Some(ssid);
                        }
                        Err(e) => {
                            eprintln!("tfctl: wifi setup: join {ssid}: {e}");
                            s.error = Some(format!("That didn't work ({e}). Check the password."));
                        }
                    }
                });
            see_other("/wifi")
        }
        ("GET", "/font.ttf") => file(FONT, "font/ttf"),
        ("GET", "/poppins.ttf") => file(POPPINS, "font/ttf"),
        ("GET", "/poppins-light.ttf") => file(POPPINS_LIGHT, "font/ttf"),
        ("GET", "/backdrop.png") => match lock(st).backdrop.clone() {
            Some(p) => file(&p, "image/png"),
            None => not_found(),
        },
        _ => not_found(),
    }
}

pub(crate) fn file(path: &str, ctype: &'static str) -> Resp {
    match std::fs::read(path) {
        Ok(body) => Resp {
            status: 200,
            ctype,
            body,
            location: None,
        },
        Err(_) => not_found(),
    }
}

// ---- pages (1280x800 landscape after cog's rotation; sized in vh/vw like pair.rs) ----

const CSS: &str = "@font-face{font-family:Shrikhand;src:url(/font.ttf)}\
html,body{margin:0;height:100%;overflow:hidden;background:#f4dcaa BG;\
user-select:none;-webkit-user-select:none;touch-action:manipulation}\
body{color:#5b3a1e;font:3.4vh Shrikhand,Georgia,serif}\
.card{position:absolute;left:4vw;top:7vh;width:56vw;max-height:80vh;box-sizing:border-box;padding:2.2vh 2.4vw 2.6vh;\
background:#f4dcaa;border:.6vh solid #5b3a1e;border-radius:3vh;box-shadow:1.2vh 1.2vh 0 #b8452a}\
.wide{width:92vw;top:4vh;max-height:none}.low{top:40vh;width:54vw}\
h1{margin:0 0 .2em;font-size:6.4vh;font-weight:400;color:#b8452a;line-height:1.1}p{margin:.2em 0 .6em}\
.steps{position:absolute;right:3vw;top:2.4vh;font-size:3vh;background:#f4dcaa;border:.4vh solid #5b3a1e;\
border-radius:999px;padding:.1em .8em}.steps b{color:#b8452a;font-weight:400}\
.note{font:2.8vh 'DejaVu Sans',sans-serif}\
.err{color:#f4dcaa;background:#b8452a;border-radius:1.5vh;padding:.3em .7em;font:2.8vh 'DejaVu Sans',sans-serif}\
ul{list-style:none;margin:0 0 1.4vh;padding:0;max-height:52vh;overflow-y:auto}\
li a{display:flex;align-items:center;gap:1.2vw;color:inherit;text-decoration:none;padding:1.4vh 1vw;\
border-bottom:.35vh dashed #d9662b}li:last-child a{border:0}li a:active{background:#eecb8d}\
.name{flex:1;overflow:hidden;white-space:nowrap;text-overflow:ellipsis}.dim{opacity:.45}\
.sig{display:flex;align-items:flex-end;gap:.25vw;height:3.4vh}.sig i{width:.6vw;background:#5b3a1e;opacity:.25}\
.sig i.on{opacity:1}.sig i:nth-child(1){height:25%}.sig i:nth-child(2){height:50%}.sig i:nth-child(3){height:75%}\
.sig i:nth-child(4){height:100%}.lock{font:2.6vh 'DejaVu Sans',sans-serif;opacity:.7}.lock svg{height:3vh;display:block}\
button,.btn{font:4.6vh Shrikhand,Georgia,serif;color:#f4dcaa;background:#d9662b;border:0;border-radius:999px;\
padding:.1em 1.1em;box-shadow:0 .9vh 0 #5b3a1e;text-decoration:none;display:inline-block}\
button:active,.btn:active{transform:translateY(.7vh);box-shadow:0 .2vh 0 #5b3a1e}\
button:disabled{opacity:.4}.ghost{background:#eecb8d;color:#5b3a1e}\
small{position:absolute;left:4.8vw;bottom:3vh;font-size:3vh}\
.dots span{animation:b 1.2s infinite}.dots span:nth-child(2){animation-delay:.2s}.dots span:nth-child(3){animation-delay:.4s}\
@keyframes b{0%,60%,100%{opacity:.2}30%{opacity:1}}";

fn page(s: &Setup, step: &str, body: &str, refresh: Option<u32>, script: &str) -> String {
    let bg = if s.backdrop.is_some() {
        "url(/backdrop.png) center/cover no-repeat"
    } else {
        ""
    };
    let meta = refresh
        .map(|r| format!("<meta http-equiv=\"refresh\" content=\"{r};url=/wifi\">"))
        .unwrap_or_default();
    let css = if s.dawn {
        crate::dawnpage::css("/poppins.ttf", "/poppins-light.ttf", bg)
    } else {
        CSS.replace("BG", bg)
    };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">{meta}<title>thinframe</title>\
         <style>{css}</style></head><body><div class=\"steps\">{step}</div>{body}\
         <small>frame {} &middot; hold the screen 5 s for settings</small>{script}</body></html>",
        esc(&s.frame_id)
    )
}

/// A padlock drawn in SVG: the frame has no emoji font.
const LOCK: &str = "<svg viewBox=\"0 0 10 12\"><rect x=\"1\" y=\"5\" width=\"8\" height=\"7\" rx=\"1\" fill=\"#5b3a1e\"/>\
<path d=\"M3 5V3.5a2 2 0 0 1 4 0V5\" fill=\"none\" stroke=\"#5b3a1e\" stroke-width=\"1.3\"/></svg>";

const STEP1: &str = "<b>1 Wi-Fi</b> &middot; 2 pair";

fn bars(n: &Net) -> String {
    let b = n.bars();
    let i: String = (1..=4)
        .map(|k| {
            if k <= b {
                "<i class=\"on\"></i>"
            } else {
                "<i></i>"
            }
        })
        .collect();
    format!("<span class=\"sig\">{i}</span>")
}

pub fn render_list(s: &Setup) -> String {
    let mut body = String::from("<main class=\"card\"><h1>Let's get online</h1>");
    match (&s.missing, &s.error) {
        (_, Some(e)) => body.push_str(&format!("<p class=\"err\">{}</p>", esc(e))),
        (Some(m), _) => body.push_str(&format!(
            "<p class=\"err\">Can't reach {}. Pick a network:</p>",
            esc(m)
        )),
        _ => body.push_str("<p>Tap your Wi-Fi network.</p>"),
    }
    if s.nets.is_empty() {
        body.push_str(
            "<p class=\"dots\">Looking for networks<span>.</span><span>.</span><span>.</span></p>",
        );
    } else {
        body.push_str("<ul>");
        for n in s.nets.iter().take(12) {
            let lock = if n.security == "open" { "" } else { LOCK };
            if n.supported() {
                body.push_str(&format!(
                    "<li><a href=\"/wifi/net?ssid={}&amp;sec={}\"><span class=\"name\">{}</span>\
                     <span class=\"lock\">{lock}</span>{}</a></li>",
                    pct(&n.ssid),
                    pct(&n.security),
                    esc(&n.ssid),
                    bars(n)
                ));
            } else {
                body.push_str(&format!(
                    "<li><a class=\"dim\"><span class=\"name\">{}</span>\
                     <span class=\"lock\">{}</span>{}</a></li>",
                    esc(&n.ssid),
                    esc(&n.security),
                    bars(n)
                ));
            }
        }
        body.push_str("</ul>");
    }
    body.push_str(
        "<form method=\"post\" action=\"/wifi/scan\"><button class=\"ghost\">Scan again</button></form></main>",
    );
    // Reload to show scan results: often while still looking, rarely once there is a list, so a
    // slow panel never swallows a tap in a reload.
    let refresh = if s.nets.is_empty() { 3 } else { 20 };
    page(s, STEP1, &body, Some(refresh), "")
}

/// The keyboard: four layers, special keys by name. Small on purpose (no library). Every key is
/// printable ASCII (WPA passphrases are nothing else), so 63 UTF-16 units here are 63 bytes.
pub(crate) const KEYBOARD_JS: &str = r##"<script>
var L={a:["qwertyuiop","asdfghjkl","zxcvbnm"],A:["QWERTYUIOP","ASDFGHJKL","ZXCVBNM"],
n:["1234567890","-/:;()$&@\"",".,?!'"],s:["[]{}#%^*+=","_\\|~<>`",".,?!'"]};
var layer="a",pw="",shown=true,kb=document.getElementById("kb"),out=document.getElementById("pw"),
go=document.getElementById("go"),min=+kb.dataset.min;
function key(label,cls,fn){var b=document.createElement("button");b.type="button";b.textContent=label;
if(cls)b.className=cls;b.onclick=fn;return b}
function draw(){kb.innerHTML="";var rows=L[layer];
rows.forEach(function(r,i){var d=document.createElement("div");
if(i==2){if(layer=="a"||layer=="A")d.appendChild(key("⇧","k2"+(layer=="A"?" on":""),function(){layer=layer=="a"?"A":"a";draw()}));
else d.appendChild(key(layer=="n"?"#+=":"123","k2",function(){layer=layer=="n"?"s":"n";draw()}))}
r.split("").forEach(function(c){d.appendChild(key(c,"",function(){if(pw.length<63)pw+=c;show()}))});
if(i==2)d.appendChild(key("⌫","k2",function(){pw=pw.slice(0,-1);show()}));kb.appendChild(d)});
var d=document.createElement("div");
d.appendChild(key(layer=="a"||layer=="A"?"123":"abc","k2",function(){layer=layer=="a"||layer=="A"?"n":"a";draw()}));
d.appendChild(key("space","sp",function(){if(pw.length<63)pw+=" ";show()}));
d.appendChild(key(shown?"hide":"show","k2",function(){shown=!shown;show();draw()}));kb.appendChild(d)}
function show(){out.className=pw.length?"field":"field hint";out.textContent=pw.length?(shown?pw:pw.replace(/./g,"•")):(out.dataset.hint||"password");
document.getElementById("psk").value=pw;go.disabled=pw.length<min}
draw();show();
</script>"##;

pub fn render_password(s: &Setup, ssid: &str, net: Option<&Net>) -> String {
    let open = net.is_some_and(|n| n.security == "open");
    let err = s
        .error
        .as_deref()
        .map(|e| format!("<p class=\"err\">{}</p>", esc(e)))
        .unwrap_or_default();
    let form_head = format!(
        "<form method=\"post\" action=\"/wifi/join\" class=\"row\">\
         <input type=\"hidden\" name=\"ssid\" value=\"{}\"><input type=\"hidden\" name=\"sec\" value=\"{}\">\
         <input type=\"hidden\" name=\"psk\" id=\"psk\">\
         <a class=\"btn ghost\" href=\"/wifi?list\">back</a>",
        esc(ssid),
        if open { "open" } else { "psk" }
    );
    if open {
        let body = format!(
            "<main class=\"card\"><h1>{}</h1>{err}<p>An open network: no password.</p>\
             {form_head} <button>Join</button></form></main>",
            esc(ssid)
        );
        return page(s, STEP1, &body, None, "");
    }
    let seventies = "<style>.wide h1{font-size:5vh}.top{display:flex;align-items:center;gap:2vw}\
        .field{flex:1;font:4.4vh 'DejaVu Sans Mono',monospace;background:#fff6e0;border:.4vh solid #5b3a1e;\
        border-radius:1.5vh;padding:.25em .5em;white-space:pre;overflow:hidden}\
        .row{display:flex;gap:1.5vw;align-items:center;margin:0}\
        #kb{margin:1.6vh 0 0}#kb div{display:flex;justify-content:center;gap:.6vw;margin-bottom:1.2vh}\
        #kb button{font:4.4vh 'DejaVu Sans Mono',monospace;min-width:7.6vw;height:9.6vh;padding:0;border-radius:1.6vh;\
        background:#fff6e0;color:#5b3a1e;box-shadow:0 .6vh 0 #b8452a}\
        #kb button.k2{background:#eecb8d;min-width:10.5vw;font:3.4vh Shrikhand,Georgia,serif}\
        #kb button.on{background:#d9662b;color:#f4dcaa}#kb button.sp{min-width:44vw;font:3.4vh Shrikhand,Georgia,serif}\
        small,.steps{display:none}.hint{opacity:.35}</style>";
    let css = if s.dawn {
        format!("<style>{}</style>", crate::dawnpage::KEYBOARD_CSS)
    } else {
        seventies.to_string()
    };
    let body = format!(
        "{css}<main class=\"card wide\"><div class=\"top\"><h1>{}</h1>{err}</div>\
         <div class=\"top\"><div class=\"field\" id=\"pw\"></div>{form_head}\
         <button id=\"go\" disabled>Join</button></form></div>\
         <div id=\"kb\" data-min=\"8\"></div></main>",
        esc(ssid)
    );
    page(s, STEP1, &body, None, KEYBOARD_JS)
}

fn render_joining(s: &Setup, ssid: &str) -> String {
    let body = format!(
        "<main class=\"card low\"><h1 class=\"dots\">Joining<span>.</span><span>.</span><span>.</span></h1>\
         <p>{}</p><p class=\"note\">This can take half a minute.</p></main>",
        esc(ssid)
    );
    page(s, STEP1, &body, Some(2), "")
}

fn render_joined(s: &Setup, ssid: &str) -> String {
    let body = format!(
        "<main class=\"card low\"><h1>Far out!</h1><p>Connected to {}.</p>\
         <p class=\"dots\">Next: pairing<span>.</span><span>.</span><span>.</span></p></main>",
        esc(ssid)
    );
    page(s, "1 Wi-Fi &middot; <b>2 pair</b>", &body, Some(3), "")
}

// ---- the agent's side ----

/// iwd gives a network a few tries, then backs off for a minute or more: at boot the frame's
/// first four joins failed within 1.3 s and the fifth came 73 s later [V 2026-10-22]. While a
/// saved network is down, the agent asks for it again itself, KICK_AFTER into the outage and
/// then every KICK_EVERY, unless the setup page is joining one the person picked.
const KICK_AFTER: Duration = Duration::from_secs(8);
const KICK_EVERY: Duration = Duration::from_secs(20);

pub fn kick_due(down_for: Duration, since_kick: Option<Duration>, joining: bool) -> bool {
    !joining && down_for >= KICK_AFTER && since_kick.is_none_or(|t| t >= KICK_EVERY)
}

const DIRECT_EVERY: Duration = Duration::from_secs(20);
/// Wi-Fi down this long: look for the server over the USB-C link too, at most every CABLE_EVERY.
const CABLE_AFTER: Duration = Duration::from_secs(20);
const CABLE_EVERY: Duration = Duration::from_secs(30);
/// A probe that failed is tried again sooner: at boot the computer takes a few seconds to bring
/// its end of the cable up, and the frame waited a whole DIRECT_EVERY for it [V 2026-10-22].
const DIRECT_RETRY: Duration = Duration::from_secs(2);

fn direct_probe(host: &str, port: u16) -> bool {
    use std::net::{IpAddr, SocketAddr};
    let Ok(ip) = host.parse::<IpAddr>() else {
        return false; // a name needs DNS, which needs a network: not the cable case
    };
    if ip.is_loopback() {
        return false;
    }
    TcpStream::connect_timeout(&SocketAddr::new(ip, port), Duration::from_secs(1)).is_ok()
}

impl Agent {
    /// Wi-Fi comes first: true while the agent must not try the server yet.
    pub(crate) fn needs_wifi(&mut self) -> bool {
        let radio = self.radio.clone();
        if !radio.present() {
            return false;
        }
        let connected = self.kick_wifi();
        if !connected {
            self.find_server_on_cable();
        }
        if !connected && self.server_direct() {
            if self.wifi_page_up {
                self.leave_wifi_setup();
            }
            return false;
        }
        let down_for = self.wifi_down_since.map_or(Duration::ZERO, |t| t.elapsed());
        let configured = wifi::configured_ssid(Path::new(wifi::IWD_DIR));
        match phase(
            true,
            connected,
            configured.is_some(),
            down_for,
            self.wifi_page_up,
        ) {
            Phase::Online => {
                if self.wifi_page_up {
                    self.leave_wifi_setup();
                }
                false
            }
            Phase::Waiting => {
                self.wifi_setup = false;
                true
            }
            Phase::Setup => {
                self.wifi_setup = true;
                if let Some(ui) = &self.wifi_ui {
                    lock(ui).missing = configured.filter(|_| down_for >= GRACE);
                }
                true
            }
        }
    }

    /// Whether the station is up, tracking since when it is down, and asking iwd to rejoin the
    /// saved network when `kick_due`. Also called while the server is reached over the cable,
    /// where nothing else waits for Wi-Fi.
    pub(crate) fn kick_wifi(&mut self) -> bool {
        let radio = self.radio.clone();
        if !radio.present() {
            return false;
        }
        if radio.connected() {
            self.wifi_down_since = None;
            return true;
        }
        let down_for = self
            .wifi_down_since
            .get_or_insert_with(Instant::now)
            .elapsed();
        if wifi::configured_ssid(Path::new(wifi::IWD_DIR)).is_none() {
            return false;
        }
        let joining = self
            .wifi_ui
            .as_ref()
            .is_some_and(|ui| lock(ui).joining.is_some());
        if !kick_due(down_for, self.wifi_kicked.map(|t| t.elapsed()), joining) {
            return false;
        }
        self.wifi_kicked = Some(Instant::now());
        eprintln!(
            "tfctl: wifi: down for {}s, asking iwd to rejoin",
            down_for.as_secs()
        );
        if let Err(e) = radio.reconnect() {
            eprintln!("tfctl: wifi: rejoin: {e}");
        }
        false
    }

    /// Wi-Fi down for a while and the server on a network address: ask over the USB-C link too,
    /// and move there when the server answers (`leave_usb_link` moves back once Wi-Fi is up).
    /// Without it a frame whose Wi-Fi broke waited for Wi-Fi with the cable in [V 2026-10-22].
    fn find_server_on_cable(&mut self) {
        let on_usb = self.base.host.parse().is_ok_and(crate::discover::is_usb);
        let down = self.wifi_down_since.map_or(Duration::ZERO, |t| t.elapsed());
        if on_usb
            || down < CABLE_AFTER
            || self.cable_asked.is_some_and(|t| t.elapsed() < CABLE_EVERY)
        {
            return;
        }
        self.cable_asked = Some(Instant::now());
        let Ok(nonce) = crate::pair::nonce() else {
            return;
        };
        let token = Some(self.cfg.token.as_str()).filter(|t| !t.is_empty());
        let who = crate::discover::Asker {
            id: &self.cfg.frame_id,
            token,
        };
        let usb = [crate::discover::targets()[1]];
        if let Ok(Some(url)) = crate::discover::find(&usb, crate::discover::WAIT, &who, &nonce) {
            if Url::parse(&url)
                .and_then(|u| u.host.parse().ok())
                .is_some_and(crate::discover::is_usb)
            {
                self.move_server(&url);
            }
        }
    }

    /// One round of waiting for Wi-Fi; returns how long to sleep.
    pub(crate) fn wifi_step(&mut self) -> Duration {
        if !self.wifi_setup {
            let due = self.status_written.is_none_or(|t| t.elapsed() >= STEP);
            if self.on_fallback && due {
                self.status_written = Some(Instant::now());
                self.write_status_page();
            }
            return WAIT_STEP;
        }
        let ui = match &self.wifi_ui {
            Some(ui) => ui.clone(),
            None => {
                let ui: Shared = Arc::default();
                if let Err(e) = start(ui.clone(), self.radio.clone(), &format!("127.0.0.1:{PORT}"))
                {
                    eprintln!("tfctl: wifi setup: listen on {PORT}: {e}");
                    return Duration::from_secs(10);
                }
                self.wifi_ui = Some(ui.clone());
                ui
            }
        };
        let rescan = {
            let s = lock(&ui);
            s.joining.is_none() && s.scanned.is_none_or(|t| t.elapsed() >= RESCAN)
        };
        if rescan {
            let nets = self.radio.scan();
            let mut s = lock(&ui);
            if !nets.is_empty() || s.nets.is_empty() {
                s.nets = nets;
            }
            s.scanned = Some(Instant::now());
        }
        if !self.wifi_page_up {
            let bg = crate::pair::page_path().with_file_name("backdrop-page.png");
            let backdrop = crate::splash::write_page_backdrop(&self.cfg, &bg.to_string_lossy())
                .ok()
                .flatten();
            {
                let mut s = lock(&ui);
                s.active = true;
                s.frame_id = self.cfg.frame_id.clone();
                s.backdrop = backdrop;
                s.dawn = self.cfg.splash_style == "dawn";
                s.joined = None;
            }
            eprintln!("tfctl: wifi setup: on screen");
            self.event("wifi", "showing the Wi-Fi setup page");
            self.wifi_page_up = true;
            self.on_fallback = true;
            let _ = self.send(KioskCmd::SetUrl(format!("http://127.0.0.1:{PORT}/wifi")));
        }
        STEP
    }

    /// The server answers without Wi-Fi: the frame is on the USB-C cable to the computer that
    /// runs it (SERVER_URL=http://192.168.7.1:...). Then Wi-Fi must not stand between the frame
    /// and its server, or a frame whose Wi-Fi fails cannot be fixed remotely. One TCP connect,
    /// at most every DIRECT_EVERY. A loopback server (tests, the sim) never counts.
    fn server_direct(&mut self) -> bool {
        if let Some((t, ok)) = self.direct {
            if t.elapsed() < if ok { DIRECT_EVERY } else { DIRECT_RETRY } {
                return ok;
            }
        }
        let ok = direct_probe(&self.base.host, self.base.port);
        if ok && self.direct.is_none_or(|(_, was)| !was) {
            eprintln!(
                "tfctl: agent: server reachable without Wi-Fi (USB link), not waiting for Wi-Fi"
            );
        }
        self.direct = Some((Instant::now(), ok));
        ok
    }

    fn leave_wifi_setup(&mut self) {
        eprintln!("tfctl: wifi setup: connected, moving on");
        self.event("wifi", "Wi-Fi connected from the setup page");
        if let Some(ui) = &self.wifi_ui {
            let mut s = lock(ui);
            s.active = false;
            s.error = None;
            s.last = None;
        }
        self.wifi_page_up = false;
        self.wifi_setup = false;
        // Back to the status page ("reaching the server"); pairing or the dashboard follows.
        self.write_status_page();
        if !self.cfg.fallback_url.is_empty() {
            let _ = self.send(KioskCmd::SetUrl(self.cfg.fallback_url.clone()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_probe_ignores_loopback_and_names() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        assert!(
            !direct_probe("127.0.0.1", port),
            "a loopback server is the sim, not a cable"
        );
        assert!(!direct_probe("nas.local", port));
    }
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Fake {
        up: AtomicBool,
        psk: &'static str,
    }

    impl Radio for Fake {
        fn present(&self) -> bool {
            true
        }
        fn connected(&self) -> bool {
            self.up.load(Ordering::SeqCst)
        }
        fn scan(&self) -> Vec<Net> {
            vec![]
        }
        fn join(&self, _ssid: &str, psk: Option<&str>) -> Result<(), String> {
            if psk == Some(self.psk) || psk.is_none() {
                self.up.store(true, Ordering::SeqCst);
                Ok(())
            } else {
                Err("Operation failed".into())
            }
        }
    }

    fn net(ssid: &str, security: &str) -> Net {
        Net {
            ssid: ssid.into(),
            security: security.into(),
            dbm: Some(-50),
            current: false,
        }
    }

    #[test]
    fn wifi_comes_before_everything_else() {
        let g = GRACE;
        let z = Duration::ZERO;
        assert_eq!(
            phase(false, false, false, g, false),
            Phase::Online,
            "no radio"
        );
        assert_eq!(phase(true, true, true, z, true), Phase::Online);
        assert_eq!(
            phase(true, false, false, z, false),
            Phase::Setup,
            "nothing configured"
        );
        assert_eq!(
            phase(true, false, true, z, false),
            Phase::Waiting,
            "give iwd a moment"
        );
        assert_eq!(
            phase(true, false, true, g, false),
            Phase::Setup,
            "configured one is gone"
        );
        assert_eq!(
            phase(true, false, true, z, true),
            Phase::Setup,
            "page stays up"
        );
    }

    #[test]
    fn decodes_forms() {
        let f = form_decode("ssid=Groovy+Wifi&psk=far%20out%21%2B&x");
        assert_eq!(field(&f, "ssid"), Some("Groovy Wifi"));
        assert_eq!(field(&f, "psk"), Some("far out!+"));
        assert_eq!(field(&f, "x"), Some(""));
        assert_eq!(form_decode("a=%zz%4")[0].1, "%zz%4");
        assert_eq!(form_decode("a=caf%C3%A9")[0].1, "café");
        assert_eq!(pct("Café & co"), "Caf%C3%A9%20%26%20co");
        assert_eq!(
            form_decode(&format!("s={}", pct("a+b c/é")))[0].1,
            "a+b c/é"
        );
    }

    #[test]
    fn pages_escape_and_link() {
        let s = Setup {
            nets: vec![
                net("<Home>", "psk"),
                net("Cafe", "open"),
                net("Corp", "8021x"),
            ],
            frame_id: "kitchen".into(),
            ..Setup::default()
        };
        let h = render_list(&s);
        assert!(h.contains("href=\"/wifi/net?ssid=%3CHome%3E&amp;sec=psk\""));
        assert!(h.contains("&lt;Home&gt;") && !h.contains("<Home>"));
        assert!(
            h.contains("class=\"dim\"><span class=\"name\">Corp"),
            "8021x listed, not tappable"
        );
        assert!(h.contains("<b>1 Wi-Fi</b>"));
        let p = render_password(&s, "<Home>", s.nets.first());
        assert!(p.contains("name=\"ssid\" value=\"&lt;Home&gt;\""));
        assert!(p.contains("id=\"kb\""));
        let o = render_password(&s, "Cafe", s.nets.get(1));
        assert!(!o.contains("id=\"kb\"") && o.contains("no password"));
    }

    #[test]
    fn dawn_pages_use_the_sheet_look_and_keep_the_same_markup() {
        let s = Setup {
            nets: vec![net("Home", "psk")],
            frame_id: "kitchen".into(),
            backdrop: Some("/run/thinframe/backdrop-page.png".into()),
            dawn: true,
            ..Setup::default()
        };
        let l = render_list(&s);
        assert!(l.contains("/poppins-light.ttf") && !l.contains("Shrikhand"));
        assert!(
            l.contains("url(/backdrop.png)")
                && l.contains("href=\"/wifi/net?ssid=Home&amp;sec=psk\"")
        );
        let p = render_password(&s, "Home", s.nets.first());
        assert!(p.contains("id=\"kb\"") && p.contains("rgba(255,255,255,.66)"));
        assert!(!p.contains("#fff6e0"), "no seventies keys");
        let old = render_list(&Setup { dawn: false, ..s });
        assert!(old.contains("Shrikhand") && !old.contains("/poppins.ttf"));
    }

    /// `TF_PREVIEW=/dir cargo test preview_pages -- --ignored`: the setup pages as HTML files,
    /// for a browser. Fonts and the backdrop are read from the paths in `TF_PREVIEW_FONTS`
    /// (a directory holding the Poppins files) and `/dir/backdrop-page.png`.
    #[test]
    #[ignore]
    fn preview_pages() {
        let dir = std::env::var("TF_PREVIEW").expect("TF_PREVIEW=/dir");
        let fonts = std::env::var("TF_PREVIEW_FONTS").unwrap_or_default();
        let s = Setup {
            nets: vec![
                net("Groovy Wifi", "psk"),
                net("Cafe", "open"),
                net("Corp", "8021x"),
            ],
            frame_id: "kitchen".into(),
            backdrop: Some(format!("{dir}/backdrop-page.png")),
            dawn: true,
            ..Setup::default()
        };
        let fix = |h: String| {
            h.replace(
                "/poppins-light.ttf",
                &format!("file://{fonts}/Poppins-ExtraLight.ttf"),
            )
            .replace(
                "/poppins.ttf",
                &format!("file://{fonts}/Poppins-Regular.ttf"),
            )
            .replace(
                "url(/backdrop.png)",
                &format!("url(file://{dir}/backdrop-page.png)"),
            )
        };
        let put = |n: &str, h: String| std::fs::write(format!("{dir}/{n}.html"), fix(h)).unwrap();
        put("list", render_list(&s));
        put(
            "password",
            render_password(&s, "Groovy Wifi", s.nets.first()),
        );
        put("joining", render_joining(&s, "Groovy Wifi"));
        put("joined", render_joined(&s, "Groovy Wifi"));
        let pair = crate::pair::render(
            crate::pair::Ask::Button,
            "http://nas.local:8080/",
            "kitchen",
            "abc",
            Some("192.168.1.42"),
            Some(&format!("{dir}/backdrop-page.png")),
            true,
        );
        put(
            "pair",
            pair.replace(
                "file:///usr/share/fonts/thinframe",
                &format!("file://{fonts}"),
            ),
        );
    }

    fn req(method: &str, target: &str, body: &str, port: u16) -> Req {
        let (path, q) = target.split_once('?').unwrap_or((target, ""));
        Req {
            method: method.into(),
            path: path.into(),
            query: form_decode(q),
            host: format!("127.0.0.1:{port}"),
            origin: String::new(),
            form: form_decode(body),
        }
    }

    fn wait_join(st: &Shared) {
        for _ in 0..100 {
            if lock(st).joining.is_none() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("join never finished");
    }

    #[test]
    fn join_flow_wrong_then_right_password() {
        let st: Shared = Arc::default();
        let fake = Arc::new(Fake {
            up: AtomicBool::new(false),
            psk: "far out man",
        });
        let radio: Arc<dyn Radio> = fake.clone();
        assert_eq!(
            respond(&st, &radio, 1, &req("GET", "/wifi", "", 1)).status,
            404,
            "inactive"
        );
        {
            let mut s = lock(&st);
            s.active = true;
            s.nets = vec![net("Groovy Wifi", "psk"), net("Cafe", "open")];
        }
        let mut bad_host = req("GET", "/wifi", "", 1);
        bad_host.host = "evil.example:1".into();
        assert_eq!(respond(&st, &radio, 1, &bad_host).status, 404, "rebinding");

        let short = respond(
            &st,
            &radio,
            1,
            &req("POST", "/wifi/join", "ssid=Groovy+Wifi&psk=short", 1),
        );
        assert_eq!(short.status, 303);
        assert!(lock(&st).error.as_deref().unwrap().contains("8 to 63"));

        respond(
            &st,
            &radio,
            1,
            &req(
                "POST",
                "/wifi/join",
                "ssid=Groovy+Wifi&psk=wrong+password",
                1,
            ),
        );
        wait_join(&st);
        let again =
            String::from_utf8(respond(&st, &radio, 1, &req("GET", "/wifi", "", 1)).body).unwrap();
        assert!(
            again.contains("Check the password") && again.contains("id=\"kb\""),
            "back on the keyboard"
        );
        assert!(!fake.connected());

        respond(
            &st,
            &radio,
            1,
            &req("POST", "/wifi/join", "ssid=Groovy+Wifi&psk=far+out+man", 1),
        );
        wait_join(&st);
        assert!(fake.connected());
        let done =
            String::from_utf8(respond(&st, &radio, 1, &req("GET", "/wifi", "", 1)).body).unwrap();
        assert!(done.contains("Connected to Groovy Wifi"));
    }

    #[test]
    fn open_network_needs_no_password_and_server_speaks_http() {
        let st: Shared = Arc::default();
        {
            let mut s = lock(&st);
            s.active = true;
            s.nets = vec![net("Cafe", "open")];
        }
        let fake = Arc::new(Fake {
            up: AtomicBool::new(false),
            psk: "x",
        });
        let port = start(st.clone(), fake.clone(), "127.0.0.1:0").unwrap();
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let body = "ssid=Cafe&psk=";
        write!(
            c,
            "POST /wifi/join HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
             Content-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut out = String::new();
        c.read_to_string(&mut out).unwrap();
        assert!(
            out.starts_with("HTTP/1.1 303") && out.contains("Location: /wifi\r\n"),
            "{out}"
        );
        wait_join(&st);
        assert!(fake.connected());
    }

    #[test]
    fn rejoin_kicks() {
        let s = Duration::from_secs;
        assert!(!kick_due(s(3), None, false), "iwd gets its own first tries");
        assert!(kick_due(s(8), None, false));
        assert!(
            !kick_due(s(30), Some(s(5)), false),
            "not more than every KICK_EVERY"
        );
        assert!(kick_due(s(30), Some(s(20)), false));
        assert!(
            !kick_due(s(30), None, true),
            "the person's own join goes first"
        );
    }

    /// Records what each join was asked for.
    #[derive(Default)]
    struct Rec(Mutex<Vec<(String, Option<String>)>>);

    impl Radio for Rec {
        fn present(&self) -> bool {
            true
        }
        fn connected(&self) -> bool {
            false
        }
        fn scan(&self) -> Vec<Net> {
            vec![]
        }
        fn join(&self, ssid: &str, psk: Option<&str>) -> Result<(), String> {
            let mut v = self.0.lock().unwrap();
            v.push((ssid.into(), psk.map(str::to_string)));
            Ok(())
        }
    }

    fn post_join(st: &Shared, radio: &Arc<dyn Radio>, body: &str) {
        {
            let mut s = lock(st);
            s.joined = None;
            s.error = None;
        }
        respond(st, radio, 1, &req("POST", "/wifi/join", body, 1));
        wait_join(st);
    }

    #[test]
    fn same_name_open_and_secured_never_drops_the_password() {
        let nets = vec![net("Same", "open"), net("Same", "psk"), net("Lone", "open")];
        assert_eq!(find_net(&nets, "Same", None).unwrap().security, "psk");
        assert_eq!(
            find_net(&nets, "Same", Some("open")).unwrap().security,
            "open"
        );
        assert_eq!(find_net(&nets, "Lone", None).unwrap().security, "open");
        assert!(find_net(&nets, "Lone", Some("psk")).is_none());

        let st: Shared = Arc::default();
        {
            let mut s = lock(&st);
            s.active = true;
            s.nets = nets.clone();
        }
        let list = render_list(&lock(&st));
        assert!(list.contains("href=\"/wifi/net?ssid=Same&amp;sec=open\""));
        assert!(list.contains("href=\"/wifi/net?ssid=Same&amp;sec=psk\""));
        let p = String::from_utf8(
            respond(
                &st,
                &(Arc::new(Rec::default()) as Arc<dyn Radio>),
                1,
                &req("GET", "/wifi/net?ssid=Same&sec=psk", "", 1),
            )
            .body,
        )
        .unwrap();
        assert!(p.contains("name=\"sec\" value=\"psk\"") && p.contains("id=\"kb\""));

        let rec = Arc::new(Rec::default());
        let radio: Arc<dyn Radio> = rec.clone();
        // Old form, no sec: the secured entry wins and the password is used.
        post_join(&st, &radio, "ssid=Same&psk=secret+pass");
        // The open row tapped: no password.
        post_join(&st, &radio, "ssid=Same&sec=open&psk=");
        // A password typed even though the form says open: still a psk join.
        post_join(&st, &radio, "ssid=Same&sec=open&psk=typed+anyway");
        // Old form for a name that is only open.
        post_join(&st, &radio, "ssid=Lone&psk=");
        let got = rec.0.lock().unwrap().clone();
        let s = |v: &str| Some(v.to_string());
        assert_eq!(
            got,
            [
                ("Same".into(), s("secret pass")),
                ("Same".into(), None),
                ("Same".into(), s("typed anyway")),
                ("Lone".into(), None),
            ]
        );
    }

    #[test]
    fn passwords_are_printable_ascii_only() {
        // The keyboard offers nothing else.
        let layers = KEYBOARD_JS
            .lines()
            .filter(|l| l.starts_with("var L=") || l.starts_with("n:["))
            .collect::<String>();
        assert!(layers.contains("qwertyuiop") && layers.contains("_\\\\|~<>`"));
        assert!(layers.is_ascii(), "{layers}");
        assert!(!KEYBOARD_JS.contains('€') && !KEYBOARD_JS.contains('£'));

        let st: Shared = Arc::default();
        {
            let mut s = lock(&st);
            s.active = true;
            s.nets = vec![net("Home", "psk")];
        }
        let rec = Arc::new(Rec::default());
        let radio: Arc<dyn Radio> = rec.clone();
        for bad in [
            "ssid=Home&psk=%E2%82%AC%E2%82%AC%E2%82%AC".to_string(), // 3 x EUR: 9 bytes
            "ssid=Home&psk=caf%C3%A9caf%C3%A9".to_string(),
            "ssid=Home&psk=new%0Aline+here".to_string(),
            "ssid=Home&psk=del%7Fhere+ok".to_string(),
            format!("ssid=Home&psk={}", "x".repeat(64)),
        ] {
            post_join(&st, &radio, &bad);
            assert!(
                lock(&st).error.as_deref().unwrap_or("").contains("8 to 63"),
                "{bad}"
            );
        }
        post_join(&st, &radio, "ssid=Ho%0Ame&psk=goodpassword");
        assert_eq!(lock(&st).error.as_deref(), Some("Pick a network first."));
        assert!(rec.0.lock().unwrap().is_empty(), "nothing joined");
        post_join(&st, &radio, "ssid=Home&psk=%20sp%5Cace~%60%20");
        assert_eq!(
            rec.0.lock().unwrap().clone(),
            [("Home".to_string(), Some(" sp\\ace~` ".to_string()))]
        );
    }

    /// A connected pair: (our end, the client's end).
    fn pair() -> (TcpStream, TcpStream) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let c = TcpStream::connect(l.local_addr().unwrap()).unwrap();
        (l.accept().unwrap().0, c)
    }

    #[test]
    fn request_head_is_capped_and_has_one_deadline() {
        let limit = Duration::from_secs(5);
        let (s, mut c) = pair();
        write!(c, "GET /wifi?x=1 HTTP/1.1\r\nHost: a:1\r\n\r\n").unwrap();
        let r = read_req_within(&s, limit).unwrap();
        assert_eq!(
            (r.method.as_str(), r.path.as_str(), r.host.as_str()),
            ("GET", "/wifi", "a:1")
        );

        // One long header: refused once 8 KiB is read, without waiting for the rest.
        let (s, mut c) = pair();
        let long = format!("GET / HTTP/1.1\r\nX-Long: {}\r\n\r\n", "a".repeat(9000));
        c.write_all(long.as_bytes()).unwrap();
        let e = read_req_within(&s, limit).err().unwrap();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);

        // Many small headers that fit in 8 KiB but are over 50.
        let (s, mut c) = pair();
        let many: String = (0..51).map(|i| format!("X-{i}: y\r\n")).collect();
        write!(c, "GET / HTTP/1.1\r\n{many}\r\n").unwrap();
        assert!(read_req_within(&s, limit).is_err());
        let (s, mut c) = pair();
        let fifty: String = (0..50).map(|i| format!("X-{i}: y\r\n")).collect();
        write!(c, "GET / HTTP/1.1\r\n{fifty}\r\n").unwrap();
        assert!(read_req_within(&s, limit).is_ok());

        // A client trickling a byte at a time: each read is quick, the request is not.
        let (s, c) = pair();
        let t = thread::spawn(move || {
            let mut c = c;
            for b in b"GET / HTTP/1.1\r\nHost: slowly-slowly-slowly\r\n\r\n" {
                if c.write_all(&[*b]).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(30));
            }
        });
        let start = Instant::now();
        let e = read_req_within(&s, Duration::from_millis(300))
            .err()
            .unwrap();
        assert!(
            matches!(
                e.kind(),
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
            ),
            "{e:?}"
        );
        assert!(start.elapsed() < Duration::from_secs(1));
        drop(s);
        t.join().unwrap();
    }
}
