//! Hardware watchdog: the A33's (sunxi_wdt, 16 s) resets the SoC when nothing pets it, so a hung
//! kernel or a wedged tfctl no longer needs someone at the frame. A thread opened at the top of
//! `tfctl kiosk` writes to /dev/watchdog every PET_EVERY; the kernel keeps the timer armed when
//! the process dies (no magic close), and init's respawn reopens it within seconds. Not
//! NOWAYOUT: a frame without tfctl is a dark frame, not a 16 s reboot loop. No device: logged
//! once, nothing else. Upstream measured the reset at 16 s on this board (ormarch experiments
//! E041).
//!
//! The thread pets only while the kiosk loop is alive (`beat` each pass, 10 a second): a loop
//! stuck for STUCK stops the petting and the SoC resets 16 s later. Before the loop starts
//! (settings, agent, LAN search) the start time counts as a beat.
use std::fs;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

pub const DEVICE: &str = "/dev/watchdog";
const PET_EVERY: Duration = Duration::from_secs(5);
/// Longer than anything the kiosk loop does in one pass (a cog group reap is 5 s at most).
const STUCK: Duration = Duration::from_secs(120);

/// Milliseconds since EPOCH (process start) of the kiosk loop's last pass.
static LAST_BEAT_MS: AtomicU64 = AtomicU64::new(0);
static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

fn now_ms() -> u64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// The kiosk loop is alive.
pub fn beat() {
    LAST_BEAT_MS.store(now_ms(), Ordering::Relaxed);
}

/// Pet now? Only while the last beat is younger than `stuck`.
fn alive(now_ms: u64, last_ms: u64, stuck: Duration) -> bool {
    now_ms.saturating_sub(last_ms) < stuck.as_millis() as u64
}

pub fn start() {
    beat();
    let _ = thread::Builder::new().name("watchdog".into()).spawn(|| {
        let mut f = match fs::OpenOptions::new().write(true).open(DEVICE) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("tfctl: watchdog: {DEVICE}: {e} (no hardware watchdog)");
                return;
            }
        };
        eprintln!(
            "tfctl: watchdog: {DEVICE} armed, petting every {} s",
            PET_EVERY.as_secs()
        );
        let mut warned = false;
        loop {
            if alive(now_ms(), LAST_BEAT_MS.load(Ordering::Relaxed), STUCK) {
                warned = false;
                if let Err(e) = f.write_all(b"1") {
                    eprintln!("tfctl: watchdog: pet: {e}");
                }
            } else if !warned {
                warned = true;
                eprintln!("tfctl: watchdog: kiosk loop stuck, letting the SoC reset");
            }
            thread::sleep(PET_EVERY);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pets_only_while_the_loop_beats() {
        let stuck = Duration::from_secs(120);
        assert!(alive(1_000, 900, stuck));
        assert!(alive(119_000, 0, stuck));
        assert!(!alive(121_000, 0, stuck), "stuck: let the SoC reset");
        assert!(
            alive(0, 5_000, stuck),
            "a beat from another thread just now"
        );
    }
}
