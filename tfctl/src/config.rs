//! KEY=VALUE config file. No TOML crate: the file has a handful of keys.
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;

pub const DEFAULT_PATH: &str = "/etc/thinframe.conf";

#[derive(Debug, Clone)]
pub struct Config {
    pub url: String,
    /// "drm" or "wl"
    pub platform: String,
    /// cog DRM renderer: "gles" (rotates) or "modeset" (no rotation; plain KMS planes)
    pub renderer: String,
    /// quarter turns 0..3
    pub rotation: u8,
    pub extra_args: Vec<String>,
    pub user: String,
    /// https://github.com/<owner>/thinframe/archive/refs/heads/main.tar.gz
    pub repo_tarball: String,
    pub health_interval_s: u64,
    /// Thin-client mode: base URL of frameserver, e.g. http://nas.local:8080. Empty = standalone.
    pub server_url: String,
    pub frame_id: String,
    pub poll_interval_s: u64,
    /// Shown when the server is unreachable.
    pub fallback_url: String,
    /// 0..100, applied to /sys/class/backlight/*/brightness (TASK 10)
    pub brightness: u8,
    /// "HH:MM" local time; empty = never off (TASK 10)
    pub screen_off: String,
    pub screen_on: String,
    /// Shared secret with frameserver, sent as `Authorization: Bearer`.
    pub token: String,
    /// Last page_version applied from the server; reload cog when it changes.
    pub page_version: String,
    /// Time zone name or POSIX TZ string from the server's hello reply.
    pub tz: String,
    /// Splash screen (TASK 15): background "#rrggbb" and the one text line.
    pub splash_bg: u32,
    pub splash_text: String,
    /// "dawn" (default, animated), "seventies" (animated), "retro" or "plain"
    pub splash_style: String,
    /// Where this config was loaded from (and where overrides are written back).
    pub path: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            url: "file:///var/lib/thinframe/www/index.html".into(),
            platform: "drm".into(),
            renderer: "gles".into(),
            rotation: 3,
            extra_args: vec![],
            user: "frame".into(),
            repo_tarball: concat!(
                "https://github.com/MikeO7/thinframe/",
                "archive/refs/heads/main.tar.gz"
            )
            .into(),
            health_interval_s: 30,
            server_url: String::new(),
            frame_id: "frame".into(),
            poll_interval_s: 15,
            fallback_url: "file:///run/thinframe/status.html".into(),
            brightness: 100,
            screen_off: String::new(),
            screen_on: String::new(),
            token: String::new(),
            page_version: String::new(),
            tz: String::new(),
            splash_bg: 0x0b0d10,
            splash_text: "thinframe".into(),
            splash_style: "dawn".into(),
            path: DEFAULT_PATH.into(),
        }
    }
}

/// Path from $TFCTL_CONF (sim, tests on the Mac) or the default.
pub fn path() -> String {
    std::env::var("TFCTL_CONF").unwrap_or_else(|_| DEFAULT_PATH.into())
}

/// The config file is world-readable (the `frame` ssh user runs `tfctl status`, the tests grep
/// it); the token is not. It lives next to the real file as `thinframe.token`, mode 0600, and
/// wins over a `FRAME_TOKEN` line left in the file by an older tfctl. A reader that may not
/// open the token file (not root) gets an empty token, not an error.
pub fn load(path: &str) -> io::Result<Config> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let mut cfg = parse(&text);
    cfg.path = path.to_string();
    if let Ok(t) = fs::read_to_string(token_path(path)) {
        let t = t.trim();
        if !t.is_empty() {
            cfg.token = t.to_string();
        }
    }
    Ok(cfg)
}

