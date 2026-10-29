//! Hardware watchdog: the A33's (sunxi_wdt, 16 s) resets the SoC when nothing pets it, so a hung
//! kernel or a wedged tfctl no longer needs someone at the frame. A thread started first thing in
//! `tfctl kiosk` (main, before the config is read) writes to /dev/watchdog every PET_EVERY; the
//! kernel keeps the timer armed when the process dies (no magic close), and init's respawn
//! reopens it within seconds. Not NOWAYOUT: a frame without tfctl is a dark frame, not a 16 s
//! reboot loop. No device: logged once, the rest still runs. Upstream measured the reset at 16 s
//! on this board (ormarch experiments E041).
//!
//! The thread pets only while the kiosk loop is alive (`beat` each pass, 10 a second): a loop
//! stuck for STUCK stops the petting and the SoC resets 16 s later. Before the loop starts
//! (settings, agent, LAN search) the start time counts as a beat.
//!
//! It also holds the unconfirmed-slot rule, so it works whatever part of tfctl fails: a new
//! system slot that U-Boot still counts tries for (uEnv `tries` > 0 naming the running slot) and
//! that has not reached the server UNCONFIRMED_REBOOT_S after boot is rebooted, so U-Boot spends
//! a try and falls back. Checked once synchronously at start (a tfctl that crashes a second later
//! in Kiosk::new, the splash or the agent, and is respawned by init, still gets there) and then
//! every CHECK_EVERY. The petting stops too, so a `reboot` that hangs ends in a reset. A
//! confirmed slot (tries=0) is never rebooted by this.
use crate::update::{self, Paths};
use std::fs;
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Once, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

pub const DEVICE: &str = "/dev/watchdog";
const PET_EVERY: Duration = Duration::from_secs(5);
/// Longer than anything the kiosk loop does in one pass (a cog group reap is 5 s at most).
const STUCK: Duration = Duration::from_secs(120);
/// A slot U-Boot still counts tries for that has not reached the server this long after boot is
/// rebooted, so U-Boot spends a try (and falls back) without a power cycle.
pub const UNCONFIRMED_REBOOT_S: u64 = 10 * 60;
const CHECK_EVERY: Duration = Duration::from_secs(30);

/// Milliseconds since EPOCH (process start) of the kiosk loop's last pass.
static LAST_BEAT_MS: AtomicU64 = AtomicU64::new(0);
static EPOCH: OnceLock<Instant> = OnceLock::new();
/// Seconds since boot of this process's last server answer; 0 = none yet.
static LAST_CONTACT_UP: AtomicU64 = AtomicU64::new(0);
/// The unconfirmed-slot reboot was started: stop petting, never start it twice.
static UNCONFIRMED: AtomicBool = AtomicBool::new(false);

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

fn uptime() -> Option<u64> {
    crate::agent::uptime_s(&fs::read_to_string("/proc/uptime").ok()?)
}

/// The server answered (agent). Any earlier contact on this boot has already confirmed the slot
/// (tries=0), so only this process's contact matters; it covers a confirm that could not be
/// written while the server is reachable.
pub fn contact() {
    if let Some(up) = uptime() {
        LAST_CONTACT_UP.store(up.max(1), Ordering::Relaxed);
    }
}

/// Reboot only while uEnv names the running slot with tries left (tries=0 is a confirmed slot;
/// another slot means U-Boot already fell back), and only once both the boot and the last
/// server contact (or the boot, with none) are `UNCONFIRMED_REBOOT_S` old.
pub fn should_reboot_unconfirmed(
    tries: u32,
    slot_is_running: bool,
    secs_since_boot: u64,
    secs_since_last_contact: u64,
) -> bool {
    tries > 0
        && slot_is_running
        && secs_since_boot >= UNCONFIRMED_REBOOT_S
        && secs_since_last_contact >= UNCONFIRMED_REBOOT_S
}

/// What to log when the running slot is unconfirmed and overdue, else None. `up` is seconds
/// since boot, `last_contact` the uptime of the last server answer (0 = none). Reads only the
/// booted disk's /boot (uEnv.txt, or uEnv.bak) and the kernel command line.
fn overdue(p: &Paths, up: u64, last_contact: u64) -> Option<String> {
    let since_contact = if last_contact == 0 {
        up
    } else {
        up.saturating_sub(last_contact)
    };
    if up < UNCONFIRMED_REBOOT_S || since_contact < UNCONFIRMED_REBOOT_S {
        return None;
    }
    let uenv = crate::sysupdate::uenv(p).ok()?;
    let running = crate::sysupdate::running_slot(p);
    let is_running = running.is_some() && uenv.slot == running;
    should_reboot_unconfirmed(uenv.tries, is_running, up, since_contact).then(|| {
        format!(
            "slot {} not confirmed (tries={}) and no server contact for {since_contact} s; rebooting",
            running.map_or("?", |s| s.name()),
            uenv.tries
        )
    })
}

