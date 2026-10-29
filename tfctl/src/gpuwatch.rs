//! GPU hang guard. When cog was killed in the middle of a frame, the Mali-400 took an MMU page
//! fault and lima's reset then failed on every job ("pp0 job timeout", "bus stop timeout" every
//! 10 s): the page kept running, so the server watchdog saw it alive, but nothing reached the
//! screen until cog was started again [V 2026-10-26, dmesg 78129 s to 78795 s]. This thread reads
//! the kernel log and restarts the kiosk on the second job timeout within HANG_WINDOW, at most
//! once per COOLDOWN. A single timeout is left alone: lima usually recovers from one.
//! That night the hang outlasted two cog restarts, so the guard escalates: a hang again within
//! ESCALATE of its own restart reboots the frame (a GPU reset at power-on always works).
//! The kiosk holds a GPU restart while the screen is off or Settings is open and does it on wake
//! or close (`KioskCmd::GpuRestart`); only a restart it really did (`restarted`) counts toward
//! the reboot, so a held one never turns into a reboot two minutes later.
use crate::kiosk::KioskCmd;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

static EPOCH: OnceLock<Instant> = OnceLock::new();
/// Milliseconds after EPOCH (plus one, 0 = never) of the kiosk's last GPU restart.
static RESTARTED_MS: AtomicU64 = AtomicU64::new(0);

/// The kiosk restarted cog for a GPU hang just now.
pub fn restarted() {
    let e = EPOCH.get_or_init(Instant::now);
    RESTARTED_MS.store(e.elapsed().as_millis() as u64 + 1, Ordering::SeqCst);
}

fn last_restart() -> Option<Instant> {
    let ms = RESTARTED_MS.load(Ordering::SeqCst);
    let e = EPOCH.get()?;
    (ms > 0).then(|| *e + Duration::from_millis(ms - 1))
}

const KMSG: &str = "/dev/kmsg";
const HANG_WINDOW: Duration = Duration::from_secs(30);
const COOLDOWN: Duration = Duration::from_secs(120);
const ESCALATE: Duration = Duration::from_secs(600);

#[derive(Debug, PartialEq)]
enum Act {
    Nothing,
    Restart,
    Reboot,
}

/// Is this kernel log record a lima job timeout?
fn is_timeout(rec: &str) -> bool {
    rec.contains("lima_sched_timedout_job") || (rec.contains("lima") && rec.contains("job timeout"))
}

/// Decides when to restart: the second timeout within HANG_WINDOW, not within COOLDOWN of the
/// last restart asked for. It reboots only when the kiosk really restarted (`restarted`, given
/// as `done`) and the hang is back within ESCALATE of that.
#[derive(Default)]
struct Hang {
    last_timeout: Option<Instant>,
    last_ask: Option<Instant>,
}

impl Hang {
    fn timeout(&mut self, now: Instant, done: Option<Instant>) -> Act {
        let repeat = self
            .last_timeout
            .is_some_and(|t| now.duration_since(t) <= HANG_WINDOW);
        self.last_timeout = Some(now);
        if !repeat {
            return Act::Nothing;
        }
        match done.map(|t| now.saturating_duration_since(t)) {
            Some(d) if d < COOLDOWN => return Act::Nothing,
            Some(d) if d < ESCALATE => return Act::Reboot,
            _ => {}
        }
        // Asked, and the kiosk is holding it (screen off, Settings open): ask again later.
        if self
            .last_ask
            .is_some_and(|t| now.duration_since(t) < COOLDOWN)
        {
            return Act::Nothing;
        }
        self.last_ask = Some(now);
        self.last_timeout = None;
        Act::Restart
    }
}

