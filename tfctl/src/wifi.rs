//! Write an iwd profile and reconnect (TASK 7).
//!   /var/lib/iwd/<SSID>.psk, mode 0600:
//!     [Security]\nPassphrase=<psk>\n\n[Settings]\nAutoConnect=true\n
//!   then `/etc/init.d/S40iwd restart`, `iwctl station wlan0 connect <SSID>`, print `iwctl ... show`.
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

pub const IWD_DIR: &str = "/var/lib/iwd";
pub const STATION: &str = "wlan0";

pub fn profile(psk: &str) -> String {
    format!("[Security]\nPassphrase={psk}\n\n[Settings]\nAutoConnect=true\n")
}

/// iwd 3.0 `storage_get_network_file_path`: an SSID of only [A-Za-z0-9 _-] is the file name as
/// is; anything else becomes "=" + lowercase hex of the whole SSID.
pub fn profile_name(ssid: &str) -> String {
    let plain = ssid
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"-_ ".contains(&b));
    if plain {
        format!("{ssid}.psk")
    } else {
        let hex: String = ssid.bytes().map(|b| format!("{b:02x}")).collect();
        format!("={hex}.psk")
    }
}

/// The SSID of an iwd profile file name (`Home.psk`, `=4d79....psk`, `Cafe.open`).
pub fn ssid_of(file_name: &str) -> Option<String> {
    let stem = file_name
        .strip_suffix(".psk")
        .or_else(|| file_name.strip_suffix(".open"))?;
    let Some(hex) = stem.strip_prefix('=') else {
        return Some(stem.to_string());
    };
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect();
    Some(String::from_utf8_lossy(&bytes?).into_owned())
}

/// The first configured network in `dir`, for the status page ("joining <ssid>").
pub fn configured_ssid(dir: &Path) -> Option<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .collect();
    names.sort();
    names.iter().find_map(|n| ssid_of(n))
}

pub fn write_profile(dir: &Path, ssid: &str, psk: &str) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let path = dir.join(profile_name(ssid));
    fs::write(&path, profile(psk))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
}

fn sh(cmd: &str, args: &[&str]) -> io::Result<String> {
    let out = Command::new(cmd).args(args).output()?;
    if !out.status.success() {
        return Err(io::Error::other(format!(
            "{cmd} {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn run(ssid: &str, psk: &str) -> io::Result<()> {
    if ssid.is_empty() || psk.len() < 8 || psk.len() > 63 {
        return Err(io::Error::other(
            "wifi: SSID required, PSK must be 8..63 chars",
        ));
    }
    write_profile(Path::new(IWD_DIR), ssid, psk)?;
    sh("/etc/init.d/S40iwd", &["restart"])?;
    std::thread::sleep(std::time::Duration::from_secs(2));
    sh("iwctl", &["station", STATION, "connect", ssid])?;
    print!("{}", sh("iwctl", &["station", STATION, "show"])?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_format_and_names() {
        assert_eq!(
            profile("secret123"),
            "[Security]\nPassphrase=secret123\n\n[Settings]\nAutoConnect=true\n"
        );
        assert_eq!(profile_name("Home_5G-x"), "Home_5G-x.psk");
        assert_eq!(profile_name("My Net"), "My Net.psk");
        assert_eq!(profile_name("Café"), "=436166c3a9.psk");
        assert_eq!(profile_name("a.b"), "=612e62.psk");
    }

    #[test]
    fn ssid_round_trips_through_file_names() {
        for s in ["Home_5G-x", "My Net", "Café", "a.b", "x=y"] {
            assert_eq!(ssid_of(&profile_name(s)).as_deref(), Some(s));
        }
        assert_eq!(ssid_of("Cafe.open").as_deref(), Some("Cafe"));
        assert_eq!(ssid_of("notes.txt"), None);
        assert_eq!(ssid_of("=4.psk"), None);
    }

    #[test]
    fn writes_mode_0600() {
        let dir = std::env::temp_dir().join(format!("tfctl-iwd-{}", std::process::id()));
        write_profile(&dir, "Home", "secret123").unwrap();
        let p = dir.join("Home.psk");
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(fs::read_to_string(&p).unwrap(), profile("secret123"));
        fs::remove_dir_all(&dir).unwrap();
    }
}
