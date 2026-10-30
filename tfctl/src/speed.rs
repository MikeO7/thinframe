//! Speed knobs that need no new system image (docs/decisions.md "Speed").
//!
//! - The Wi-Fi driver loads at the first sysinit line instead of when udev reaches it.
//! - BFQ on the frame's disks, so small reads are not stuck behind the prewarm's bulk reads.
//! - The CPU at full clock while the screen is on, schedutil while it is off; the GPU likewise.
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

const WIFI_MODULE: &str = "aic8800_fdrv";
const DISKS: [&str; 3] = ["mmcblk0", "mmcblk1", "mmcblk2"];
const CPUFREQ: &str = "/sys/devices/system/cpu/cpufreq";
const DEVFREQ: &str = "/sys/class/devfreq";

/// Start loading the Wi-Fi driver in the background, from `tfctl boot fsck` (the first sysinit
/// line, ~0.7 s). udev loaded it at ~2.0 s, and its firmware upload then took 1.2 s before iwd
/// could join [V 2026-10-27 dmesg]. Not waited for: udev's own modprobe later finds it loaded.
/// It loads with this frame's own Wi-Fi address (netid.rs), so it must be the first load.
pub fn load_wifi_early() {
    if Path::new("/sys/module").join(WIFI_MODULE).exists() {
        return;
    }
    let _ = Command::new("modprobe")
        .arg(WIFI_MODULE)
        .args(crate::netid::module_args())
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
    gpu_floor(Path::new(DEVFREQ), screen_on);
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

/// The GPU's lowest clock: its highest step while the screen is on, its lowest while it is off.
/// lima's devfreq has only simple_ondemand, which starts at 144 MHz and samples every 50 ms, so
/// the first frames after a quiet spell (a tap, a tab change) drew at 144 of 384 MHz
/// [V 2026-10-29, /sys/class/devfreq/1c40000.gpu]. Only GPU devices (name ends in ".gpu").
fn gpu_floor(devfreq: &Path, screen_on: bool) {
    let Ok(rd) = fs::read_dir(devfreq) else {
        return;
    };
    for e in rd.flatten() {
        if !e.file_name().to_string_lossy().ends_with(".gpu") {
            continue;
        }
        let d = e.path();
        let steps = fs::read_to_string(d.join("available_frequencies")).unwrap_or_default();
        let Some(f) = floor(&steps, screen_on) else {
            continue;
        };
        let p = d.join("min_freq");
        if fs::read_to_string(&p).is_ok_and(|c| c.trim().parse() != Ok(f)) {
            if let Err(err) = fs::write(&p, f.to_string()) {
                eprintln!("tfctl: devfreq: {}: {err}", p.display());
            }
        }
    }
}

/// The step to pin as the floor, from `available_frequencies`.
fn floor(steps: &str, screen_on: bool) -> Option<u64> {
    let mut v: Vec<u64> = steps
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect();
    v.sort_unstable();
    if screen_on {
        v.last().copied()
    } else {
        v.first().copied()
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
        let steps = "144000000 240000000 384000000\n";
        assert_eq!(floor(steps, true), Some(384_000_000));
        assert_eq!(floor(steps, false), Some(144_000_000));
        assert_eq!(floor("", true), None);
    }

    #[test]
    fn gpu_floor_sets_only_gpus() {
        let d = std::env::temp_dir().join(format!("tfctl-devfreq-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        for n in ["1c40000.gpu", "dmc"] {
            fs::create_dir_all(d.join(n)).unwrap();
            fs::write(
                d.join(n).join("available_frequencies"),
                "144000000 384000000",
            )
            .unwrap();
            fs::write(d.join(n).join("min_freq"), "144000000").unwrap();
        }
        gpu_floor(&d, true);
        let read = |n: &str| fs::read_to_string(d.join(n).join("min_freq")).unwrap();
        assert_eq!(read("1c40000.gpu"), "384000000");
        assert_eq!(read("dmc"), "144000000");
        gpu_floor(&d, false);
        assert_eq!(read("1c40000.gpu"), "144000000");
        let _ = fs::remove_dir_all(&d);
    }
}
