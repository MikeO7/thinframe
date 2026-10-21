//! Write an iwd profile and reconnect (TASK 7).
//!   /var/lib/iwd/<SSID>.psk, mode 0600:
//!     [Security]\nPassphrase=<psk>\n\n[Settings]\nAutoConnect=true\n
//!   then `/etc/init.d/S40iwd stop` + `start`, `iwctl station wlan0 connect <SSID>`, print `iwctl ... show`.
//! Also the radio as the on-screen Wi-Fi setup sees it (TASK 17, wifisetup.rs): state, scan,
//! network list and join, all through `iwctl` (no D-Bus client in tfctl).
use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

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
    // Buildroot 2025.02's S40iwd has only start and stop.
    let _ = sh("/etc/init.d/S40iwd", &["stop"]);
    sh("/etc/init.d/S40iwd", &["start"])?;
    std::thread::sleep(std::time::Duration::from_secs(2));
    sh("iwctl", &["station", STATION, "connect", ssid])?;
    print!("{}", sh("iwctl", &["station", STATION, "show"])?);
    Ok(())
}

/// One network from `iwctl station wlan0 get-networks rssi-dbms`.
#[derive(Debug, Clone, PartialEq)]
pub struct Net {
    pub ssid: String,
    /// iwd's word: "psk", "open", "8021x", "wep".
    pub security: String,
    pub dbm: Option<i32>,
    /// The `>` mark: the network the station is on.
    pub current: bool,
}

impl Net {
    /// Signal as 1..4 bars, like a phone.
    pub fn bars(&self) -> u8 {
        match self.dbm {
            Some(d) if d >= -55 => 4,
            Some(d) if d >= -67 => 3,
            Some(d) if d >= -78 => 2,
            _ => 1,
        }
    }

    /// The setup page can join psk and open networks; enterprise and WEP need a profile by hand.
    pub fn supported(&self) -> bool {
        self.security == "psk" || self.security == "open"
    }
}

/// iwctl colours its tables even without a terminal: drop CSI escapes (ESC [ ... final byte).
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' {
            if it.peek() == Some(&'[') {
                it.next();
                for f in it.by_ref() {
                    if ('@'..='~').contains(&f) {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// `get-networks rssi-dbms` table: rows under the header, "  >   Name    psk    -5400".
/// The last two words are security and signal; the name is everything before them, so names
/// with spaces survive. Signal is dBm, or dBm x 100 as iwd reports it.
pub fn parse_networks(text: &str) -> Vec<Net> {
    let text = strip_ansi(text);
    let mut nets: Vec<Net> = Vec::new();
    let mut in_rows = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("Network name") {
            in_rows = true;
            continue;
        }
        if !in_rows || t.is_empty() || t.starts_with('-') || t.starts_with("No networks") {
            continue;
        }
        let (current, t) = match t.strip_prefix('>') {
            Some(rest) => (true, rest.trim_start()),
            None => (false, t),
        };
        let Some((rest, signal)) = t.rsplit_once(char::is_whitespace) else {
            continue;
        };
        let Some((name, security)) = rest.trim_end().rsplit_once(char::is_whitespace) else {
            continue;
        };
        let ssid = name.trim_end().to_string();
        if ssid.is_empty() || nets.iter().any(|n| n.ssid == ssid) {
            continue;
        }
        let dbm = signal
            .parse::<i32>()
            .ok()
            .map(|v| if v.abs() > 200 { v / 100 } else { v });
        nets.push(Net {
            ssid,
            security: security.to_string(),
            dbm,
            current,
        });
    }
    nets.sort_by_key(|n| std::cmp::Reverse(n.dbm.unwrap_or(-100)));
    nets
}

/// `iwctl station wlan0 show`: the word after "State" ("connected", "disconnected", ...).
pub fn parse_state(text: &str) -> Option<String> {
    strip_ansi(text)
        .lines()
        .map(str::split_whitespace)
        .find_map(|mut w| (w.next() == Some("State")).then(|| w.next()).flatten())
        .map(str::to_string)
}

/// The station, as the on-screen setup needs it. A trait so the setup is testable off the frame.
pub trait Radio: Send + Sync {
    /// There is a Wi-Fi interface at all (not on the Mac, not in a QEMU without hwsim).
    fn present(&self) -> bool;
    fn connected(&self) -> bool;
    /// Ask for a fresh scan, wait for it, return what is in range.
    fn scan(&self) -> Vec<Net>;
    /// Join `ssid` (passphrase for psk, None for open); the error is what to show.
    fn join(&self, ssid: &str, psk: Option<&str>) -> Result<(), String>;
}

/// The real radio: wlan0 through iwctl.
pub struct Iwctl;

const JOIN_TIMEOUT: Duration = Duration::from_secs(40);

/// Run `iwctl args`, killing it after `limit`; (success, stdout + stderr).
fn iwctl(args: &[&str], limit: Duration) -> io::Result<(bool, String)> {
    let mut child = Command::new("iwctl")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break Some(s);
        }
        if start.elapsed() >= limit {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let mut text = String::new();
    for pipe in [
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn io::Read>),
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn io::Read>),
    ]
    .into_iter()
    .flatten()
    {
        let mut p = pipe;
        let _ = p.read_to_string(&mut text);
    }
    match status {
        Some(s) => Ok((s.success(), strip_ansi(&text))),
        None => Ok((false, "timed out".into())),
    }
}

