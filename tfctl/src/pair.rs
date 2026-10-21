//! One-press pairing (TASK 16, server/internal/api/pair.go). A frame without FRAME_TOKEN asks the
//! server to pair with a random nonce, then shows a local page with a big PAIR button. The button
//! posts the nonce from cog straight to the server, so only someone at the frame can accept. The
//! agent polls until the server hands over the frame's own token (once), stores it as FRAME_TOKEN
//! and opens the dashboard. A 401 later (the admin forgot the frame) clears the token and the
//! button comes back.
use crate::agent::Agent;
use crate::http;
use crate::json::{self, Value};
use crate::kiosk::KioskCmd;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);
/// How long to wait between steps while the button is up, and after "already paired".
pub const POLL: Duration = Duration::from_secs(2);
pub const REFUSED_RETRY: Duration = Duration::from_secs(30);

/// What the pairing page says.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ask {
    /// The PAIR button.
    Button,
    /// The server already has a token for this frame id.
    Refused,
}

pub(crate) fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The server as people know it: "nas.local:8080" from "http://nas.local:8080/".
fn server_name(url: &str) -> &str {
    url.trim_start_matches("http://").trim_end_matches('/')
}

pub fn render(
    ask: Ask,
    server: &str,
    frame_id: &str,
    nonce: &str,
    ip: Option<&str>,
    backdrop: Option<&str>,
) -> String {
    let bg = match backdrop {
        Some(b) => format!("#f4dcaa url(\"file://{}\") center/cover no-repeat", esc(b)),
        None => "#f4dcaa".into(),
    };
    let who = match ip {
        Some(ip) => format!("frame {} &middot; {}", esc(frame_id), esc(ip)),
        None => format!("frame {}", esc(frame_id)),
    };
    let body = match ask {
        Ask::Button => format!(
            "<h1>Pair this frame?</h1><p>with the thinframe server at {}</p>\
             <form method=\"post\" action=\"{}/pair/{}/accept\">\
             <input type=\"hidden\" name=\"nonce\" value=\"{}\"><button>PAIR</button></form>",
            esc(server_name(server)),
            esc(server.trim_end_matches('/')),
            esc(frame_id),
            esc(nonce)
        ),
        Ask::Refused => format!(
            "<h1>Already paired</h1><p>{} knows this frame already.<br>\
             Forget it on the server's admin page to pair it again.</p>",
            esc(server_name(server))
        ),
    };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>thinframe</title><style>\
         @font-face{{font-family:Shrikhand;src:url(\"file:///usr/share/fonts/thinframe/Shrikhand-Regular.ttf\")}}\
         html,body{{margin:0;height:100%;overflow:hidden;background:{bg};user-select:none;-webkit-user-select:none;touch-action:manipulation}}\
         body{{color:#5b3a1e;font:3.6vh Shrikhand,Georgia,serif}}\
         main{{position:absolute;left:5vw;top:40vh;width:54vw;padding:2.5vh 3vw 3.5vh;\
         background:#f4dcaa;border:.6vh solid #5b3a1e;border-radius:3vh;box-shadow:1.2vh 1.2vh 0 #b8452a}}\
         h1{{margin:0;font-size:7vh;font-weight:400;color:#b8452a}}p{{margin:.2em 0 .7em}}\
         button{{font:7vh Shrikhand,Georgia,serif;color:#f4dcaa;background:#d9662b;border:0;\
         border-radius:999px;padding:.15em 1.4em;box-shadow:0 1vh 0 #5b3a1e}}\
         button:active{{transform:translateY(.8vh);box-shadow:0 .2vh 0 #5b3a1e}}\
         small{{position:absolute;left:5.8vw;bottom:4vh;font-size:3.4vh}}\
         .steps{{position:absolute;right:3vw;top:2.4vh;font-size:3vh;background:#f4dcaa;border:.4vh solid #5b3a1e;border-radius:999px;padding:.1em .8em}}.steps b{{color:#b8452a;font-weight:400}}</style></head>\
         <body><div class=\"steps\">1 Wi-Fi &middot; <b>2 pair</b></div><main>{body}</main><small>{who}</small></body></html>"
    )
}

/// 32 hex characters from the kernel's RNG.
pub fn nonce() -> io::Result<String> {
    let mut b = [0u8; 16];
    fs::File::open("/dev/urandom")?.read_exact(&mut b)?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// The pairing page lives next to the status page.
pub fn page_path() -> PathBuf {
    let status = std::env::var("TFCTL_STATUS_PAGE")
        .unwrap_or_else(|_| crate::statuspage::DEFAULT_PATH.into());
    Path::new(&status).with_file_name("pair.html")
}

impl Agent {
    /// Whether the agent must pair before it can talk to the server.
    pub(crate) fn needs_pairing(&self) -> bool {
        self.cfg.token.is_empty()
    }

    /// One pairing step; returns how long to wait before the next one.
    pub(crate) fn pair_step(&mut self) -> Duration {
        let res = match self.pair_nonce.clone() {
            None => self.pair_request(),
            Some(n) => self.pair_poll(&n),
        };
        match res {
            Ok(d) => d,
            Err(e) => {
                eprintln!("tfctl: agent: pairing: {e}");
                if !self.pair_page_up {
                    self.server_failed();
                }
                Duration::from_secs(5)
            }
        }
    }

    fn pair_request(&mut self) -> io::Result<Duration> {
        let n = nonce()?;
        let body = Value::obj()
            .set("id", self.cfg.frame_id.as_str())
            .set("nonce", n.as_str())
            .to_string();
        let r = http::post_json(&self.api("/api/pair"), "", &body, TIMEOUT)?;
        match r.status {
            202 => {
                self.pair_nonce = Some(n.clone());
                self.show_pair_page(Ask::Button, &n);
                Ok(POLL)
            }
            409 => {
                self.show_pair_page(Ask::Refused, "");
                Ok(REFUSED_RETRY)
            }
            s => Err(io::Error::other(format!("POST /api/pair: HTTP {s}"))),
        }
    }

    fn pair_poll(&mut self, n: &str) -> io::Result<Duration> {
        let url = self.api(&format!("/api/pair/{}?nonce={n}", self.cfg.frame_id));
        let r = http::get(&url, "", TIMEOUT)?;
        match r.status {
            202 => Ok(POLL),
            200 => {
                let v = json::parse(&String::from_utf8_lossy(&r.body)).map_err(io::Error::other)?;
                let tok = v.field_string("token").unwrap_or_default();
                if tok.is_empty() || !tok.bytes().all(|b| b.is_ascii_alphanumeric()) {
                    return Err(io::Error::other("pairing answer without a usable token"));
                }
                self.paired(&tok);
                Ok(Duration::ZERO)
            }
            // Expired, or the server restarted: ask again with a new nonce.
            404 => {
                self.pair_nonce = None;
                Ok(Duration::ZERO)
            }
            s => Err(io::Error::other(format!("GET /api/pair: HTTP {s}"))),
        }
    }

    fn paired(&mut self, tok: &str) {
        self.persist(&[("FRAME_TOKEN", tok)]);
        self.cfg.token = tok.to_string();
        self.pair_nonce = None;
        self.pair_page_up = false;
        self.event("pair", "paired with the button on the frame");
        eprintln!("tfctl: agent: paired, opening the dashboard");
        self.on_fallback = false;
        let _ = self.send(KioskCmd::SetUrl(self.cfg.url.clone()));
    }

    /// The server no longer accepts our token: forget it, the PAIR button comes back.
    pub(crate) fn unauthorized(&mut self) {
        if self.cfg.token.is_empty() {
            return;
        }
        eprintln!("tfctl: agent: server refused our token, pairing again");
        self.persist(&[("FRAME_TOKEN", "")]);
        self.cfg.token.clear();
    }

    fn show_pair_page(&mut self, ask: Ask, nonce: &str) {
        let st = crate::status::collect();
        let ip = st
            .ips
            .iter()
            .find(|ip| *ip != crate::statuspage::USB_IP)
            .cloned();
        let path = page_path();
        let bg = path.with_file_name("backdrop.png");
        let backdrop = crate::splash::write_backdrop(&self.cfg, &bg.to_string_lossy())
            .ok()
            .flatten();
        let html = render(
            ask,
            &self.cfg.server_url,
            &self.cfg.frame_id,
            nonce,
            ip.as_deref(),
            backdrop.as_deref(),
        );
        let tmp = path.with_extension("html.tmp");
        if let Err(e) = fs::create_dir_all(path.parent().unwrap_or(Path::new("/")))
            .and_then(|_| fs::write(&tmp, html))
            .and_then(|_| fs::rename(&tmp, &path))
        {
            eprintln!("tfctl: agent: pairing page: {e}");
            return;
        }
        self.on_fallback = true;
        self.pair_page_up = true;
        // A fresh URL each time: cog reloads, so a new nonce is never stale on screen.
        let url = format!(
            "file://{}?{}",
            path.display(),
            nonce.get(..8).unwrap_or("x")
        );
        let _ = self.send(KioskCmd::SetUrl(url));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_posts_the_nonce_to_the_server() {
        let h = render(
            Ask::Button,
            "http://nas.local:8080/",
            "kitchen",
            "abc123",
            Some("192.168.1.20"),
            Some("/run/thinframe/backdrop.png"),
        );
        assert!(h.contains("action=\"http://nas.local:8080/pair/kitchen/accept\""));
        assert!(h.contains("name=\"nonce\" value=\"abc123\""));
        assert!(h.contains("<button>PAIR</button>"));
        assert!(h.contains("server at nas.local:8080"));
        assert!(h.contains("frame kitchen &middot; 192.168.1.20"));
        assert!(h.contains("url(\"file:///run/thinframe/backdrop.png\")"));
    }

    #[test]
    fn refused_page_has_no_button_and_escapes() {
        let h = render(Ask::Refused, "http://a<b", "k", "", None, None);
        assert!(!h.contains("<button>"));
        assert!(h.contains("a&lt;b"));
        assert!(h.contains("Already paired"));
    }

    #[test]
    fn nonce_is_32_hex_and_differs() {
        let (a, b) = (nonce().unwrap(), nonce().unwrap());
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}
