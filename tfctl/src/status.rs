//! One screen of state. Read /proc and /sys directly; shell out only for `ip` and `iwctl`.
//! Every field is optional so this also runs on the Mac (nothing under /proc there).
use crate::config::Config;
use crate::json::Value;
use std::fs;
use std::io;
use std::process::Command;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Default, Clone)]
pub struct Status {
    pub uptime_s: Option<u64>,
    pub mem_used_mb: Option<u64>,
    pub mem_total_mb: Option<u64>,
    pub ips: Vec<String>,
    pub cog_pid: Option<u32>,
    pub temp_c: Option<f64>,
    pub wifi_rssi: Option<i64>,
    /// /etc/thinframe.version: the software (tfctl + overlay) release installed (TASK 11).
    pub software_version: Option<String>,
    /// /etc/thinframe.system-version: the OS image on this slot, written at image build time.
    pub system_version: Option<String>,
    pub slot: Option<String>,
}

fn read(path: &str) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn spawn(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn collect() -> Status {
    let mut st = Status {
        uptime_s: read("/proc/uptime").and_then(|t| parse_uptime(&t)),
        software_version: read("/etc/thinframe.version").map(|s| s.trim().to_string()),
        system_version: read("/etc/thinframe.system-version").map(|s| s.trim().to_string()),
        slot: read("/proc/cmdline").and_then(|c| parse_slot(&c)),
        temp_c: read("/sys/class/thermal/thermal_zone0/temp")
            .and_then(|t| t.trim().parse::<f64>().ok())
            .map(|m| m / 1000.0),
        wifi_rssi: spawn("iwctl", &["station", "wlan0", "show"]).and_then(|o| parse_rssi(&o)),
        cog_pid: find_pid("cog"),
        ..Status::default()
    };
    if let Some(m) = read("/proc/meminfo") {
        let (total, used) = parse_meminfo(&m);
        st.mem_total_mb = total;
        st.mem_used_mb = used;
    }
    // /proc only (TASK 9: no shelling out except iwctl). The USB-C link's own address goes last,
    // so `ips[0]` (heartbeat, status page) is the LAN one.
    st.ips = read("/proc/net/fib_trie")
        .map(|t| parse_fib_trie(&t))
        .unwrap_or_default();
    st.ips.sort_by_key(|ip| ip == crate::statuspage::USB_IP);
    st
}

pub fn parse_uptime(text: &str) -> Option<u64> {
    text.split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()
        .map(|s| s as u64)
}

/// (MemTotal, MemTotal - MemAvailable) in MiB.
pub fn parse_meminfo(text: &str) -> (Option<u64>, Option<u64>) {
    let kb = |key: &str| {
        text.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
    };
    let total = kb("MemTotal:");
    let avail = kb("MemAvailable:");
    (
        total.map(|t| t / 1024),
        total.zip(avail).map(|(t, a)| t.saturating_sub(a) / 1024),
    )
}

/// /proc/net/fib_trie: local addresses sit on the line before a "/32 host LOCAL" marker.
pub fn parse_fib_trie(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    for (i, l) in lines.iter().enumerate() {
        if l.contains("/32 host LOCAL") {
            if let Some(prev) = lines.get(i.wrapping_sub(1)) {
                let ip = prev.trim().trim_start_matches("|-- ").to_string();
                if !ip.starts_with("127.") && !out.contains(&ip) {
                    out.push(ip);
                }
            }
        }
    }
    out
}

/// `iwctl station wlan0 show`: a line like "  RSSI    -52 dBm".
pub fn parse_rssi(text: &str) -> Option<i64> {
    text.lines()
        .find(|l| l.trim_start().starts_with("RSSI"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
}

/// Boot slot ("a"/"b") from the kernel cmdline: `tf.slot=`, PARTUUID or /dev/mmcblkXpN (slot.rs).
pub fn parse_slot(cmdline: &str) -> Option<String> {
    crate::slot::from_cmdline(cmdline).map(|s| s.name().to_string())
}

fn find_pid(name: &str) -> Option<u32> {
    let dir = fs::read_dir("/proc").ok()?;
    dir.flatten().find_map(|e| {
        let pid: u32 = e.file_name().to_str()?.parse().ok()?;
        let comm = fs::read_to_string(e.path().join("comm")).ok()?;
        (comm.trim() == name).then_some(pid)
    })
}

impl Status {
    pub fn to_json(&self, page_url: &str, screen_on: bool) -> Value {
        Value::obj()
            .set("uptime_s", self.uptime_s)
            .set("mem_used_mb", self.mem_used_mb)
            .set("mem_total_mb", self.mem_total_mb)
            .set("ip", self.ips.first().cloned())
            .set("wifi_rssi", self.wifi_rssi)
            .set("temp_c", self.temp_c)
            .set("tfctl_version", VERSION)
            .set("software_version", self.software_version.clone())
            .set("system_version", self.system_version.clone())
            .set("slot", self.slot.clone())
            .set("page_url", page_url)
            .set("cog_pid", self.cog_pid.map(u64::from))
            .set("screen_state", if screen_on { "on" } else { "off" })
    }
}

fn show<T: std::fmt::Display>(v: &Option<T>) -> String {
    v.as_ref().map(|v| v.to_string()).unwrap_or("-".into())
}

pub fn run(cfg: &Config) -> io::Result<()> {
    let st = collect();
    println!(
        "url      : {}  platform={} rotation={}",
        cfg.url, cfg.platform, cfg.rotation
    );
    println!("server   : {} id={}", cfg.server_url, cfg.frame_id);
    println!(
        "tfctl    : {VERSION}  software={} system={} slot={}",
        show(&st.software_version),
        show(&st.system_version),
        show(&st.slot)
    );
    println!("uptime   : {} s", show(&st.uptime_s));
    println!(
        "ip       : {}",
        if st.ips.is_empty() {
            "-".into()
        } else {
            st.ips.join(" ")
        }
    );
    println!("wifi     : rssi {} dBm", show(&st.wifi_rssi));
    println!(
        "mem      : {} / {} MiB",
        show(&st.mem_used_mb),
        show(&st.mem_total_mb)
    );
    println!("cog pid  : {}", show(&st.cog_pid));
    println!("temp     : {} C", show(&st.temp_c));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_files() {
        assert_eq!(parse_uptime("12345.67 23456.78\n"), Some(12345));
        let m = "MemTotal:        1024000 kB\nMemFree:  10 kB\nMemAvailable:     512000 kB\n";
        assert_eq!(parse_meminfo(m), (Some(1000), Some(500)));
        assert_eq!(parse_meminfo("junk"), (None, None));
        assert_eq!(
            parse_slot("console=ttyS0 root=/dev/mmcblk1p3 tf.slot=b rw"),
            Some("b".into())
        );
        assert_eq!(parse_slot("root=/dev/mmcblk1p2"), Some("a".into()));
        assert_eq!(parse_slot("root=PARTUUID=50463037-03 ro"), Some("b".into()));
        assert_eq!(
            parse_rssi("  Connected network  Home\n  RSSI   -52 dBm\n"),
            Some(-52)
        );
    }

    #[test]
    fn parses_ip_sources() {
        let trie = "Main:\n  +-- 0.0.0.0/0 3 0 5\n     |-- 127.0.0.1\n        /32 host LOCAL\n\
                    |-- 192.168.1.20\n        /32 host LOCAL\n     |-- 192.168.1.255\n        /32 link BROADCAST\n";
        assert_eq!(parse_fib_trie(trie), vec!["192.168.1.20"]);
    }

    #[test]
    fn collect_does_not_panic_anywhere() {
        let st = collect();
        let j = st.to_json("http://x/", true).to_string();
        assert!(j.contains("\"screen_state\":\"on\""));
        assert!(j.contains(&format!("\"tfctl_version\":\"{VERSION}\"")));
    }

    #[test]
    fn json_carries_memory_total() {
        // The server stores mem_total_mb; without it the status widget showed "423/0 MB".
        let st = Status {
            mem_used_mb: Some(423),
            mem_total_mb: Some(1000),
            ..Status::default()
        };
        let j = st.to_json("http://x/", true).to_string();
        assert!(
            j.contains("\"mem_used_mb\":423") && j.contains("\"mem_total_mb\":1000"),
            "{j}"
        );
    }
}
