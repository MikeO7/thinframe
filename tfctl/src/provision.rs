//! `tfctl provision [--boot-dir DIR]` (TASK 14): first-boot settings from `provision.txt` on
//! the PFBOOT partition, written by `scripts/flash.sh` (format: image/boot/provision.txt.example).
//!
//! Writes, in order: the iwd profile (`wifi.rs`, no service restart: iwd starts after us),
//! `/etc/thinframe.conf` overrides, `authorized_keys` for the `frame` user and for dropbear,
//! the hostname, then deletes `provision.txt`. The FAT partition is mounted rw at /boot only
//! while this runs (the init script mounts it before and unmounts after). Absent file = exit 0.
//!
//! Dropbear's authorized_keys: the image bind-mounts `/data/ssh` over `/etc/dropbear`, so the
//! key is written to `/data/ssh/authorized_keys` (parameter `dropbear_keys`); the frame user's
//! `~/.ssh/authorized_keys` is written too, for whichever path the image ends up reading.
use crate::config;
use crate::wifi;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const FILE: &str = "provision.txt";

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Provision {
    pub wifi_ssid: Option<String>,
    pub wifi_psk: Option<String>,
    pub server_url: Option<String>,
    pub frame_id: Option<String>,
    pub frame_token: Option<String>,
    pub rotation: Option<u8>,
    pub ssh_pubkey: Option<String>,
}

/// KEY=VALUE, quotes optional, `#` comments. Unknown keys are ignored.
pub fn parse(text: &str) -> Provision {
    let mut p = Provision::default();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let Some((k, v)) = t.split_once('=') else {
            continue;
        };
        let v = v.trim();
        let v = v
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .or_else(|| v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
            .unwrap_or(v)
            .to_string();
        let some = (!v.is_empty()).then_some(v);
        match k.trim() {
            "WIFI_SSID" => p.wifi_ssid = some,
            "WIFI_PSK" => p.wifi_psk = some,
            "SERVER_URL" => p.server_url = some.map(|s| s.trim_end_matches('/').to_string()),
            "FRAME_ID" => p.frame_id = some.filter(|s| valid_hostname(s)),
            "FRAME_TOKEN" => p.frame_token = some,
            "COG_ROTATION" => p.rotation = some.and_then(|s| s.parse().ok()).filter(|r| *r < 4),
            "SSH_PUBKEY" => {
                p.ssh_pubkey = some.filter(|s| s.starts_with("ssh-") || s.starts_with("ecdsa-"))
            }
            _ => {}
        }
    }
    p
}

/// RFC 1123 label: letters, digits, hyphens, 1..63 chars, no leading/trailing hyphen.
pub fn valid_hostname(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}

/// Where the settings go. `Targets::system()` on the frame.
#[derive(Debug, Clone)]
pub struct Targets {
    pub boot_dir: PathBuf,
    pub iwd_dir: PathBuf,
    pub conf: PathBuf,
    /// `~frame/.ssh/authorized_keys`
    pub user_keys: PathBuf,
    /// `/data/ssh/authorized_keys`, bind-mounted over /etc/dropbear by the image.
    pub dropbear_keys: PathBuf,
    /// Where to keep the name; None on the frame, whose root is read-only: `tfctl boot` sets the
    /// hostname from FRAME_ID in thinframe.conf on every boot instead.
    pub hostname_file: Option<PathBuf>,
    /// Run `hostname NAME` too (only on the frame).
    pub set_hostname: bool,
}

impl Targets {
    pub fn system(boot_dir: &str) -> Targets {
        Targets {
            boot_dir: boot_dir.into(),
            iwd_dir: wifi::IWD_DIR.into(),
            conf: config::path().into(),
            user_keys: "/home/frame/.ssh/authorized_keys".into(),
            dropbear_keys: "/data/ssh/authorized_keys".into(),
            hostname_file: None,
            set_hostname: true,
        }
    }
    /// Everything under one temp dir, for tests.
    #[cfg(test)]
    pub fn under(base: &Path) -> Targets {
        Targets {
            boot_dir: base.join("boot"),
            iwd_dir: base.join("iwd"),
            conf: base.join("thinframe.conf"),
            user_keys: base.join("home/.ssh/authorized_keys"),
            dropbear_keys: base.join("data/ssh/authorized_keys"),
            hostname_file: Some(base.join("hostname")),
            set_hostname: false,
        }
    }
}

/// Add `key` to an authorized_keys file (dir 0755, file 0644, root-owned) unless it is already
/// there. Dropbear opens the file as the logging-in user, and refuses group/world-writable ones.
pub fn add_authorized_key(path: &Path, key: &str) -> io::Result<()> {
    let dir = path.parent().unwrap();
    fs::create_dir_all(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o755))?;
    let mut cur = fs::read_to_string(path).unwrap_or_default();
    if cur.lines().any(|l| l.trim() == key.trim()) {
        return Ok(());
    }
    if !cur.is_empty() && !cur.ends_with('\n') {
        cur.push('\n');
    }
    cur.push_str(key.trim());
    cur.push('\n');
    fs::write(path, cur)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o644))
}