pub fn start(tx: Sender<KioskCmd>) {
    let _ = thread::Builder::new()
        .name("gpuwatch".into())
        .spawn(move || {
            let mut f = match File::open(KMSG) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("tfctl: gpuwatch: {KMSG}: {e}");
                    return;
                }
            };
            // Only what happens from now on; the boot's old records are not a hang.
            let _ = f.seek(SeekFrom::End(0));
            let mut r = BufReader::new(f);
            let mut hang = Hang::default();
            let mut rec = String::new();
            loop {
                rec.clear();
                match r.read_line(&mut rec) {
                    Ok(0) => thread::sleep(Duration::from_secs(1)),
                    // EPIPE: records were overwritten before we read them; the next read goes on.
                    Err(_) => thread::sleep(Duration::from_millis(100)),
                    Ok(_) => {
                        if !is_timeout(&rec) {
                            continue;
                        }
                        match hang.timeout(Instant::now(), last_restart()) {
                            Act::Nothing => {}
                            Act::Restart => {
                                eprintln!(
                                    "tfctl: gpuwatch: GPU jobs time out, restarting the kiosk"
                                );
                                if tx.send(KioskCmd::GpuRestart).is_err() {
                                    return;
                                }
                            }
                            Act::Reboot => {
                                eprintln!(
                                    "tfctl: gpuwatch: GPU still hung after a restart, rebooting"
                                );
                                crate::sysupdate::reboot_soon();
                                return;
                            }
                        }
                    }
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_a_lima_timeout() {
        assert!(is_timeout(
            "3,1234,78139998886,-;[drm:lima_sched_timedout_job] *ERROR* pp0 job timeout\n"
        ));
        assert!(!is_timeout(
            "6,1235,78129625920,-;lima 1c40000.gpu: pp0 bus stop timeout\n"
        ));
        assert!(!is_timeout("6,9,1,-;mmc0: job timeout\n"));
    }

    #[test]
    fn restarts_on_the_second_timeout_then_reboots_if_it_comes_back() {
        let t0 = Instant::now();
        let s = |n: u64| t0 + Duration::from_secs(n);
        let mut h = Hang::default();
        assert_eq!(
            h.timeout(s(0), None),
            Act::Nothing,
            "one timeout is left alone"
        );
        assert_eq!(h.timeout(s(60), None), Act::Nothing, "too far apart");
        assert_eq!(
            h.timeout(s(70), None),
            Act::Restart,
            "second within the window"
        );
        // The kiosk restarted at once.
        let done = Some(s(70));
        assert_eq!(h.timeout(s(80), done), Act::Nothing);
        assert_eq!(
            h.timeout(s(90), done),
            Act::Nothing,
            "the restart gets its chance"
        );
        assert_eq!(h.timeout(s(200), done), Act::Nothing);
        assert_eq!(
            h.timeout(s(210), done),
            Act::Reboot,
            "still hung after the restart"
        );
        let mut h = Hang::default();
        h.timeout(s(0), None);
        assert_eq!(h.timeout(s(10), None), Act::Restart);
        h.timeout(s(1000), Some(s(10)));
        assert_eq!(
            h.timeout(s(1010), Some(s(10))),
            Act::Restart,
            "a new hang much later restarts again"
        );
    }

    #[test]
    fn a_held_restart_is_asked_again_and_never_escalates() {
        let t0 = Instant::now();
        let s = |n: u64| t0 + Duration::from_secs(n);
        let mut h = Hang::default();
        h.timeout(s(0), None);
        assert_eq!(h.timeout(s(10), None), Act::Restart);
        // Screen off: the kiosk holds it. Timeouts go on, no reboot, no flood of asks.
        h.timeout(s(130), None);
        assert_eq!(
            h.timeout(s(135), None),
            Act::Restart,
            "asked again after the cooldown"
        );
        h.timeout(s(140), None);
        assert_eq!(h.timeout(s(145), None), Act::Nothing);
        // On wake the kiosk restarts (s(300)); a hang that outlives it reboots.
        h.timeout(s(310), Some(s(300)));
        assert_eq!(h.timeout(s(320), Some(s(300))), Act::Nothing, "its chance");
        h.timeout(s(430), Some(s(300)));
        assert_eq!(h.timeout(s(440), Some(s(300))), Act::Reboot);
        restarted();
        assert!(last_restart().is_some_and(|t| t.elapsed() < Duration::from_secs(5)));
    }
}
