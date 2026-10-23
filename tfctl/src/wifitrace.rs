//! Why Wi-Fi dropped, in the log. iwd keeps its reasons to itself (its output goes nowhere on
//! the frame) and the AIC8800 driver logs none, so the kiosk runs `iwmon` (part of iwd, needs
//! root) and copies only the lines that say what happened to the link: connects, disconnects,
//! roams, BSS transition requests, and their reason and status codes. Nothing else of iwmon's
//! output (key exchange frames, scans) is kept. It found why the frame dropped its access point
//! ~31 s after each join (reason 34, fixed with he_on=0 in /etc/modprobe.d) [V 2026-10-22].
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const IWMON: &str = "/usr/bin/iwmon";
/// Lines kept per hour, so a flapping link cannot fill the log.
const PER_HOUR: u32 = 200;
const KEEP: &[&str] = &[
    "Event: Connect",
    "Event: Disconnect",
    "Event: Deauthenticate",
    "Event: Disassociate",
    "Event: Roam",
    "Event: Port Authorized",
    "Request: Connect",
    "Request: Disconnect",
    "Reason Code",
    "Reason code",
    "Status Code",
    "Disconnect by AP",
    "BSS Transition",
];

/// Whether an iwmon line is one of the few worth keeping.
pub fn keep(line: &str) -> bool {
    KEEP.iter().any(|k| line.contains(k))
}

/// Start iwmon in the background, for the life of the kiosk (restarted a few times if it ends).
pub fn start() {
    if !std::path::Path::new(IWMON).exists() {
        return;
    }
    let r = thread::Builder::new().name("wifitrace".into()).spawn(|| {
        for _ in 0..5 {
            if let Err(e) = run() {
                eprintln!("tfctl: wifi trace: {e}");
            }
            thread::sleep(Duration::from_secs(60));
        }
    });
    if let Err(e) = r {
        eprintln!("tfctl: wifi trace: thread: {e}");
    }
}

fn run() -> std::io::Result<()> {
    let mut child = Command::new(IWMON)
        .args(["--nortnl", "--nowiphy", "--noscan", "--noies"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let out = child.stdout.take().expect("piped");
    let (mut hour, mut n) = (Instant::now(), 0u32);
    for line in BufReader::new(out).lines() {
        let line = line?;
        if !keep(&line) {
            continue;
        }
        if hour.elapsed() >= Duration::from_secs(3600) {
            (hour, n) = (Instant::now(), 0);
        }
        n += 1;
        if n <= PER_HOUR {
            let t: String = line.trim().chars().take(160).collect();
            eprintln!("tfctl: wifi trace: {t}");
        } else if n == PER_HOUR + 1 {
            eprintln!("tfctl: wifi trace: more than {PER_HOUR} lines this hour, pausing");
        }
    }
    let _ = child.wait();
    Err(std::io::Error::other("iwmon ended"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_link_events_only() {
        assert!(keep("> Event: Disconnect (0x30) len 40   12.345678"));
        assert!(keep("        Reason Code: 3"));
        assert!(keep("        Disconnect by AP: true"));
        assert!(keep("            Status Code: 0"));
        assert!(!keep("> Event: New Scan Results (0x22) len 28"));
        assert!(!keep("        Key Data: 30 14 01 00"));
        assert!(!keep("        Frame: EAPOL"));
    }
}
