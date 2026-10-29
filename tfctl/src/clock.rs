//! The clock without the server. The server's hello sets the time (agent.rs), but a frame whose
//! server is down booted at 1970 and ran its screen schedule on that. Three sources now: the
//! A33's RTC (the kernel reads it at boot, CONFIG_RTC_HCTOSYS; it keeps time across reboots
//! while powered, not across a power cut), written here whenever the clock is set; the server's
//! hello; and SNTP (RFC 4330, one UDP packet to pool.ntp.org) while the clock is unset, then
//! once a day.
use std::net::UdpSocket;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, SystemTime};

/// Seconds since the epoch at 2024-01-01: anything earlier is not a clock.
pub const CLOCK_MIN: i64 = 1_704_067_200;
/// A time more than this after the build is not a clock either (a bad server, a flipped bit).
const MAX_AFTER_BUILD: i64 = 20 * 365 * 86_400 + 5 * 86_400;

/// When this tfctl was built (build.rs), never before CLOCK_MIN.
fn built() -> i64 {
    env!("TF_BUILD_EPOCH")
        .parse::<i64>()
        .unwrap_or(CLOCK_MIN)
        .max(CLOCK_MIN)
}

/// The latest time taken from any source: 20 years after the build.
pub fn clock_max() -> i64 {
    built() + MAX_AFTER_BUILD
}

/// A time any source may set: from 2024 to 20 years after the build.
fn plausible(t: i64) -> bool {
    (CLOCK_MIN..=clock_max()).contains(&t)
}
const NTP_SERVERS: [&str; 2] = ["pool.ntp.org:123", "time.google.com:123"];
/// Seconds from 1900 (NTP) to 1970 (Unix).
const NTP_UNIX_OFFSET: i64 = 2_208_988_800;

static RTC_WRITTEN: AtomicBool = AtomicBool::new(false);

fn write_rtc() {
    if let Err(e) = Command::new("hwclock").args(["-w", "-u"]).output() {
        eprintln!("tfctl: clock: hwclock -w: {e}");
    }
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The clock is set: later than 2024.
pub fn valid() -> bool {
    now() >= CLOCK_MIN
}

/// `date -s @t` when off by more than 2 s, then the RTC follows. `who` names the source in the log.
pub fn set(t: i64, who: &str) {
    if !plausible(t) {
        eprintln!("tfctl: clock: ignoring {t} from {who}");
        return;
    }
    if (t - now()).abs() <= 2 {
        // Right already; the RTC may still read 1970 (it lost power, or was never written).
        if !RTC_WRITTEN.swap(true, Ordering::Relaxed) {
            write_rtc();
        }
        return;
    }
    match Command::new("date").arg("-s").arg(format!("@{t}")).output() {
        Ok(o) if o.status.success() => {
            eprintln!("tfctl: clock: set to {t} from {who}");
            RTC_WRITTEN.store(true, Ordering::Relaxed);
            write_rtc();
        }
        Ok(o) => eprintln!(
            "tfctl: clock: date -s: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => eprintln!("tfctl: clock: date -s: {e}"),
    }
}

/// The 48-byte SNTP client request: LI 0, version 4, mode 3, and `nonce` as the transmit time,
/// which the server echoes as its originate time (RFC 4330 section 5).
fn request(nonce: [u8; 8]) -> [u8; 48] {
    let mut b = [0u8; 48];
    b[0] = 0b00_100_011;
    b[40..48].copy_from_slice(&nonce);
    b
}

/// Unix seconds from a server reply (its transmit timestamp), if the reply is sane and answers
/// our request. NTP seconds are 32 bits and wrap in 2036 (era 1): a value below 2^31 is read as
/// era 1, since nothing here predates 2024.
fn parse(reply: &[u8], nonce: [u8; 8]) -> Option<i64> {
    if reply.len() < 48 || reply[24..32] != nonce {
        return None;
    }
    let (leap, mode, stratum) = (reply[0] >> 6, reply[0] & 7, reply[1]);
    if leap == 3 || mode != 4 || stratum == 0 || stratum > 15 {
        return None;
    }
    // A zero transmit time is "not set", not 2036 (era 1 of 0).
    if reply[40..48] == [0; 8] {
        return None;
    }
    let mut secs = u32::from_be_bytes([reply[40], reply[41], reply[42], reply[43]]) as i64;
    if secs < 0x8000_0000 {
        secs += 1 << 32;
    }
    let t = secs - NTP_UNIX_OFFSET;
    plausible(t).then_some(t)
}

/// Eight unpredictable bytes: the kernel's random pool.
fn nonce() -> [u8; 8] {
    let mut n = [0u8; 8];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = f.read_exact(&mut n);
    }
    n
}

fn query(server: &str) -> Option<i64> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    s.connect(server).ok()?;
    let n = nonce();
    s.send(&request(n)).ok()?;
    let mut buf = [0u8; 64];
    let len = s.recv(&mut buf).ok()?;
    parse(&buf[..len], n)
}

