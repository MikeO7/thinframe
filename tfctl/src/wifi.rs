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

/// iwd escapes SSID bytes that are not [A-Za-z0-9_-] as =XX in the file name.
pub fn profile_name(ssid: &str) -> String {
    let mut out = String::new();
    for b in ssid.bytes() {
        if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' {
            out.push(b as char);
        } else {
            out.push_str(&format!("={b:02x}"));
        }
    }
    out.push_str(".psk");
    out
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
        assert_eq!(profile_name("My Net"), "My=20Net.psk");
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
