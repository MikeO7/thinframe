//! Speed knobs that need no new system image (docs/decisions.md "Speed").
//!
//! - The Wi-Fi driver loads at the first sysinit line instead of when udev reaches it.
//! - BFQ on the frame's disks, so small reads are not stuck behind the prewarm's bulk reads.
//! - The CPU at full clock while the screen is on, schedutil while it is off.
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

const WIFI_MODULE: &str = "aic8800_fdrv";
const DISKS: [&str; 3] = ["mmcblk0", "mmcblk1", "mmcblk2"];
const CPUFREQ: &str = "/sys/devices/system/cpu/cpufreq";

/// Start loading the Wi-Fi driver in the background, from `tfctl boot fsck` (the first sysinit
/// line, ~0.7 s). udev loaded it at ~2.0 s, and its firmware upload then took 1.2 s before iwd
/// could join [V 2026-10-27 dmesg]. Not waited for: udev's own modprobe later finds it loaded.
pub fn load_wifi_early() {
    if Path::new("/sys/module").join(WIFI_MODULE).exists() {
        return;
    }
    let _ = Command::new("modprobe")
        .arg(WIFI_MODULE)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// BFQ on the frame's disks. Its low-latency mode serves small reads (a daemon starting, dhcpcd's
/// hook scripts) ahead of the bulk reads of the prewarm and cog's start. With mq-deadline, DHCP
/// took ~7 s at boot against 1.9 s on an idle frame, and everything stalled on the disk for
/// 2.8 s of the boot [V 2026-10-27, /proc/pressure/io on the SD card].
pub fn use_bfq() {
    for d in DISKS {
        let p = format!("/sys/block/{d}/queue/scheduler");
        if Path::new(&p).exists() {
            let _ = fs::write(&p, "bfq");
        }
    }
}

/// The governor for every CPU policy: `performance` while the screen is on, so a tap or a page
/// change never waits for schedutil to raise the clock; `schedutil` while it is off. The frame
/// runs on mains power. No cpufreq (QEMU, the Mac): nothing.
pub fn cpu_governor(screen_on: bool) {
    let want = governor(screen_on);
    let Ok(rd) = fs::read_dir(CPUFREQ) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path().join("scaling_governor");
        if fs::read_to_string(&p).is_ok_and(|g| g.trim() != want) {
            if let Err(err) = fs::write(&p, want) {
                eprintln!("tfctl: cpufreq: {}: {err}", p.display());
            }
        }
    }
}

fn governor(screen_on: bool) -> &'static str {
    if screen_on {
        "performance"
    } else {
        "schedutil"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_clock_only_while_the_screen_is_on() {
        assert_eq!(governor(true), "performance");
        assert_eq!(governor(false), "schedutil");
    }
}
