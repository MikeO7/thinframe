//! The one local page besides the dashboard (TASKS 14/15): what the frame is waiting for, shown
//! by cog until the server answers. Two files, no JS. `status.html` is the still shell (the
//! splash picture as backdrop, or the plain colour and logo) and never reloads, so its big
//! backdrop is decoded once; `status-line.html` is a transparent iframe with the headline and
//! details that reloads itself every 15 s.
use std::fs;
use std::io;
use std::path::Path;

/// On tmpfs: the root file system is read-only.
pub const DEFAULT_PATH: &str = "/run/thinframe/status.html";
/// Next to the shell.
pub const LINE_FILE: &str = "status-line.html";

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The shell: the background and the frame for the line. With a backdrop (retro) the picture
/// already carries the logo; plain shows the logo itself (TASK 15: same logo from splash on).
pub fn render_shell(logo: Option<&str>, backdrop: Option<&str>) -> String {
    let bg = match backdrop {
        Some(b) => format!("#07051a url(\"file://{}\") center/cover no-repeat", esc(b)),
        None => "#0b0d10".into(),
    };
    let img = match (backdrop, logo) {
        (None, Some(p)) => format!("<img src=\"file://{}\" alt=\"\">", esc(p)),
        _ => String::new(),
    };
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>thinframe</title>\
         <style>html,body{{margin:0;height:100%;overflow:hidden;background:{bg}}}\
         img{{position:absolute;left:50%;top:32%;transform:translate(-50%,-50%);\
         max-width:40%;max-height:40%}}iframe{{position:absolute;left:0;top:0;width:100%;\
         height:100%;border:0;background:transparent}}</style></head>\
         <body>{img}<iframe src=\"{LINE_FILE}\"></iframe></body></html>"
    )
}

/// Amber terminal plate under the picture, like the splash's status line.
const RETRO_CSS: &str = "html,body{margin:0;height:100%;overflow:hidden;background:transparent;\
    color:#ffb000;font:28px \"DejaVu Sans Mono\",monospace}\
    main{position:absolute;left:0;right:0;top:79%;display:flex;flex-direction:column;\
    align-items:center;text-align:center}\
    h1{margin:0;padding:.3em .9em;font-size:34px;font-weight:700;background:rgba(5,2,12,.78);\
    border:solid #3fd8ff;border-width:2px 0;text-shadow:3px 3px #3a1d00;letter-spacing:.03em}\
    h1::after{content:\"\";display:inline-block;width:.55em;height:.8em;margin-left:.45em;\
    background:#ffb000;vertical-align:-.06em;animation:b 1.1s steps(1) infinite}\
    @keyframes b{50%{opacity:0}}\
    p{margin:.4em 0 0;color:#7ef0ff;font-size:22px;text-shadow:2px 2px #1d0b3d}";

const PLAIN_CSS: &str = "html,body{margin:0;height:100%;overflow:hidden;background:transparent;\
    color:#dfe3e8;font:32px sans-serif}\
    main{position:absolute;left:0;right:0;top:58%;display:flex;flex-direction:column;\
    align-items:center;text-align:center}\
    h1{margin:0 0 .3em;font-size:44px;font-weight:600}p{margin:.2em 0;color:#9aa4af}";

/// The line: headline ("cannot reach http://nas:8080") plus detail lines (frame id, IP, retry).
pub fn render_line(headline: &str, details: &[String], retro: bool) -> String {
    let lines: String = details
        .iter()
        .map(|d| format!("<p>{}</p>", esc(d)))
        .collect();
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <meta http-equiv=\"refresh\" content=\"15\"><style>{}</style></head>\
         <body><main><h1>{}</h1>{}</main></body></html>",
        if retro { RETRO_CSS } else { PLAIN_CSS },
        esc(headline),
        lines
    )
}

/// The USB-C link's own address (image/rootfs/etc/network/interfaces); not a way to the server.
pub const USB_IP: &str = "192.168.7.2";

/// What the frame is waiting for (TASK 15 order: joining Wi-Fi, reaching the server, given up).
/// `ips` are the frame's IPv4 addresses, `failures` the failed server attempts so far.
pub fn headline(
    server: &str,
    ssid: Option<&str>,
    ips: &[String],
    failures: u32,
    give_up: u32,
) -> String {
    let online = ips.iter().any(|ip| ip != USB_IP);
    match (online, ssid) {
        (false, Some(s)) => format!("joining {s}"),
        (false, None) => "no Wi-Fi configured".into(),
        (true, _) if failures < give_up => format!("reaching {server}"),
        (true, _) => format!("cannot reach {server}"),
    }
}