/// `/etc/thinframe.conf` -> `/data/etc/thinframe.token` (the link is followed: `/etc` is
/// read-only on the frame).
pub fn token_path(conf: &str) -> std::path::PathBuf {
    resolve_target(Path::new(conf)).with_extension("token")
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
    cfg.renderer = get("COG_RENDERER", &cfg.renderer);
    cfg.user = get("FRAME_USER", &cfg.user);
    cfg.repo_tarball = get("REPO_TARBALL", &cfg.repo_tarball);
    cfg.server_url = get("SERVER_URL", &cfg.server_url);
    cfg.frame_id = get("FRAME_ID", &cfg.frame_id);
    cfg.fallback_url = get("FALLBACK_URL", &cfg.fallback_url);
    cfg.screen_off = get("SCREEN_OFF", &cfg.screen_off);
    cfg.screen_on = get("SCREEN_ON", &cfg.screen_on);
    cfg.token = get("FRAME_TOKEN", &cfg.token);
    cfg.page_version = get("PAGE_VERSION", &cfg.page_version);
    cfg.tz = get("TZ", &cfg.tz);
    cfg.splash_text = get("SPLASH_TEXT", &cfg.splash_text);
    cfg.splash_style = get("SPLASH_STYLE", &cfg.splash_style);
    if let Some(bg) = kv.get("SPLASH_BG").and_then(|v| parse_rgb(v)) {
        cfg.splash_bg = bg;
    }
    if let Some(v) = kv.get("BRIGHTNESS") {
        cfg.brightness = v.parse().unwrap_or(cfg.brightness).min(100);
    }
    if let Some(v) = kv.get("POLL_INTERVAL_S") {
        cfg.poll_interval_s = v.parse().unwrap_or(cfg.poll_interval_s);
    }
    if !cfg.server_url.is_empty() && !kv.contains_key("FRAME_URL") {
        let base = cfg.server_url.trim_end_matches('/');
        cfg.url = format!("{base}/frame/{}", cfg.frame_id);
    }
    cfg.extra_args = get("COG_EXTRA_ARGS", "")
        .split_whitespace()
        .map(String::from)
        .collect();
    if let Some(r) = kv.get("COG_ROTATION").and_then(|v| v.parse::<u8>().ok()) {
        if r < 4 {
            cfg.rotation = r;
        }
    }
    if let Some(v) = kv.get("HEALTH_INTERVAL_S") {
        cfg.health_interval_s = v.parse().unwrap_or(cfg.health_interval_s);
    }
    cfg
}

/// "#rrggbb" or "rrggbb" -> 0xrrggbb.
pub fn parse_rgb(s: &str) -> Option<u32> {
    let h = s.trim().trim_start_matches('#');
    (h.len() == 6)
        .then(|| u32::from_str_radix(h, 16).ok())
        .flatten()
}

/// Rewrite the given keys in place (comments, order and unrelated lines preserved), append the
/// ones that were missing, drop duplicates of a rewritten key. Atomic via rename next to the
/// real file: on the frame /etc/thinframe.conf is a symlink into /data (the root is read-only).
/// `FRAME_TOKEN` never lands in the file: it goes to the 0600 token file (an empty value
/// removes it) and the line in the file is blanked. A token an older tfctl left in the file is
/// moved out on the next save, whatever keys that save carries.
pub fn save_overrides(path: &str, kv: &[(&str, &str)]) -> io::Result<()> {
    let real = resolve_target(Path::new(path));
    let path = real.to_str().unwrap_or(path);
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let legacy = parse(&text).token;
    let token = kv
        .iter()
        .find(|(k, _)| *k == "FRAME_TOKEN")
        .map(|(_, v)| *v)
        .or((!legacy.is_empty()).then_some(legacy.as_str()));
    let mut kv: Vec<(&str, &str)> = kv
        .iter()
        .copied()
        .filter(|(k, _)| *k != "FRAME_TOKEN")
        .collect();
    if let Some(t) = token {
        save_token(Path::new(path), t)?;
        if !legacy.is_empty() {
            kv.push(("FRAME_TOKEN", ""));
        }
    }
    let tmp = format!("{path}.tmp");
    write_mode(
        Path::new(&tmp),
        apply_overrides(&text, &kv).as_bytes(),
        0o644,
    )?;
    fs::rename(&tmp, path)
}