/// Apply `provision.txt` if present. Returns false when there was nothing to do.
pub fn apply(t: &Targets) -> io::Result<bool> {
    let file = t.boot_dir.join(FILE);
    let text = match fs::read_to_string(&file) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let res = apply_text(t, &text);
    // The file holds the Wi-Fi passphrase and the token on a FAT partition anyone can read
    // by pulling the card: it goes whether or not applying it worked, and a failure is
    // reported once rather than retried on every boot.
    if file.exists() {
        if let Err(e) = fs::remove_file(&file) {
            eprintln!("tfctl: provision: cannot remove {}: {e}", file.display());
        }
    }
    res
}

fn apply_text(t: &Targets, text: &str) -> io::Result<bool> {
    let file = t.boot_dir.join(FILE);
    let p = parse(text);
    if let (Some(ssid), Some(psk)) = (&p.wifi_ssid, &p.wifi_psk) {
        if !wifi::valid_ssid(ssid) {
            eprintln!("tfctl: provision: WIFI_SSID has control characters or is over 32 bytes, skipping Wi-Fi");
        } else if !wifi::valid_psk(psk) {
            eprintln!(
                "tfctl: provision: WIFI_PSK must be 8..63 printable ASCII chars, skipping Wi-Fi"
            );
        } else {
            wifi::write_profile(&t.iwd_dir, ssid, psk)?;
            eprintln!("tfctl: provision: wifi profile for {ssid}");
        }
    }
    let rot = p.rotation.map(|r| r.to_string());
    let mut kv: Vec<(&str, &str)> = Vec::new();
    for (k, v) in [
        ("SERVER_URL", &p.server_url),
        ("FRAME_ID", &p.frame_id),
        ("FRAME_TOKEN", &p.frame_token),
        ("COG_ROTATION", &rot),
    ] {
        if let Some(v) = v {
            kv.push((k, v.as_str()));
        }
    }
    if !kv.is_empty() {
        if let Some(d) = t.conf.parent() {
            fs::create_dir_all(d)?;
        }
        config::save_overrides(t.conf.to_str().unwrap(), &kv)?;
        eprintln!(
            "tfctl: provision: {} keys into {}",
            kv.len(),
            t.conf.display()
        );
    }
    if let Some(key) = &p.ssh_pubkey {
        for path in [&t.user_keys, &t.dropbear_keys] {
            if let Err(e) = add_authorized_key(path, key) {
                eprintln!("tfctl: provision: {}: {e}", path.display());
            }
        }
    }
    if let Some(name) = &p.frame_id {
        if let Some(f) = &t.hostname_file {
            if let Err(e) = fs::write(f, format!("{name}\n")) {
                eprintln!("tfctl: provision: {}: {e}", f.display());
            }
        }
        if t.set_hostname {
            let _ = Command::new("hostname").arg(name).status();
        }
    }
    fs::remove_file(&file)?;
    let _ = Command::new("sync").status();
    eprintln!("tfctl: provision: done, {} removed", file.display());
    Ok(true)
}

