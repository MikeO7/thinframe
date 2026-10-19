//! KEY=VALUE config file. No TOML crate: the file has six keys.
use std::collections::HashMap;
use std::fs;
use std::io;

pub const DEFAULT_PATH: &str = "/etc/alpineframe.conf";

#[derive(Debug, Clone)]
pub struct Config {
    pub url: String,
    pub platform: String,  // "drm" or "wl"
    pub rotation: u8,      // quarter turns 0..3
    pub extra_args: Vec<String>,
    pub user: String,
    pub repo_tarball: String, // https://github.com/<owner>/alpineframe/archive/refs/heads/main.tar.gz
    pub health_interval_s: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            url: "file:///var/lib/alpineframe/www/index.html".into(),
            platform: "drm".into(),
            rotation: 3,
            extra_args: vec![],
            user: "frame".into(),
            repo_tarball: "https://github.com/MikeO7/alpineframe/archive/refs/heads/main.tar.gz".into(),
            health_interval_s: 30,
        }
    }
}

pub fn load(path: &str) -> io::Result<Config> {
    let mut cfg = Config::default();
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(cfg),
        Err(e) => return Err(e),
    };
    let kv: HashMap<&str, &str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim(), v.trim().trim_matches('"')))
        .collect();
    if let Some(v) = kv.get("FRAME_URL") { cfg.url = v.to_string(); }
    if let Some(v) = kv.get("COG_PLATFORM") { cfg.platform = v.to_string(); }
    if let Some(v) = kv.get("COG_ROTATION") { cfg.rotation = v.parse().unwrap_or(cfg.rotation); }
    if let Some(v) = kv.get("COG_EXTRA_ARGS") { cfg.extra_args = v.split_whitespace().map(String::from).collect(); }
    if let Some(v) = kv.get("FRAME_USER") { cfg.user = v.to_string(); }
    if let Some(v) = kv.get("REPO_TARBALL") { cfg.repo_tarball = v.to_string(); }
    if let Some(v) = kv.get("HEALTH_INTERVAL_S") { cfg.health_interval_s = v.parse().unwrap_or(cfg.health_interval_s); }
    Ok(cfg)
}