fn save_token(conf: &Path, token: &str) -> io::Result<()> {
    let tp = conf.with_extension("token");
    if token.is_empty() {
        return match fs::remove_file(&tp) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    let tmp = conf.with_extension("token.tmp");
    write_private(&tmp, format!("{token}\n").as_bytes())?;
    fs::rename(&tmp, &tp)
}

/// Where a write to `path` lands: the symlink target, followed even when it does not exist yet
/// (a fresh data partition: `/etc/thinframe.conf` on the read-only root points at a file that
/// `canonicalize` cannot see, and the temp file would land in `/etc`, EROFS).
fn resolve_target(path: &Path) -> std::path::PathBuf {
    if let Ok(real) = fs::canonicalize(path) {
        return real;
    }
    let mut cur = path.to_path_buf();
    for _ in 0..8 {
        match fs::read_link(&cur) {
            Ok(t) if t.is_absolute() => cur = t,
            Ok(t) => cur = cur.parent().unwrap_or(Path::new("/")).join(t),
            Err(_) => break,
        }
    }
    cur
}

/// Write a file that may hold a secret (the token, a Wi-Fi passphrase): created mode 0600
/// before any byte lands in it, and flushed to disk so a rename right after is durable.
pub fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_mode(path, bytes, 0o600)
}

