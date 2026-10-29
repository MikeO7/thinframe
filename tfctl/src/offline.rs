//! The last dashboard, kept on /data and shown at boot before the network is up
//! (docs/decisions.md "Speed"). cog opens it at once; the live page replaces it in place as soon
//! as the server answers, so the screen shows the dashboard seconds earlier instead of the
//! status page. Its numbers are from the last time it was saved until then; the clock is live.
//! The agent drops back to the status page while Wi-Fi or the server stay away (agent.rs
//! `leave_cached`), so a frame that cannot reach its server still says why.
use crate::http::{self, Url};
use std::io;
use std::path::Path;
use std::time::Duration;

pub const PAGE: &str = "/data/thinframe/dashboard.html";
const TIMEOUT: Duration = Duration::from_secs(10);

/// The page's URL for cog, when a saved page exists.
pub fn url() -> Option<String> {
    Path::new(PAGE).is_file().then(|| format!("file://{PAGE}"))
}

/// The page with a `<base>` at the server, so its fonts, scripts, stream and beacons still reach
/// the server from a file:// URL.
pub fn with_base(html: &str, server_url: &str) -> String {
    let tag = format!("<base href=\"{}/\">", server_url.trim_end_matches('/'));
    match html.find("<head>") {
        Some(i) => format!("{}{tag}{}", &html[..i + 6], &html[i + 6..]),
        None => format!("{tag}{html}"),
    }
}

/// Fetch the dashboard from `url` and keep it (written to a temporary name and renamed: a power
/// cut leaves the old page or none).
pub fn save(url: &str, server_url: &str, token: &str) -> io::Result<()> {
    let u = Url::parse(url).ok_or_else(|| io::Error::other(format!("bad URL {url}")))?;
    let r = http::get(&u, token, TIMEOUT)?;
    if r.status != 200 {
        return Err(io::Error::other(format!("GET {url}: HTTP {}", r.status)));
    }
    let html = String::from_utf8_lossy(&r.body);
    if !html.contains("<html") && !html.contains("<!doctype") && !html.contains("<!DOCTYPE") {
        return Err(io::Error::other(format!("GET {url}: not a page")));
    }
    // Synced before the rename: a power cut right after the first save left an empty page,
    // which the next boot opened.
    crate::update::write_synced(
        std::path::Path::new(PAGE),
        with_base(&html, server_url).as_bytes(),
    )
}

/// One save at a time: a connect and a page-version change share the temporary file.
static SAVING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// `save` in its own thread: the agent's loop does not wait on it.
pub fn save_in_background(url: String, server_url: String, token: String) {
    let _ = std::thread::Builder::new()
        .name("offline".into())
        .spawn(move || {
            let _one = SAVING.lock().unwrap_or_else(|p| p.into_inner());
            if let Err(e) = save(&url, &server_url, &token) {
                eprintln!("tfctl: offline page: {e}");
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_goes_first_in_head() {
        assert_eq!(
            with_base("<html><head><title>x</title></head>", "http://nas:41007/"),
            "<html><head><base href=\"http://nas:41007/\"><title>x</title></head>"
        );
        assert_eq!(
            with_base("<p>hi", "http://nas:41007"),
            "<base href=\"http://nas:41007/\"><p>hi"
        );
    }
}
