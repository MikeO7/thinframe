//! `report` action: one text file with everything a remote debugger needs (versions, slot,
//! status, config, uEnv, mounts, processes, Wi-Fi, dmesg, this boot's log, the previous boot's
//! log) posted to `POST /api/frames/<id>/report`. The server keeps the last few per frame and
//! shows them in /admin and over MCP, so no ssh or cable is needed to see what the frame saw.
//! Sent automatically on the first server contact when debug mode is on (dbg.rs).
use crate::config::Config;
use crate::http::{self, Url};
use crate::status;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::process::Command;
use std::time::Duration;

/// The server refuses more (api handlers); a frame never produces more.
pub const MAX_BYTES: usize = 384 << 10;
const LOG_TAIL: u64 = 96 << 10;
const PREV_TAIL: u64 = 24 << 10;
const DMESG_TAIL: usize = 64 << 10;

/// Files copied whole (title, path).
const FILES: [(&str, &str); 5] = [
    ("uEnv.txt", "/boot/uEnv.txt"),
    ("uEnv.bak", "/boot/uEnv.bak"),
    ("cmdline", "/proc/cmdline"),
    ("mounts", "/proc/mounts"),
    ("loadavg", "/proc/loadavg"),
];
/// Commands run (title, program, args).
const CMDS: [(&str, &str, &[&str]); 3] = [
    ("df", "df", &["-h"]),
    ("ps", "ps", &["-o", "pid,user,vsz,rss,stat,comm"]),
    ("dmesg", "dmesg", &[]),
];

pub fn collect(cfg: &Config) -> String {
    let st = status::collect();
    let mut out = String::with_capacity(64 << 10);
    let opt = |v: Option<String>| v.unwrap_or_else(|| "-".into());
    let num = |v: Option<i64>| opt(v.map(|n| n.to_string()));
    let head = format!(
        "tfctl {}  software={} system={} slot={}\nframe_id={} server={}\nuptime={} s  ip={}  rssi={} dBm  mem={}/{} MiB  temp={} C  cog_pid={}\n",
        status::VERSION,
        opt(st.software_version),
        opt(st.system_version),
        opt(st.slot),
        cfg.frame_id,
        cfg.server_url,
        num(st.uptime_s.map(|u| u as i64)),
        st.ips.join(" "),
        num(st.wifi_rssi),
        num(st.mem_used_mb.map(|m| m as i64)),
        num(st.mem_total_mb.map(|m| m as i64)),
        num(st.temp_c.map(|t| t as i64)),
        num(st.cog_pid.map(|p| p as i64)),
    );
    section(&mut out, "tfctl", &head);
    section(&mut out, "thinframe.conf", &read(&cfg.path));
    for (title, path) in FILES {
        section(&mut out, title, &read(path));
    }
    section(&mut out, "meminfo", &head_lines(&read("/proc/meminfo"), 6));
    for (title, prog, args) in CMDS {
        let body = cmd(prog, args);
        section(&mut out, title, tail_str(&body, DMESG_TAIL));
    }
    section(
        &mut out,
        "iwctl station show",
        &crate::wifi::iwctl(
            &["station", crate::wifi::STATION, "show"],
            Duration::from_secs(5),
        )
        .map(|(_, o)| o)
        .unwrap_or_else(|e| e.to_string()),
    );
    section(
        &mut out,
        "thinframe.log (this boot)",
        &tail(crate::kiosk::LOG, LOG_TAIL),
    );
    section(
        &mut out,
        "previous boot",
        &tail(
            &format!("{}/previous.log", crate::logmirror::DIR),
            PREV_TAIL,
        ),
    );
    out.truncate(MAX_BYTES.min(out.len()));
    out
}

/// POST the report; the frame's own token opens its report route. Returns the bytes sent.
pub fn send(base: &Url, cfg: &Config, text: &str) -> io::Result<usize> {
    let url = base.with_path(&format!("/api/frames/{}/report", cfg.frame_id));
    let r = http::post_bytes(
        &url,
        &cfg.token,
        "text/plain; charset=utf-8",
        text.as_bytes(),
        Duration::from_secs(30),
    )?;
    if r.status != 200 && r.status != 201 && r.status != 204 {
        return Err(io::Error::other(format!("POST report: HTTP {}", r.status)));
    }
    Ok(text.len())
}

fn section(out: &mut String, title: &str, body: &str) {
    out.push_str("==== ");
    out.push_str(title);
    out.push('\n');
    out.push_str(body);
    if !body.ends_with('\n') {
        out.push('\n');
    }
}

fn read(path: &str) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| format!("({e})"))
}

fn head_lines(text: &str, n: usize) -> String {
    text.lines().take(n).map(|l| format!("{l}\n")).collect()
}

fn cmd(name: &str, args: &[&str]) -> String {
    match Command::new(name).args(args).output() {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
            s.push_str(&String::from_utf8_lossy(&o.stderr));
            s
        }
        Err(e) => format!("({name}: {e})"),
    }
}

/// The last `n` bytes of a file, from the first whole line.
fn tail(path: &str, n: u64) -> String {
    let Ok(mut f) = fs::File::open(path) else {
        return "(none)".into();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(n);
    let _ = f.seek(SeekFrom::Start(start));
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    let s = String::from_utf8_lossy(&buf);
    if start == 0 {
        return s.into_owned();
    }
    // Cut in the middle of a line: start at the next whole one.
    s.find('\n')
        .map_or(String::new(), |i| s[i + 1..].to_string())
}

fn tail_str(s: &str, n: usize) -> &str {
    if s.len() <= n {
        return s;
    }
    let cut = s.len() - n;
    match s[cut..].find('\n') {
        Some(i) => &s[cut + i + 1..],
        None => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tails_from_a_whole_line() {
        assert_eq!(tail_str("ab\ncd\nef\n", 5), "ef\n");
        assert_eq!(tail_str("ab\ncd\n", 100), "ab\ncd\n");
        assert_eq!(tail_str("abcdef", 3), "");
        let dir = std::env::temp_dir().join(format!("tfctl-report-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("log");
        fs::write(&p, "one\ntwo\nthree\n").unwrap();
        assert_eq!(tail(p.to_str().unwrap(), 9), "three\n");
        assert_eq!(tail("/nonexistent/x", 9), "(none)");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn collect_has_every_section_and_is_bounded() {
        let cfg = Config {
            path: "/nonexistent/thinframe.conf".into(),
            ..Config::default()
        };
        let r = collect(&cfg);
        for s in [
            "==== tfctl\n",
            "==== thinframe.conf\n",
            "==== dmesg\n",
            "==== previous boot\n",
        ] {
            assert!(r.contains(s), "{s}");
        }
        assert!(r.len() <= MAX_BYTES);
    }
}
