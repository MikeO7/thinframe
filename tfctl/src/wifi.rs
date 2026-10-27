//! Write an iwd profile and reconnect (TASK 7).
//!   /var/lib/iwd/<SSID>.psk, mode 0600:
//!     [Security]\nPassphrase=<psk>\n\n[Settings]\nAutoConnect=true\n
//!   then `/etc/init.d/S40iwd stop` + `start`, `iwctl station wlan0 connect <SSID>`, print `iwctl ... show`.
//! Also the radio as the on-screen Wi-Fi setup sees it (TASK 17, wifisetup.rs): state, scan,
//! network list and join, all through `iwctl` (no D-Bus client in tfctl).
use std::fs;
use std::io::{self, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const IWD_DIR: &str = "/var/lib/iwd";
pub const STATION: &str = "wlan0";

pub fn profile(psk: &str) -> String {
    let psk = escape_value(psk);
    format!("[Security]\nPassphrase={psk}\n\n[Settings]\nAutoConnect=true\n")
}

/// A value for iwd's settings parser (`ell/settings.c` in the iwd-3.0 tarball): `\`
/// starts an escape and blanks after `=` are skipped, so `\` becomes `\\` and a leading space
/// `\s`. A trailing space is `\s` too, in case a parser trims the end. Control characters never
/// get here (`valid_psk`).
pub fn escape_value(v: &str) -> String {
    let mut out = v.replace('\\', "\\\\");
    if out.starts_with(' ') {
        out.replace_range(..1, "\\s");
    }
    if out.ends_with(' ') {
        out.replace_range(out.len() - 1.., "\\s");
    }
    out
}

/// A WPA passphrase as iwd 3.0 accepts it (`crypto_passphrase_is_valid`, IEEE 802.11 Annex M):
/// 8 to 63 bytes, each printable ASCII 0x20..=0x7e.
pub fn valid_psk(psk: &str) -> bool {
    (8..=63).contains(&psk.len()) && psk.bytes().all(|b| (0x20..=0x7e).contains(&b))
}

/// An SSID we put on an iwctl command line and in a file name: 1 to 32 bytes, no control
/// characters (bytes below 0x20, and 0x7f).
pub fn valid_ssid(ssid: &str) -> bool {
    (1..=32).contains(&ssid.len()) && !ssid.bytes().any(|b| b < 0x20 || b == 0x7f)
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
    configured_ssids(dir).into_iter().next()
}

/// Every saved network's SSID.
pub fn configured_ssids(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok()?.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names.iter().filter_map(|n| ssid_of(n)).collect()
}

/// The strongest network in range that has a saved profile.
pub fn pick_known(nets: &[Net], known: &[String]) -> Option<String> {
    nets.iter()
        .filter(|n| known.contains(&n.ssid))
        .max_by_key(|n| n.dbm.unwrap_or(i32::MIN))
        .map(|n| n.ssid.clone())
}

pub fn write_profile(dir: &Path, ssid: &str, psk: &str) -> io::Result<()> {
    if !valid_ssid(ssid) || !valid_psk(psk) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "wifi: bad SSID or passphrase",
        ));
    }
    fs::create_dir_all(dir)?;
    // Mode 0600 from the first byte: the profile holds the passphrase. Written beside it under a
    // name iwd does not read, synced and renamed, so a power cut leaves the old profile or the
    // new one: a profile cut in half is a frame that never joins again.
    let path = dir.join(profile_name(ssid));
    let tmp = dir.join(format!(".{}.tmp", profile_name(ssid)));
    crate::config::write_private(&tmp, profile(psk).as_bytes())?;
    fs::rename(&tmp, &path)?;
    crate::update::sync_dir(dir);
    Ok(())
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
    if !valid_ssid(ssid) || !valid_psk(psk) {
        return Err(io::Error::other(
            "wifi: SSID 1..32 bytes without control characters, PSK 8..63 printable ASCII chars",
        ));
    }
    write_profile(Path::new(IWD_DIR), ssid, psk)?;
    // Buildroot 2025.02's S40iwd has only start and stop.
    let _ = sh("/etc/init.d/S40iwd", &["stop"]);
    sh("/etc/init.d/S40iwd", &["start"])?;
    std::thread::sleep(std::time::Duration::from_secs(2));
    sh("iwctl", &connect_args(ssid, "psk"))?;
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
///
/// iwd 3.0 (`client/station.c`, `display.c`) prints a 2-char margin, a 2-wide column holding
/// "> " for the current network, then the name in a 32-wide column. The name column starts at
/// the header's "Network name" offset, so the `>` is looked for only left of it and a name's
/// leading spaces or `>` survive. Columns after the name are padded by display width (wcwidth,
/// locale dependent), so they are not at fixed character offsets after a non-ASCII name: the
/// signal is the last word and the security the word before ("wep (unsupported)" is two).
/// Trailing spaces in a name cannot be told from the padding: iwctl's table loses them.
/// Signal is dBm, or dBm x 100 as iwd reports it. The same name with two securities is two rows.
pub fn parse_networks(text: &str) -> Vec<Net> {
    let text = strip_ansi(text);
    let mut nets: Vec<Net> = Vec::new();
    let mut name_col: Option<usize> = None;
    for line in text.lines() {
        let Some(col) = name_col else {
            if let Some(i) = line.find("Network name") {
                name_col = Some(line[..i].chars().count());
            }
            continue;
        };
        let t = line.trim();
        if t.is_empty() || t.starts_with('-') || t.starts_with("No networks") {
            continue;
        }
        let margin: String = line.chars().take(col).collect();
        let current = margin.contains('>');
        let rest: String = line.chars().skip(col).collect();
        let Some((rest, signal)) = rest.trim_end().rsplit_once(' ') else {
            continue;
        };
        let rest = rest.trim_end();
        let (name, security) = match rest.strip_suffix(" wep (unsupported)") {
            Some(n) => (n, "wep"),
            None => match rest.rsplit_once(' ') {
                Some(p) => p,
                None => continue,
            },
        };
        let ssid = name.trim_end().to_string();
        if ssid.trim().is_empty()
            || nets
                .iter()
                .any(|n| n.ssid == ssid && n.security == security)
        {
            continue;
        }
        let dbm = signal
            .parse::<i32>()
            .ok()
            .map(|v| if v.unsigned_abs() > 200 { v / 100 } else { v });
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

/// The station, as the on-screen setup needs it. A trait so the setup is testable off the frame.
pub trait Radio: Send + Sync {
    /// There is a Wi-Fi interface at all (not on the Mac, not in a QEMU without hwsim).
    fn present(&self) -> bool;
    fn connected(&self) -> bool;
    /// Joined to an access point (link up), with or without an address yet.
    fn associated(&self) -> bool {
        false
    }
    /// Ask for a fresh scan, wait for it, return what is in range.
    fn scan(&self) -> Vec<Net>;
    /// Join `ssid` (passphrase for psk, None for open); the error is what to show.
    fn join(&self, ssid: &str, psk: Option<&str>) -> Result<(), String>;
    /// Ask the station to connect now to the strongest saved network in range (no profile
    /// change); returns once asked, not once joined.
    fn reconnect(&self) -> Result<(), String> {
        Err("not supported".into())
    }
}

/// `dev` holds an IPv4 address (the kernel's local table in /proc/net/fib_trie is per-address,
/// not per-device, so ask `ip`).
fn has_ipv4(dev: &str) -> bool {
    Command::new("ip")
        .args(["-4", "-o", "addr", "show", "dev", dev])
        .output()
        .map(|o| parse_has_inet(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or(false)
}

pub fn parse_has_inet(text: &str) -> bool {
    text.split_whitespace().any(|w| w == "inet")
}

/// The real radio: wlan0 through iwctl.
pub struct Iwctl;

const JOIN_TIMEOUT: Duration = Duration::from_secs(40);
const KICK_TIMEOUT: Duration = Duration::from_secs(20);

/// Run `iwctl args`, killing it after `limit`; (success, stdout + stderr).
pub fn iwctl(args: &[&str], limit: Duration) -> io::Result<(bool, String)> {
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
    /// wlan0, or the Wi-Fi chip on SDIO before its driver made wlan0: under a boot that no longer
    /// waits for udev the agent started at 2.5 s and wlan0 came at 2.7 s, so it took "no radio",
    /// met "network unreachable" and backed off until 8 s [V 2026-10-22].
    fn present(&self) -> bool {
        Path::new("/sys/class/net").join(STATION).exists()
            || fs::read_dir("/sys/bus/sdio/devices").is_ok_and(|mut d| d.next().is_some())
    }

    /// Online means an address on wlan0, not iwd's "connected": that comes before DHCP, and
    /// the agent then met "network unreachable", rediscovered and moved its server back and
    /// forth [V 2026-10-22]. iwctl also once said "not connected" over a working link.
    fn connected(&self) -> bool {
        has_ipv4(STATION)
    }

    /// The kernel's link state: "up" once iwd's join (and its key handshake) is done.
    fn associated(&self) -> bool {
        fs::read_to_string(Path::new("/sys/class/net").join(STATION).join("operstate"))
            .is_ok_and(|s| s.trim() == "up")
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

    /// In the background: the agent calls this from its server loop, which must not stall.
    fn reconnect(&self) -> Result<(), String> {
        std::thread::Builder::new()
            .name("wifi-rejoin".into())
            .spawn(move || {
                let known = configured_ssids(Path::new(IWD_DIR));
                let listed = || {
                    iwctl(
                        &["station", STATION, "get-networks", "rssi-dbms"],
                        Duration::from_secs(5),
                    )
                    .map(|(_, out)| parse_networks(&out))
                    .unwrap_or_default()
                };
                let mut pick = pick_known(&listed(), &known);
                if pick.is_none() {
                    let _ = iwctl(&["station", STATION, "scan"], Duration::from_secs(5));
                    std::thread::sleep(Duration::from_secs(4));
                    pick = pick_known(&listed(), &known);
                }
                let Some(ssid) = pick else {
                    eprintln!("tfctl: wifi: rejoin: no saved network in range");
                    return;
                };
                // An explicit connect also tries access points iwd's autoconnect has set aside.
                let r = iwctl(&["station", STATION, "connect", &ssid], KICK_TIMEOUT);
                let why = match r {
                    Ok((true, _)) => "joined".to_string(),
                    Ok((false, out)) => out.trim().lines().last().unwrap_or("failed").to_string(),
                    Err(e) => e.to_string(),
                };
                eprintln!("tfctl: wifi: rejoin {ssid:?}: {why}");
            })
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    fn join(&self, ssid: &str, psk: Option<&str>) -> Result<(), String> {
        // The passphrase never goes on a command line (it would sit in /proc/<pid>/cmdline for
        // the whole join): it goes in the 0600 profile, which iwd picks up through its watch on
        // IWD_DIR (knownnetworks.c). Order matters: forget first (a wrong profile from
        // provision.txt would otherwise stay), since forgetting deletes the file; then write;
        // then connect, naming the security so a same-name open network is never picked.
        let _ = iwctl(&["known-networks", ssid, "forget"], Duration::from_secs(5));
        let security = match psk {
            Some(p) => {
                write_profile(Path::new(IWD_DIR), ssid, p).map_err(|e| e.to_string())?;
                wait_known(ssid);
                "psk"
            }
            None => "open",
        };
        let res = match iwctl(&connect_args(ssid, security), JOIN_TIMEOUT) {
            Ok((true, _)) => Ok(()),
            Ok((false, out)) => Err(out.trim().lines().last().unwrap_or("failed").to_string()),
            Err(e) => Err(e.to_string()),
        };
        if res.is_err() && psk.is_some() {
            // As before, a failed join leaves no profile: iwd would retry a wrong password on
            // every boot, and the setup page would wait GRACE for it.
            let _ = iwctl(
                &["known-networks", &format!("{ssid}.psk"), "forget"],
                Duration::from_secs(5),
            );
            let _ = fs::remove_file(Path::new(IWD_DIR).join(profile_name(ssid)));
        }
        res
    }
}

/// The network in `iwctl station wlan0 show` ("Connected network  Name"), if any.
pub fn parse_connected(text: &str) -> Option<String> {
    strip_ansi(text).lines().find_map(|l| {
        let name = l.trim().strip_prefix("Connected network")?.trim();
        (!name.is_empty()).then(|| name.to_string())
    })
}

/// The network the station is on now (None off the frame, or not connected).
pub fn connected_ssid() -> Option<String> {
    let (_, out) = iwctl(&["station", STATION, "show"], Duration::from_secs(3)).ok()?;
    parse_connected(&out)
}

/// Forget a saved network: iwd's entry and our profile file.
pub fn forget(ssid: &str) {
    let _ = iwctl(&["known-networks", ssid, "forget"], Duration::from_secs(5));
    let _ = fs::remove_file(Path::new(IWD_DIR).join(profile_name(ssid)));
}

/// `iwctl station wlan0 connect <name> <security>` (iwd 3.0: `<"network name"> [security]`).
pub fn connect_args<'a>(ssid: &'a str, security: &'a str) -> [&'a str; 5] {
    ["station", STATION, "connect", ssid, security]
}

/// Wait up to 5 s for iwd to load the profile just written (`known-networks <ssid>.psk show`
/// succeeds once it has); connecting before that would make iwctl ask for a passphrase.
fn wait_known(ssid: &str) {
    let name = format!("{ssid}.psk");
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        if let Ok((true, _)) = iwctl(&["known-networks", &name, "show"], Duration::from_secs(3)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_strongest_saved_network() {
        let n = |ssid: &str, dbm: Option<i32>| Net {
            ssid: ssid.into(),
            security: "psk".into(),
            dbm,
            current: false,
        };
        let nets = [
            n("Cafe", Some(-30)),
            n("Home", Some(-70)),
            n("Phone", Some(-50)),
        ];
        let known = vec!["Home".to_string(), "Phone".to_string()];
        assert_eq!(pick_known(&nets, &known).as_deref(), Some("Phone"));
        assert_eq!(pick_known(&nets[..2], &known).as_deref(), Some("Home"));
        assert_eq!(pick_known(&nets[..1], &known), None);
        assert_eq!(
            pick_known(&[n("Home", None)], &known).as_deref(),
            Some("Home")
        );
    }

    #[test]
    fn inet_lines() {
        assert!(parse_has_inet(
            "3: wlan0    inet 192.168.1.108/24 brd 192.168.1.255 scope global wlan0\n"
        ));
        assert!(!parse_has_inet(""));
    }
    use std::os::unix::fs::PermissionsExt;

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

    /// Found by fuzz.rs: a signal of i32::MIN made `v.abs()` overflow (a panic in debug).
    #[test]
    fn extreme_signal_values_do_not_overflow() {
        let t = format!(
            "      Network name   Security  Signal\n      A   psk  -2147483648\n      B   psk  {}\n",
            i32::MAX
        );
        let n = parse_networks(&t);
        assert_eq!(n.len(), 2);
        assert!(n.iter().any(|x| x.dbm == Some(i32::MIN / 100)));
        assert!(n.iter().any(|x| x.dbm == Some(i32::MAX / 100)));
    }

    /// A row as iwd 3.0 prints it: `display_table_row(MARGIN, 4, 2, mark, 32, name, 18, type,
    /// 6, signal)`, each entry "%-*s  " after a 2-space margin; colours as iwctl adds them.
    fn row(current: bool, name: &str, security: &str, signal: &str) -> String {
        let mark = if current { "\x1b[1;90m> \x1b[0m" } else { "" };
        let w = if current { 2 + 11 } else { 2 };
        format!("  {mark:<w$}  {name:<32}  {security:<18}  {signal:<6}  \n")
    }

    #[test]
    fn parses_names_by_column() {
        let header = format!(
            "\x1b[1;90m  {:<2}  {:<32}  {:<18}  {:>6}\x1b[0m\n",
            "", "Network name", "Security", "Signal"
        );
        let dash = "-".repeat(80);
        let text = format!(
            "{dash}\n{header}{dash}\n{}{}{}{}{}{}{}{dash}\n",
            row(true, ">arrow", "psk", "-4000"),
            row(false, " Lead space", "psk", "-5000"),
            row(false, "Trail  ", "open", "-6000"),
            row(false, "Café 5G", "psk", "-6100"),
            row(false, "Same", "open", "-7000"),
            row(false, "Same", "psk", "-7100"),
            row(false, "Old Box", "wep (unsupported)", "-8000"),
        );
        let n = parse_networks(&text);
        let got: Vec<(&str, &str, bool)> = n
            .iter()
            .map(|n| (n.ssid.as_str(), n.security.as_str(), n.current))
            .collect();
        assert_eq!(
            got,
            [
                (">arrow", "psk", true),
                (" Lead space", "psk", false),
                // The table pads with spaces: a name's trailing spaces are lost in iwctl.
                ("Trail", "open", false),
                ("Café 5G", "psk", false),
                ("Same", "open", false),
                ("Same", "psk", false),
                ("Old Box", "wep", false),
            ]
        );
        assert!(!n[6].supported());
    }

    #[test]
    fn profile_escapes_for_iwd() {
        assert_eq!(escape_value("plain pass"), "plain pass");
        assert_eq!(escape_value(r"a\b"), r"a\\b");
        assert_eq!(escape_value(" lead"), r"\slead");
        assert_eq!(escape_value("trail "), r"trail\s");
        assert_eq!(escape_value("  two  "), r"\s two \s");
        assert_eq!(
            escape_value(r"\s"),
            r"\\s",
            "a literal backslash-s stays literal"
        );
        assert_eq!(
            profile(r" x\y "),
            "[Security]\nPassphrase=\\sx\\\\y\\s\n\n[Settings]\nAutoConnect=true\n"
        );
    }

    #[test]
    fn validates_passphrase_and_ssid() {
        assert!(valid_psk("12345678") && valid_psk(&"~".repeat(63)));
        assert!(valid_psk(" spaces are fine "));
        assert!(!valid_psk("1234567") && !valid_psk(&"x".repeat(64)));
        assert!(!valid_psk("pass\nword") && !valid_psk("pass\x7fword"));
        assert!(!valid_psk("pässword"), "printable ASCII only");
        assert!(!valid_psk("12345678\0"));
        assert!(valid_ssid("Café") && valid_ssid(" x ") && valid_ssid(&"s".repeat(32)));
        assert!(!valid_ssid("") && !valid_ssid(&"s".repeat(33)));
        assert!(!valid_ssid("a\nb") && !valid_ssid("a\tb") && !valid_ssid("a\x7f"));
        let dir = std::env::temp_dir().join(format!("tfctl-iwd-bad-{}", std::process::id()));
        assert!(write_profile(&dir, "Home", "bad\npass\nword").is_err());
        assert!(write_profile(&dir, "Ho\nme", "goodpassword").is_err());
        assert!(!dir.exists(), "nothing written");
        assert!(run("Home", "short").is_err());
    }

    #[test]
    fn connect_never_carries_the_passphrase() {
        let a = connect_args("Groovy Wifi", "psk");
        assert_eq!(a, ["station", "wlan0", "connect", "Groovy Wifi", "psk"]);
        assert!(!a.contains(&"--passphrase"));
    }

    #[test]
    fn strips_ansi() {
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
