//! Signals without the libc crate: two `extern "C"` declarations against the libc that std
//! already links. SIGTERM is trapped into a flag the kiosk loop polls; `kill` pauses/resumes cog.
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

pub static TERM: AtomicBool = AtomicBool::new(false);

pub const SIGINT: i32 = 2;
pub const SIGTERM: i32 = 15;
pub const SIGSTOP: i32 = if cfg!(target_os = "linux") { 19 } else { 17 };
pub const SIGCONT: i32 = if cfg!(target_os = "linux") { 18 } else { 19 };

extern "C" {
    fn signal(sig: i32, handler: extern "C" fn(i32)) -> usize;
    fn kill(pid: i32, sig: i32) -> i32;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_zero_probes_own_pid() {
        assert!(send(std::process::id(), 0).is_ok());
        assert!(!term_requested());
    }
}
