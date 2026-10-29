//! Signals without the libc crate: two `extern "C"` declarations against the libc that std
//! already links. SIGTERM is trapped into a flag the kiosk loop polls; `kill` pauses/resumes cog.
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

pub static TERM: AtomicBool = AtomicBool::new(false);

pub const SIGINT: i32 = 2;
pub const SIGKILL: i32 = 9;
pub const SIGTERM: i32 = 15;
pub const SIGSTOP: i32 = if cfg!(target_os = "linux") { 19 } else { 17 };
pub const SIGCONT: i32 = if cfg!(target_os = "linux") { 18 } else { 19 };

extern "C" {
    fn signal(sig: i32, handler: extern "C" fn(i32)) -> usize;
    fn kill(pid: i32, sig: i32) -> i32;
}

#[cfg(target_os = "linux")]
extern "C" {
    fn setpriority(which: i32, who: u32, prio: i32) -> i32;
    fn gettid() -> i32;
}

/// Give the calling thread a lower CPU priority (nice `n`); Linux applies it to the thread alone.
/// For work that must not slow cog's start, such as the splash animation.
pub fn nice_this_thread(n: i32) {
    #[cfg(target_os = "linux")]
    // SAFETY: plain C calls on our own thread id; a failure only leaves the priority as it was.
    unsafe {
        setpriority(0, gettid() as u32, n);
    }
    #[cfg(not(target_os = "linux"))]
    let _ = n;
}

#[cfg(target_os = "linux")]
extern "C" {
    fn syscall(num: std::ffi::c_long, ...) -> std::ffi::c_long;
}

/// Restart the frame: `reboot` (init's orderly path), and if that program cannot run or init has
/// not restarted the frame within a minute, the kernel's own restart. A card pulled while the
/// system runs takes /sbin/reboot with it; the frame then answered `reboot` and stayed up
/// [V 2026-10-29]. Blocks the calling thread.
pub fn reboot_now() {
    let _ = std::process::Command::new("sync").status();
    match std::process::Command::new("reboot").status() {
        Ok(s) if s.success() => std::thread::sleep(std::time::Duration::from_secs(60)),
        Ok(s) => eprintln!("tfctl: reboot exited with {s}; restarting by the kernel"),
        Err(e) => eprintln!("tfctl: reboot: {e}; restarting by the kernel"),
    }
    kernel_restart();
}

/// reboot(2) with LINUX_REBOOT_CMD_RESTART, after sync(2): no program needed.
fn kernel_restart() {
    #[cfg(all(target_os = "linux", target_arch = "arm"))]
    const NR: [std::ffi::c_long; 2] = [36, 88];
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    const NR: [std::ffi::c_long; 2] = [81, 142];
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    const NR: [std::ffi::c_long; 2] = [162, 169];
    #[cfg(all(
        target_os = "linux",
        any(target_arch = "arm", target_arch = "aarch64", target_arch = "x86_64")
    ))]
    // SAFETY: sync() and reboot(magic, magic2, RESTART, NULL) take no pointers we own; on
    // success the call does not return, on failure nothing changed.
    unsafe {
        syscall(NR[0]);
        syscall(
            NR[1],
            0xfee1_deadu32 as std::ffi::c_int,
            672_274_793 as std::ffi::c_int,
            0x0123_4567 as std::ffi::c_int,
            0 as std::ffi::c_long,
        );
    }
    eprintln!("tfctl: the kernel restart failed");
}

/// Give the calling thread the lowest best-effort I/O priority (`ioprio_set`, which BFQ honours):
/// for bulk reads that may wait, such as the prewarm.
pub fn low_io_priority_this_thread() {
    #[cfg(all(target_os = "linux", target_arch = "arm"))]
    const NR_IOPRIO_SET: std::ffi::c_long = 314;
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    const NR_IOPRIO_SET: std::ffi::c_long = 30;
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    const NR_IOPRIO_SET: std::ffi::c_long = 251;
    #[cfg(all(
        target_os = "linux",
        any(target_arch = "arm", target_arch = "aarch64", target_arch = "x86_64")
    ))]
    // SAFETY: ioprio_set(IOPRIO_WHO_PROCESS, 0 = this thread, BE class 2 level 7) takes no
    // pointers; a failure only leaves the priority as it was.
    unsafe {
        syscall(
            NR_IOPRIO_SET,
            1 as std::ffi::c_int,
            0 as std::ffi::c_int,
            (2 << 13 | 7) as std::ffi::c_int,
        );
    }
}

extern "C" fn on_term(_sig: i32) {
    TERM.store(true, Ordering::SeqCst);
}

/// Install the SIGTERM/SIGINT flag handler. Idempotent.
pub fn trap_term() {
    // SAFETY: plain C calls with a valid handler; the handler only stores an atomic.
    unsafe {
        signal(SIGTERM, on_term);
        signal(SIGINT, on_term);
    }
}

pub fn term_requested() -> bool {
    TERM.load(Ordering::SeqCst)
}

pub fn send(pid: u32, sig: i32) -> io::Result<()> {
    // SAFETY: kill(2) has no memory-safety preconditions.
    let rc = unsafe { kill(pid as i32, sig) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Signal every process in group `pgid` (`kill(-pgid, sig)`). Signal 0 checks whether any is left.
pub fn send_group(pgid: u32, sig: i32) -> io::Result<()> {
    // SAFETY: kill(2) has no memory-safety preconditions.
    let rc = unsafe { kill(-(pgid as i32), sig) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_zero_probes_own_pid() {
        assert!(send(std::process::id(), 0).is_ok());
        assert!(!term_requested());
    }
}