/// The unconfirmed-slot rule on the real frame (no /proc/uptime elsewhere: nothing happens).
fn check_unconfirmed() {
    let Some(up) = uptime() else {
        return;
    };
    let p = Paths::system();
    let Some(text) = overdue(&p, up, LAST_CONTACT_UP.load(Ordering::Relaxed)) else {
        return;
    };
    if UNCONFIRMED.swap(true, Ordering::SeqCst) {
        return;
    }
    eprintln!("tfctl: watchdog: {text}");
    update::queue_event(&p, "slot-unconfirmed-reboot", &text);
    update::sync_disks();
    // In a thread of its own, not after a delay: this process may be about to crash, and the
    // watchdog stops being petted, so a hanging restart still ends in a SoC reset.
    std::thread::spawn(crate::sys::reboot_now);
}

pub fn start() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        beat();
        check_unconfirmed();
        let _ = thread::Builder::new().name("watchdog".into()).spawn(run);
    });
}

fn run() {
    let mut dev = match fs::OpenOptions::new().write(true).open(DEVICE) {
        Ok(f) => {
            eprintln!(
                "tfctl: watchdog: {DEVICE} armed, petting every {} s",
                PET_EVERY.as_secs()
            );
            Some(f)
        }
        Err(e) => {
            eprintln!("tfctl: watchdog: {DEVICE}: {e} (no hardware watchdog)");
            None
        }
    };
    let mut warned = false;
    let mut checked = Instant::now();
    loop {
        if checked.elapsed() >= CHECK_EVERY {
            checked = Instant::now();
            check_unconfirmed();
        }
        if let Some(f) = &mut dev {
            if UNCONFIRMED.load(Ordering::SeqCst) {
                if !warned {
                    warned = true;
                    eprintln!("tfctl: watchdog: unconfirmed slot, no more petting");
                }
            } else if alive(now_ms(), LAST_BEAT_MS.load(Ordering::Relaxed), STUCK) {
                warned = false;
                if let Err(e) = f.write_all(b"1") {
                    eprintln!("tfctl: watchdog: pet: {e}");
                }
            } else if !warned {
                warned = true;
                eprintln!("tfctl: watchdog: kiosk loop stuck, letting the SoC reset");
            }
        }
        thread::sleep(PET_EVERY);
    }
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

    const T: u64 = UNCONFIRMED_REBOOT_S;

    #[test]
    fn reboots_only_an_unconfirmed_running_slot_after_the_timeout() {
        assert!(should_reboot_unconfirmed(2, true, T, T));
        assert!(should_reboot_unconfirmed(1, true, T + 500, T + 1));
        // Confirmed slot: never.
        assert!(!should_reboot_unconfirmed(0, true, T * 10, T * 10));
        // uEnv names the other slot: U-Boot already fell back, leave it alone.
        assert!(!should_reboot_unconfirmed(2, false, T * 10, T * 10));
        // Too early after boot, or the server answered recently.
        assert!(!should_reboot_unconfirmed(2, true, T - 1, T - 1));
        assert!(!should_reboot_unconfirmed(2, true, T * 10, T - 1));
    }

    #[test]
    fn overdue_reads_the_booted_disks_uenv() {
        let base = update::tests::tmp_base("unconf");
        let p = Paths::under(&base);
        fs::create_dir_all(&p.boot).unwrap();
        fs::write(&p.cmdline, "root=PARTUUID=50463037-03").unwrap();
        let uenv = crate::sysupdate::uenv_path(&p);
        fs::write(&uenv, "slot=b\ntries=2\n").unwrap();
        // A fresh tfctl (respawned after a crash) on a boot 11 minutes old, no contact yet.
        let t = overdue(&p, T + 60, 0).expect("overdue");
        assert!(t.contains("slot b") && t.contains("tries=2"), "{t}");
        assert_eq!(overdue(&p, T - 1, 0), None, "too early");
        assert_eq!(overdue(&p, T * 3, T * 3 - 5), None, "server answered");
        assert!(overdue(&p, T * 3, T).is_some(), "last answer long ago");
        // Confirmed: never, however long.
        fs::write(&uenv, "slot=b\ntries=0\n").unwrap();
        assert_eq!(overdue(&p, T * 100, 0), None);
        // U-Boot already fell back to a: leave it.
        fs::write(&uenv, "slot=b\ntries=1\n").unwrap();
        fs::write(&p.cmdline, "root=PARTUUID=50463037-02").unwrap();
        assert_eq!(overdue(&p, T * 100, 0), None);
        // Booted from the eMMC (its own signature): same rule, its own /boot.
        fs::write(&p.cmdline, "root=PARTUUID=50463045-03").unwrap();
        assert!(overdue(&p, T * 2, 0).is_some());
        // No uEnv at all (/boot not mounted): nothing to go on, no reboot.
        fs::remove_file(&uenv).unwrap();
        assert_eq!(overdue(&p, T * 100, 0), None);
        fs::remove_dir_all(&base).unwrap();
    }
}