/// `tfctl clock`: what each NTP server says, against the system clock.
pub fn run() -> std::io::Result<()> {
    println!("system {}", now());
    for s in NTP_SERVERS {
        match query(s) {
            Some(t) => println!("{s} {t} (off by {} s)", t - now()),
            None => println!("{s} no answer"),
        }
    }
    Ok(())
}

pub fn start() {
    let _ = thread::Builder::new().name("clock".into()).spawn(|| {
        let mut retry = 60;
        loop {
            let unset = !valid();
            let got = NTP_SERVERS.iter().find_map(|s| query(s));
            if let Some(t) = got {
                set(t, "ntp");
            }
            // Unset: try again soon, backing off to an hour (no network for days is possible).
            let wait = match (got, unset) {
                (Some(_), _) => 24 * 3600,
                (None, true) => {
                    retry = (retry * 2).min(3600);
                    retry
                }
                (None, false) => 3600,
            };
            if got.is_some() {
                retry = 60;
            }
            thread::sleep(Duration::from_secs(wait));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_as_an_sntp_v4_client() {
        let r = request([7; 8]);
        assert_eq!((r[0] >> 3) & 7, 4, "version");
        assert_eq!(r[0] & 7, 3, "client");
        assert_eq!(&r[40..48], &[7; 8], "nonce as transmit time");
    }

    #[test]
    fn reads_the_transmit_time_of_a_sane_reply() {
        let n = [1, 2, 3, 4, 5, 6, 7, 8];
        let mut r = [0u8; 48];
        r[0] = 0b00_100_100; // server
        r[1] = 2;
        r[24..32].copy_from_slice(&n);
        let ntp = |t: i64| (((t + NTP_UNIX_OFFSET) as u64) & 0xffff_ffff) as u32;
        let t: i64 = 1_793_000_000;
        r[40..44].copy_from_slice(&ntp(t).to_be_bytes());
        assert_eq!(parse(&r, n), Some(t));
        assert_eq!(parse(&r, [0; 8]), None, "not an answer to our request");
        let mut kod = r;
        kod[1] = 0;
        assert_eq!(parse(&kod, n), None, "kiss-o'-death");
        let mut unsynced = r;
        unsynced[0] |= 0b11 << 6;
        assert_eq!(parse(&unsynced, n), None, "server not synchronised");
        let mut client = r;
        client[0] = 0b00_100_011;
        assert_eq!(parse(&client, n), None, "not a server reply");
        assert_eq!(parse(&r[..40], n), None, "short");
        let mut zero = r;
        zero[40..48].copy_from_slice(&[0; 8]);
        assert_eq!(parse(&zero, n), None, "no transmit time");
        let mut y2040 = r;
        let t2040: i64 = 2_208_988_800;
        y2040[40..44].copy_from_slice(&ntp(t2040).to_be_bytes());
        assert_eq!(parse(&y2040, n), Some(t2040), "after the 2036 wrap");
        // More than 20 years after the build: era 1 reaches 2172, a clock does not.
        assert!(clock_max() >= built() + 20 * 365 * 86_400 && built() >= CLOCK_MIN);
        let far = clock_max() + 1;
        let mut late = r;
        late[40..44].copy_from_slice(&ntp(far).to_be_bytes());
        assert_eq!(parse(&late, n), None, "past the upper bound");
        let mut edge = r;
        edge[40..44].copy_from_slice(&ntp(clock_max()).to_be_bytes());
        assert_eq!(parse(&edge, n), Some(clock_max()));
        let mut y2096 = r;
        y2096[40..44].copy_from_slice(&ntp(4_000_000_000).to_be_bytes());
        assert_eq!(parse(&y2096, n), None, "2096");
        assert!(!plausible(CLOCK_MIN - 1) && plausible(CLOCK_MIN));
    }
}
