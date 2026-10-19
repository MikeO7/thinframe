//! Backlight via sysfs and the local night-mode schedule (docs/decisions.md "Night mode").
//! Local time comes from busybox `date` (honours TZ and /etc/localtime) because std has no
//! localtime and we carry no tz crate.
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::Command;

pub fn backlight_dir() -> Option<PathBuf> {
    fs::read_dir("/sys/class/backlight")
        .ok()?
        .flatten()
        .map(|e| e.path())
        .next()
}

/// 0..100 percent of max_brightness. No backlight device = silently ok (sim, Mac).
pub fn set_brightness(pct: u8) -> io::Result<()> {
    let Some(dir) = backlight_dir() else {
        return Ok(());
    };
    let max: u64 = fs::read_to_string(dir.join("max_brightness"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(255);
    let v = (max * pct.min(100) as u64).div_ceil(100);
    fs::write(dir.join("brightness"), v.to_string())
}

/// "HH:MM" -> minutes since midnight.
pub fn parse_hm(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// True when `now` is inside [off, on), handling windows that wrap midnight.
pub fn in_off_window(now: u32, off: u32, on: u32) -> bool {
    if off == on {
        return false;
    }
    if off < on {
        now >= off && now < on
    } else {
        now >= off || now < on
    }
}

pub fn local_minutes() -> Option<u32> {
    let out = Command::new("date").arg("+%H:%M").output().ok()?;
    parse_hm(&String::from_utf8_lossy(&out.stdout))
}

/// What the schedule wants right now: Some(true)=off, Some(false)=on, None=no schedule / no clock.
pub fn schedule_wants_off(off: &str, on: &str) -> Option<bool> {
    let (off, on) = (parse_hm(off)?, parse_hm(on)?);
    Some(in_off_window(local_minutes()?, off, on))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hm_and_windows() {
        assert_eq!(parse_hm("23:00"), Some(23 * 60));
        assert_eq!(parse_hm(" 6:30\n"), Some(390));
        assert_eq!(parse_hm(""), None);
        assert_eq!(parse_hm("24:00"), None);
        let (off, on) = (23 * 60, 6 * 60 + 30);
        assert!(in_off_window(23 * 60, off, on));
        assert!(in_off_window(2 * 60, off, on));
        assert!(!in_off_window(6 * 60 + 30, off, on));
        assert!(!in_off_window(12 * 60, off, on));
        assert!(in_off_window(13 * 60, 12 * 60, 14 * 60));
        assert!(!in_off_window(15 * 60, 12 * 60, 14 * 60));
        assert!(!in_off_window(0, 300, 300));
        assert_eq!(schedule_wants_off("", "06:00"), None);
    }

    #[test]
    fn local_clock_is_readable_or_none() {
        if let Some(m) = local_minutes() {
            assert!(m < 1440);
        }
    }
}