fn write_mode(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

pub fn apply_overrides(text: &str, kv: &[(&str, &str)]) -> String {
    let quote = |v: &str| format!("\"{}\"", v.replace(['"', '\n', '\r'], ""));
    let mut seen = vec![false; kv.len()];
    let mut out = String::with_capacity(text.len() + 64);
    for line in text.lines() {
        let t = line.trim();
        let key = (!t.is_empty() && !t.starts_with('#'))
            .then(|| t.split_once('=').map(|(k, _)| k.trim()))
            .flatten();
        if let Some(i) = key.and_then(|k| kv.iter().position(|(kk, _)| *kk == k)) {
            if seen[i] {
                continue;
            }
            seen[i] = true;
            out.push_str(&format!("{}={}\n", kv[i].0, quote(kv[i].1)));
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    for (i, (k, v)) in kv.iter().enumerate() {
        if !seen[i] {
            out.push_str(&format!("{k}={}\n", quote(v)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_rewrite_in_place_and_append() {
        let src = "# head\nSERVER_URL=\"\"\n#FRAME_URL=\"x\"\nCOG_ROTATION=3\nCOG_ROTATION=1\n";
        let out = apply_overrides(
            src,
            &[
                ("COG_ROTATION", "2"),
                ("FRAME_URL", "http://a/\"b"),
                ("BRIGHTNESS", "50"),
            ],
        );
        assert_eq!(
            out,
            "# head\nSERVER_URL=\"\"\n#FRAME_URL=\"x\"\nCOG_ROTATION=\"2\"\nFRAME_URL=\"http://a/b\"\nBRIGHTNESS=\"50\"\n"
        );
        let c = parse(&out);
        assert_eq!(c.rotation, 2);
        assert_eq!(c.url, "http://a/b");
        assert_eq!(c.brightness, 50);
    }

    #[test]
    fn save_overrides_creates_file() {
        let dir = std::env::temp_dir().join(format!("tfctl-cfg-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("thinframe.conf");
        let ps = p.to_str().unwrap();
        save_overrides(ps, &[("FRAME_ID", "kitchen")]).unwrap();
        save_overrides(ps, &[("FRAME_TOKEN", "t0k")]).unwrap();
        let c = load(ps).unwrap();
        assert_eq!(c.frame_id, "kitchen");
        assert_eq!(c.token, "t0k");
        // The token is not in the world-readable file; it is in the 0600 token file.
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&p), 0o644);
        assert!(!fs::read_to_string(&p).unwrap().contains("t0k"));
        let tp = dir.join("thinframe.token");
        assert_eq!(mode(&tp), 0o600);
        assert_eq!(fs::read_to_string(&tp).unwrap(), "t0k\n");
        // Forgetting the token removes the file.
        save_overrides(ps, &[("FRAME_TOKEN", "")]).unwrap();
        assert!(!tp.exists());
        assert_eq!(load(ps).unwrap().token, "");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_token_left_in_the_file_by_an_older_tfctl_moves_out_on_the_next_save() {
        let dir = std::env::temp_dir().join(format!("tfctl-cfg-mig-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("thinframe.conf");
        let ps = p.to_str().unwrap();
        fs::write(&p, "FRAME_ID=\"a\"\nFRAME_TOKEN=\"old\"\n").unwrap();
        assert_eq!(load(ps).unwrap().token, "old");
        save_overrides(ps, &[("TZ", "UTC")]).unwrap();
        let text = fs::read_to_string(&p).unwrap();
        assert!(text.contains("FRAME_TOKEN=\"\"\n"), "{text}");
        assert!(!text.contains("old"));
        assert_eq!(
            fs::read_to_string(dir.join("thinframe.token")).unwrap(),
            "old\n"
        );
        assert_eq!(load(ps).unwrap().token, "old");
        // A new token replaces it; the token file wins over any line in the file.
        save_overrides(ps, &[("FRAME_TOKEN", "new")]).unwrap();
        assert_eq!(load(ps).unwrap().token, "new");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn save_overrides_writes_through_a_symlink() {
        let dir = std::env::temp_dir().join(format!("tfctl-cfg-ln-{}", std::process::id()));
        fs::create_dir_all(dir.join("data")).unwrap();
        fs::create_dir_all(dir.join("etc")).unwrap();
        let real = dir.join("data/thinframe.conf");
        fs::write(&real, "FRAME_ID=\"a\"\n").unwrap();
        let link = dir.join("etc/thinframe.conf");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        save_overrides(link.to_str().unwrap(), &[("FRAME_ID", "b")]).unwrap();
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(load(link.to_str().unwrap()).unwrap().frame_id, "b");
        assert!(!dir.join("etc/thinframe.conf.tmp").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn save_overrides_follows_a_symlink_to_a_missing_target() {
        let dir = std::env::temp_dir().join(format!("tfctl-cfg-dangling-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("data/etc")).unwrap();
        fs::create_dir_all(dir.join("etc")).unwrap();
        let real = dir.join("data/etc/thinframe.conf");
        let link = dir.join("etc/thinframe.conf");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        // Relative link one level further, like `ln -s ../data/etc/thinframe.conf`.
        let rel = dir.join("etc/rel.conf");
        std::os::unix::fs::symlink("../data/etc/thinframe.conf", &rel).unwrap();
        save_overrides(rel.to_str().unwrap(), &[("FRAME_ID", "k")]).unwrap();
        assert!(
            real.is_file(),
            "written at the target, not next to the link"
        );
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(!dir.join("etc/rel.conf.tmp").exists());
        assert_eq!(load(link.to_str().unwrap()).unwrap().frame_id, "k");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rotation_outside_0_to_3_is_ignored() {
        assert_eq!(parse("COG_ROTATION=7\n").rotation, 3);
        assert_eq!(parse("COG_ROTATION=x\n").rotation, 3);
        assert_eq!(parse("COG_ROTATION=2\n").rotation, 2);
    }

    #[test]
    fn parses_quoted_values_and_comments() {
        let c = parse("# x\nFRAME_URL=\"http://a/\"\nCOG_ROTATION=1\nCOG_EXTRA_ARGS=\"--a --b\"\n");
        assert_eq!(c.url, "http://a/");
        assert_eq!(c.rotation, 1);
        assert_eq!(c.extra_args, vec!["--a", "--b"]);
        assert_eq!(c.platform, "drm");
    }

    #[test]
    fn splash_keys() {
        let c = parse("SPLASH_BG=\"#102030\"\nSPLASH_TEXT=\"kitchen\"\n");
        assert_eq!((c.splash_bg, c.splash_text.as_str()), (0x102030, "kitchen"));
        let d = parse("SPLASH_BG=nope\n");
        assert_eq!(
            (d.splash_bg, d.splash_text.as_str()),
            (0x0b0d10, "thinframe")
        );
        assert_eq!(parse_rgb("ffffff"), Some(0xffffff));
        assert_eq!(parse_rgb("#fff"), None);
    }

    /// Found by fuzz.rs: `COG_ROTATION=200` was kept and handed to cog as `rotation=200`.
    #[test]
    fn rotation_out_of_range_keeps_the_default() {
        assert_eq!(parse("COG_ROTATION=200\n").rotation, 3);
        assert_eq!(parse("COG_ROTATION=4\n").rotation, 3);
        assert_eq!(parse("COG_ROTATION=0\n").rotation, 0);
    }

    #[test]
    fn server_url_derives_frame_url() {
        let c = parse("SERVER_URL=http://nas:8080/\nFRAME_ID=kitchen\n");
        assert_eq!(c.url, "http://nas:8080/frame/kitchen");
    }
}
