//! GPU hang guard. When cog was killed in the middle of a frame, the Mali-400 took an MMU page
//! fault and lima's reset then failed on every job ("pp0 job timeout", "bus stop timeout" every
//! 10 s): the page kept running, so the server watchdog saw it alive, but nothing reached the
//! screen until cog was started again [V 2026-10-26, dmesg 78129 s to 78795 s]. This thread reads
//! the kernel log and restarts the kiosk on the second job timeout within HANG_WINDOW, at most
//! once per COOLDOWN. A single timeout is left alone: lima usually recovers from one.
//! That night the hang outlasted two cog restarts, so the guard escalates: a hang again within
//! ESCALATE of its own restart reboots the frame (a GPU reset at power-on always works).
use crate::kiosk::KioskCmd;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

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
/// last restart.
#[derive(Default)]
struct Hang {
    last_timeout: Option<Instant>,
    last_restart: Option<Instant>,
}

impl Hang {
    fn timeout(&mut self, now: Instant) -> Act {
        let repeat = self
            .last_timeout
            .is_some_and(|t| now.duration_since(t) <= HANG_WINDOW);
        self.last_timeout = Some(now);
        if !repeat {
            return Act::Nothing;
        }
        match self.last_restart {
            Some(t) if now.duration_since(t) < COOLDOWN => Act::Nothing,
            Some(t) if now.duration_since(t) < ESCALATE => Act::Reboot,
            _ => {
                self.last_restart = Some(now);
                self.last_timeout = None;
                Act::Restart
            }
        }
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
                        match hang.timeout(Instant::now()) {
                            Act::Nothing => {}
                            Act::Restart => {
                                eprintln!(
                                    "tfctl: gpuwatch: GPU jobs time out, restarting the kiosk"
                                );
                                if tx.send(KioskCmd::Restart).is_err() {
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
        assert_eq!(h.timeout(s(0)), Act::Nothing, "one timeout is left alone");
        assert_eq!(h.timeout(s(60)), Act::Nothing, "too far apart");
        assert_eq!(h.timeout(s(70)), Act::Restart, "second within the window");
        assert_eq!(h.timeout(s(80)), Act::Nothing);
        assert_eq!(
            h.timeout(s(90)),
            Act::Nothing,
            "the restart gets its chance"
        );
        assert_eq!(h.timeout(s(200)), Act::Nothing);
        assert_eq!(
            h.timeout(s(210)),
            Act::Reboot,
            "still hung after the restart"
        );
        let mut h = Hang::default();
        h.timeout(s(0));
        assert_eq!(h.timeout(s(10)), Act::Restart);
        h.timeout(s(1000));
        assert_eq!(
            h.timeout(s(1010)),
            Act::Restart,
            "a new hang much later restarts again"
        );
    }
}
