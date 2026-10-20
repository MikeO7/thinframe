//! The one local page besides the dashboard (TASKS 14/15): a static status screen cog shows while
//! the server is unreachable. tfctl rewrites it with the reason and the frame's IP; no JS.
use std::fs;
use std::io;
use std::path::Path;

pub const DEFAULT_PATH: &str = "/var/lib/thinframe/www/status.html";

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Headline ("cannot reach http://nas:8080") plus detail lines (IP, frame id, retry note).
pub fn render(headline: &str, details: &[String]) -> String {
    let lines: String = details
        .iter()
        .map(|d| format!("<p>{}</p>", esc(d)))
        .collect();
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>thinframe</title>\
         <meta http-equiv=\"refresh\" content=\"15\">\
         <style>html,body{{margin:0;height:100%;background:#0b0d10;color:#dfe3e8;\
         font:32px sans-serif}}main{{height:100%;display:flex;flex-direction:column;\
         align-items:center;justify-content:center;text-align:center}}\
         h1{{font-size:44px;font-weight:600}}p{{margin:.2em 0;color:#9aa4af}}</style></head>\
         <body><main><h1>{}</h1>{}</main></body></html>",
        esc(headline),
        lines
    )
}

/// Write atomically (tmp + rename) so cog never reads half a file.
pub fn write(path: &str, headline: &str, details: &[String]) -> io::Result<()> {
    let p = Path::new(path);
    if let Some(dir) = p.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = p.with_extension("html.tmp");
    fs::write(&tmp, render(headline, details))?;
    fs::rename(&tmp, p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_and_includes_details() {
        let h = render(
            "cannot reach <srv>",
            &["IP 192.168.1.5".into(), "a&b".into()],
        );
        assert!(h.contains("cannot reach &lt;srv&gt;"));
        assert!(h.contains("<p>IP 192.168.1.5</p>"));
        assert!(h.contains("<p>a&amp;b</p>"));
        assert!(!h.contains("<srv>"));
    }

    #[test]
    fn writes_atomically_into_new_dirs() {
        let d = std::env::temp_dir().join(format!("tf-status-{}", std::process::id()));
        let f = d.join("www/status.html");
        write(f.to_str().unwrap(), "joining HomeWifi", &[]).unwrap();
        assert!(fs::read_to_string(&f).unwrap().contains("joining HomeWifi"));
        assert!(!d.join("www/status.html.tmp").exists());
        let _ = fs::remove_dir_all(&d);
    }
}
