//! One screen of state. Read /proc and /sys directly; shell out only for iwctl.
//!
//! TODO(TASK 9): uptime (/proc/uptime), ip (parse `ip -4 -o addr` or /proc/net/fib_trie),
//! wifi (iwctl), mem (/proc/meminfo MemTotal/MemAvailable), cog pid (/proc/*/comm),
//! temp (/sys/class/thermal/thermal_zone0/temp), url/platform/rotation from cfg.
use crate::config::Config;
use std::io;

pub fn run(cfg: &Config) -> io::Result<()> {
    println!("url      : {}  platform={} rotation={}", cfg.url, cfg.platform, cfg.rotation);
    Err(io::Error::other("status: rest not implemented (TASK 9)"))
}
