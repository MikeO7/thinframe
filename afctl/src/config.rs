//! KEY=VALUE config file. No TOML crate: the file has a handful of keys.
use std::collections::HashMap;
use std::fs;
use std::io;

pub const DEFAULT_PATH: &str = "/etc/alpineframe.conf";

#[derive(Debug, Clone)]
pub struct Config {
    pub url: String,
    /// "drm" or "wl"
    pub platform: String,
    /// quarter turns 0..3
    pub rotation: u8,
    pub extra_args: Vec<String>,
    pub user: String,
    /// https://github.com/<owner>/alpineframe/archive/refs/heads/main.tar.gz
    pub repo_tarball: String,
    pub health_interval_s: u64,
    /// Thin-client mode: base URL of frameserver, e.g. http://nas.local:8080. Empty = standalone.
    pub server_url: String,
    pub frame_id: String,
    pub poll_interval_s: u64,
    /// Shown when the server is unreachable.
    pub fallback_url: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            url: "file:///var/lib/alpineframe/www/index.html".into(),
            platform: "drm".into(),
            rotation: 3,
            extra_args: vec![],
            user: "frame".into(),
            repo_tarball: concat!(
                "https://github.com/MikeO7/alpineframe/",
                "archive/refs/heads/main.tar.gz"
            )
            .into(),
            health_interval_s: 30,
            server_url: String::new(),
            frame_id: "frame".into(),
            poll_interval_s: 15,
            fallback_url: "file:///var/lib/alpineframe/www/index.html".into(),
        }
    }
}

pub fn load(path: &str) -> io::Result<Config> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(e) => return Err(e),
    };
    Ok(parse(&text))
}

pub fn parse(text: &str) -> Config {
    let mut cfg = Config::default();
    let kv: HashMap<&str, &str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim(), v.trim().trim_matches('"')))
        .collect();
    let get = |k: &str, d: &str| kv.get(k).map(|v| v.to_string()).unwrap_or(d.into());
    cfg.url = get("FRAME_URL", &cfg.url);
    cfg.platform = get("COG_PLATFORM", &cfg.platform);
    cfg.user = get("FRAME_USER", &cfg.user);
    cfg.repo_tarball = get("REPO_TARBALL", &cfg.repo_tarball);
    cfg.server_url = get("SERVER_URL", &cfg.server_url);
    cfg.frame_id = get("FRAME_ID", &cfg.frame_id);
    cfg.fallback_url = get("FALLBACK_URL", &cfg.fallback_url);
    if let Some(v) = kv.get("POLL_INTERVAL_S") {
        cfg.poll_interval_s = v.parse().unwrap_or(cfg.poll_interval_s);
    }
    if !cfg.server_url.is_empty() && !kv.contains_key("FRAME_URL") {
        cfg.url = format!("{}/frame/{}", cfg.server_url.trim_end_matches('/'), cfg.frame_id);
    }
    cfg.extra_args = get("COG_EXTRA_ARGS", "")
        .split_whitespace()
        .map(String::from)
        .collect();
    if let Some(v) = kv.get("COG_ROTATION") {
        cfg.rotation = v.parse().unwrap_or(cfg.rotation);
    }
    if let Some(v) = kv.get("HEALTH_INTERVAL_S") {
        cfg.health_interval_s = v.parse().unwrap_or(cfg.health_interval_s);
    }
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quoted_values_and_comments() {
        let c = parse("# x\nFRAME_URL=\"http://a/\"\nCOG_ROTATION=1\nCOG_EXTRA_ARGS=\"--a --b\"\n");
        assert_eq!(c.url, "http://a/");
        assert_eq!(c.rotation, 1);
        assert_eq!(c.extra_args, vec!["--a", "--b"]);
        assert_eq!(c.platform, "drm");
    }

    #[test]
    fn server_url_derives_frame_url() {
        let c = parse("SERVER_URL=http://nas:8080/\nFRAME_ID=kitchen\n");
        assert_eq!(c.url, "http://nas:8080/frame/kitchen");
    }
}