/// Write the line (and the shell when it changed) atomically, tmp + rename, so cog never reads
/// half a file.
pub fn write(
    path: &str,
    headline: &str,
    details: &[String],
    backdrop: Option<&str>,
) -> io::Result<()> {
    let p = Path::new(path);
    if let Some(dir) = p.parent() {
        fs::create_dir_all(dir)?;
    }
    let logo = [crate::splash::DATA_LOGO_PATH, crate::splash::LOGO_PATH]
        .into_iter()
        .find(|l| Path::new(l).exists());
    let put = |file: &Path, body: String| -> io::Result<()> {
        if fs::read_to_string(file).ok().as_deref() == Some(body.as_str()) {
            return Ok(());
        }
        let tmp = file.with_extension("html.tmp");
        fs::write(&tmp, body)?;
        fs::rename(&tmp, file)
    };
    put(
        &p.with_file_name(LINE_FILE),
        render_line(headline, details, backdrop.is_some()),
    )?;
    put(p, render_shell(logo, backdrop))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_escapes_and_includes_details() {
        let h = render_line(
            "cannot reach <srv>",
            &["IP 192.168.1.5".into(), "a&b".into()],
            false,
        );
        assert!(h.contains("<h1>cannot reach &lt;srv&gt;</h1>"));
        assert!(h.contains("<p>IP 192.168.1.5</p>"));
        assert!(h.contains("<p>a&amp;b</p>"));
        assert!(!h.contains("<srv>"));
        assert!(
            h.contains("http-equiv=\"refresh\""),
            "the line reloads itself"
        );
        assert!(
            render_line("x", &[], true).contains("#ffb000"),
            "retro is amber"
        );
    }

    #[test]
    fn shell_holds_backdrop_or_logo_and_never_reloads() {
        let plain = render_shell(Some("/data/thinframe/splash.png"), None);
        assert!(plain.contains("<img src=\"file:///data/thinframe/splash.png\""));
        let retro = render_shell(Some("/l.png"), Some("/run/b.png"));
        assert!(retro.contains("url(\"file:///run/b.png\")"));
        assert!(
            !retro.contains("<img"),
            "the backdrop already carries the logo"
        );
        for s in [plain, retro] {
            assert!(s.contains("<iframe src=\"status-line.html\">"));
            assert!(!s.contains("refresh"));
        }
    }

    #[test]
    fn backdrop_is_written_once_and_decodes() {
        let d = std::env::temp_dir().join(format!("tf-backdrop-{}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        let p = d.join("backdrop.png");
        let cfg = crate::config::Config::default();
        let got = crate::splash::write_backdrop(&cfg, p.to_str().unwrap()).unwrap();
        assert_eq!(got.as_deref(), p.to_str());
        let img = crate::png::decode(&fs::read(&p).unwrap()).unwrap();
        assert_eq!((img.w, img.h), (1280, 800), "rotation 3: landscape");
        let plain = crate::config::Config {
            splash_style: "plain".into(),
            ..crate::config::Config::default()
        };
        assert!(crate::splash::write_backdrop(&plain, "/nonexistent/x.png")
            .unwrap()
            .is_none());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn headline_follows_the_boot_order() {
        let s = "http://nas:8080";
        let usb = vec![USB_IP.to_string()];
        let lan = vec![USB_IP.to_string(), "192.168.1.20".to_string()];
        assert_eq!(headline(s, Some("Home"), &[], 0, 3), "joining Home");
        assert_eq!(headline(s, Some("Home"), &usb, 5, 3), "joining Home");
        assert_eq!(headline(s, None, &usb, 0, 3), "no Wi-Fi configured");
        assert_eq!(
            headline(s, Some("Home"), &lan, 2, 3),
            "reaching http://nas:8080"
        );
        assert_eq!(
            headline(s, Some("Home"), &lan, 3, 3),
            "cannot reach http://nas:8080"
        );
    }

    #[test]
    fn writes_atomically_into_new_dirs() {
        let d = std::env::temp_dir().join(format!("tf-status-{}", std::process::id()));
        let f = d.join("www/status.html");
        write(f.to_str().unwrap(), "joining HomeWifi", &[], None).unwrap();
        let line = d.join("www/status-line.html");
        assert!(fs::read_to_string(&line)
            .unwrap()
            .contains("joining HomeWifi"));
        assert!(fs::read_to_string(&f).unwrap().contains("status-line.html"));
        assert!(!d.join("www/status.html.tmp").exists());
        assert!(!d.join("www/status-line.html.tmp").exists());
        write(f.to_str().unwrap(), "reaching http://nas", &[], None).unwrap();
        assert!(fs::read_to_string(&line)
            .unwrap()
            .contains("reaching http://nas"));
        let _ = fs::remove_dir_all(&d);
    }
}