/// `tfctl provision [--boot-dir DIR]`
pub fn run(args: &[String]) -> io::Result<()> {
    let mut boot = "/boot".to_string();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--boot-dir" => {
                boot = it
                    .next()
                    .ok_or_else(|| io::Error::other("--boot-dir needs a value"))?
                    .clone()
            }
            other => {
                return Err(io::Error::other(format!(
                    "provision: unknown arg '{other}'"
                )))
            }
        }
    }
    if !apply(&Targets::system(&boot))? {
        eprintln!("tfctl: provision: no {boot}/{FILE}, nothing to do");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = "# Copy to provision.txt\nWIFI_SSID=HomeWifi\nWIFI_PSK=\"correct horse battery staple\"\n\
        SERVER_URL=http://nas.local:8080/\nFRAME_ID=kitchen\nFRAME_TOKEN='dev-token'\n#COG_ROTATION=3\n\
        COG_ROTATION=1\nSSH_PUBKEY=ssh-ed25519 AAAAC3Nza you@mac\nBOGUS=1\n";

    #[test]
    fn parses_example_format() {
        let p = parse(EXAMPLE);
        assert_eq!(p.wifi_ssid.as_deref(), Some("HomeWifi"));
        assert_eq!(p.wifi_psk.as_deref(), Some("correct horse battery staple"));
        assert_eq!(p.server_url.as_deref(), Some("http://nas.local:8080"));
        assert_eq!(p.frame_id.as_deref(), Some("kitchen"));
        assert_eq!(p.frame_token.as_deref(), Some("dev-token"));
        assert_eq!(p.rotation, Some(1));
        assert_eq!(
            p.ssh_pubkey.as_deref(),
            Some("ssh-ed25519 AAAAC3Nza you@mac")
        );
        let bad = parse("FRAME_ID=bad host\nCOG_ROTATION=7\nSSH_PUBKEY=garbage\nWIFI_SSID=\n");
        assert_eq!(bad, Provision::default());
        assert!(valid_hostname("kitchen-2"));
        assert!(!valid_hostname("-x") && !valid_hostname("a_b") && !valid_hostname(""));
    }

    #[test]
    fn applies_everything_then_deletes_and_is_idempotent() {
        let base = std::env::temp_dir().join(format!("tfctl-prov-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let t = Targets::under(&base);
        fs::create_dir_all(&t.boot_dir).unwrap();
        assert!(!apply(&t).unwrap(), "absent file: nothing done");
        fs::write(t.boot_dir.join(FILE), EXAMPLE).unwrap();
        fs::write(
            &t.conf,
            "SERVER_URL=\"\"\nFRAME_ID=\"frame\"\nCOG_ROTATION=\"3\"\n",
        )
        .unwrap();
        assert!(apply(&t).unwrap());
        assert!(!t.boot_dir.join(FILE).exists());
        let prof = t.iwd_dir.join("HomeWifi.psk");
        assert_eq!(
            fs::read_to_string(&prof).unwrap(),
            wifi::profile("correct horse battery staple")
        );
        let c = config::load(t.conf.to_str().unwrap()).unwrap();
        assert_eq!(c.server_url, "http://nas.local:8080");
        assert_eq!(c.frame_id, "kitchen");
        assert_eq!(c.token, "dev-token");
        assert_eq!(c.rotation, 1);
        assert_eq!(c.url, "http://nas.local:8080/frame/kitchen");
        for keys in [&t.user_keys, &t.dropbear_keys] {
            assert_eq!(
                fs::read_to_string(keys).unwrap(),
                "ssh-ed25519 AAAAC3Nza you@mac\n"
            );
            assert_eq!(
                fs::metadata(keys).unwrap().permissions().mode() & 0o777,
                0o644
            );
            assert_eq!(
                fs::metadata(keys.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o755
            );
        }
        assert_eq!(
            fs::read_to_string(t.hostname_file.as_ref().unwrap()).unwrap(),
            "kitchen\n"
        );
        // Second run with the same file: same result, key not duplicated.
        fs::write(t.boot_dir.join(FILE), EXAMPLE).unwrap();
        assert!(apply(&t).unwrap());
        assert_eq!(fs::read_to_string(&t.user_keys).unwrap().lines().count(), 1);
        assert!(!apply(&t).unwrap());
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn partial_file_only_touches_given_keys() {
        let base = std::env::temp_dir().join(format!("tfctl-prov2-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let t = Targets::under(&base);
        fs::create_dir_all(&t.boot_dir).unwrap();
        fs::write(
            t.boot_dir.join(FILE),
            "FRAME_TOKEN=abc\nWIFI_SSID=x\nWIFI_PSK=short\n",
        )
        .unwrap();
        assert!(apply(&t).unwrap());
        assert!(!t.iwd_dir.exists(), "short psk rejected");
        assert!(!t.user_keys.exists());
        assert!(!t.hostname_file.as_ref().unwrap().exists());
        let c = config::load(t.conf.to_str().unwrap()).unwrap();
        assert_eq!((c.token.as_str(), c.frame_id.as_str()), ("abc", "frame"));
        assert!(run(&["--bogus".into()]).is_err());
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn wifi_with_control_characters_or_non_ascii_is_skipped() {
        let base = std::env::temp_dir().join(format!("tfctl-prov3-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let t = Targets::under(&base);
        fs::create_dir_all(&t.boot_dir).unwrap();
        for text in [
            "WIFI_SSID=Home\nWIFI_PSK=\"tab\there\"\n",
            "WIFI_SSID=Home\nWIFI_PSK=\"del\x7fhere12\"\n",
            "WIFI_SSID=Home\nWIFI_PSK=pässwörd12\n",
            "WIFI_SSID=\"Ho\x1bme\"\nWIFI_PSK=goodpassword\n",
        ] {
            fs::write(t.boot_dir.join(FILE), text).unwrap();
            assert!(apply(&t).unwrap());
            assert!(!t.iwd_dir.exists(), "rejected: {text:?}");
        }
        // Backslashes and edge spaces are fine; the profile escapes them for iwd.
        fs::write(
            t.boot_dir.join(FILE),
            "WIFI_SSID=Home\nWIFI_PSK=\" back\\slash \"\n",
        )
        .unwrap();
        assert!(apply(&t).unwrap());
        assert_eq!(
            fs::read_to_string(t.iwd_dir.join("Home.psk")).unwrap(),
            wifi::profile(" back\\slash ")
        );
        assert!(fs::read_to_string(t.iwd_dir.join("Home.psk"))
            .unwrap()
            .contains("Passphrase=\\sback\\\\slash\\s\n"));
        fs::remove_dir_all(&base).unwrap();
    }
}
