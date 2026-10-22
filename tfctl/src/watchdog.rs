//! Hardware watchdog: the A33's (sunxi_wdt, 16 s) resets the SoC when nothing pets it, so a hung
//! kernel or a wedged tfctl no longer needs someone at the frame. A thread opened at the top of
//! `tfctl kiosk` writes to /dev/watchdog every PET_EVERY; the kernel keeps the timer armed when
//! the process dies (no magic close), and init's respawn reopens it within seconds. Not
//! NOWAYOUT: a frame without tfctl is a dark frame, not a 16 s reboot loop. No device: logged
//! once, nothing else. Upstream measured the reset at 16 s on this board (ormarch experiments
//! E041).
use std::fs;
use std::io::Write;
use std::thread;
use std::time::Duration;

pub const DEVICE: &str = "/dev/watchdog";
const PET_EVERY: Duration = Duration::from_secs(5);

pub fn start() {
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
        loop {
            if let Err(e) = f.write_all(b"1") {
                eprintln!("tfctl: watchdog: pet: {e}");
            }
            thread::sleep(PET_EVERY);
        }
    });
}