impl Radio for Iwctl {
    fn present(&self) -> bool {
        Path::new("/sys/class/net").join(STATION).exists()
    }

    fn connected(&self) -> bool {
        iwctl(&["station", STATION, "show"], Duration::from_secs(5))
            .map(|(_, out)| parse_state(&out).as_deref() == Some("connected"))
            .unwrap_or(false)
    }

    fn scan(&self) -> Vec<Net> {
        let _ = iwctl(&["station", STATION, "scan"], Duration::from_secs(5));
        std::thread::sleep(Duration::from_secs(4));
        iwctl(
            &["station", STATION, "get-networks", "rssi-dbms"],
            Duration::from_secs(5),
        )
        .map(|(_, out)| parse_networks(&out))
        .unwrap_or_default()
    }

    fn join(&self, ssid: &str, psk: Option<&str>) -> Result<(), String> {
        // A stored profile wins over --passphrase, so a wrong one from provision.txt would
        // never be replaced: forget it first. iwd writes the new profile itself on success.
        let _ = iwctl(&["known-networks", ssid, "forget"], Duration::from_secs(5));
        let mut args = Vec::new();
        if let Some(p) = psk {
            args.extend(["--passphrase", p]);
        }
        args.extend(["station", STATION, "connect", ssid]);
        match iwctl(&args, JOIN_TIMEOUT) {
            Ok((true, _)) => Ok(()),
            Ok((false, out)) => Err(out.trim().lines().last().unwrap_or("failed").to_string()),
            Err(e) => Err(e.to_string()),
        }
    }
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

    // iwd 3 output, colours included (sim/qemu with mac80211_hwsim, TASK 17).
    const NETS: &str = "\x1b[1m                               Available networks\x1b[0m\n\
        --------------------------------------------------------------------------------\n\
        \x1b[1;90m      Network name                      Security            Signal\x1b[0m\n\
        --------------------------------------------------------------------------------\n\
        \x1b[1;90m  > \x1b[0m  Groovy Wifi                       psk                 -3000\n\
        \x1b[0m      Cafe                              open                -8100\n\
        \x1b[0m      Office  2                         8021x               -6000\n\
        \x1b[0m      Cafe                              open                -9000\n\n";

    #[test]
    fn parses_network_table() {
        let n = parse_networks(NETS);
        assert_eq!(n.len(), 3, "{n:?}");
        assert_eq!(n[0].ssid, "Groovy Wifi");
        assert!(n[0].current && n[0].supported());
        assert_eq!(n[0].dbm, Some(-30));
        assert_eq!(n[0].bars(), 4);
        assert_eq!(n[1].ssid, "Office  2");
        assert!(!n[1].supported());
        assert_eq!(
            (n[2].ssid.as_str(), n[2].security.as_str()),
            ("Cafe", "open")
        );
        assert_eq!(n[2].bars(), 1);
        assert!(parse_networks("").is_empty());
        assert!(
            parse_networks("      Network name   Security  Signal\n  No networks available\n")
                .is_empty()
        );
    }

    #[test]
    fn parses_station_state() {
        let show = "\x1b[1m  Station: wlan0\x1b[0m\n----\n  Settable  Property  Value\n----\n            Scanning  no\n            State     \x1b[32mconnected\x1b[0m\n";
        assert_eq!(parse_state(show).as_deref(), Some("connected"));
        assert_eq!(
            parse_state("  State  connected\n").as_deref(),
            Some("connected")
        );
        assert_eq!(
            parse_state("            State     disconnected\n").as_deref(),
            Some("disconnected")
        );
        assert_eq!(strip_ansi("a\x1b[1;90mb\x1b[0mc"), "abc");
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
