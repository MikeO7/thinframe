//! Seconds since boot in front of every log line, tfctl's and cog's: `[   12.345] tfctl: ...`.
//! The kiosk's stderr (the log file) becomes a pipe that one thread copies to the file with the
//! stamp; children inherit the pipe, so cog's lines are stamped too. That is what boot timing
//! and a remote look at a problem need, and the log had no time at all.
//!
//! The thread must never stop: `eprintln!` panics when stderr cannot be written, so a dead
//! reader would take the kiosk down. It ignores every write error and only reads.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
#[cfg(target_os = "linux")]
use std::os::fd::FromRawFd;
use std::time::Instant;

/// Uptime now, from /proc/uptime (0 when unreadable).
fn uptime() -> f64 {
    std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next()?.parse().ok())
        .unwrap_or(0.0)
}

/// `[  uptime] line`
pub fn stamp(secs: f64, line: &[u8]) -> Vec<u8> {
    let mut out = format!("[{secs:9.3}] ").into_bytes();
    out.extend_from_slice(line);
    if !out.ends_with(b"\n") {
        out.push(b'\n');
    }
    out
}

/// Copy `rd` to `out` with stamps, forever (see the module comment).
fn pump(rd: File, mut out: File) {
    let (base, t0) = (uptime(), Instant::now());
    let mut rd = BufReader::new(rd);
    let mut line = Vec::new();
    loop {
        line.clear();
        match rd.read_until(b'\n', &mut line) {
            Ok(0) => std::thread::park(), // no writer left; keep the thread
            Ok(_) => {
                let secs = base + t0.elapsed().as_secs_f64();
                let _ = out.write_all(&stamp(secs, &line));
            }
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(100)),
        }
    }
}

#[cfg(target_os = "linux")]
mod fd {
    extern "C" {
        pub fn pipe2(fds: *mut i32, flags: i32) -> i32;
        pub fn fcntl(fd: i32, cmd: i32, arg: i32) -> i32;
        pub fn dup2(old: i32, new: i32) -> i32;
        pub fn close(fd: i32) -> i32;
    }
    pub const O_CLOEXEC: i32 = 0o2000000;
    pub const F_DUPFD_CLOEXEC: i32 = 1030;
}

/// Put the stamping pipe in front of stderr. On any failure stderr is left as it was.
#[cfg(target_os = "linux")]
pub fn start() {
    use fd::*;
    let mut fds = [0i32; 2];
    // SAFETY: plain fd syscalls on fds this function owns.
    let orig = unsafe {
        if pipe2(fds.as_mut_ptr(), O_CLOEXEC) != 0 {
            return;
        }
        let orig = fcntl(2, F_DUPFD_CLOEXEC, 3);
        if orig < 0 {
            close(fds[0]);
            close(fds[1]);
            return;
        }
        orig
    };
    // SAFETY: fds[0] and orig are open and owned by nothing else; the reader runs before fd 2
    // is switched, so a failed spawn leaves stderr untouched.
    let (rd, out) = unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(orig)) };
    let spawned = std::thread::Builder::new()
        .name("logstamp".into())
        .spawn(move || pump(rd, out));
    // SAFETY: fds[1] is still open; after dup2 fd 2 is the pipe (children inherit it).
    unsafe {
        if spawned.is_ok() {
            dup2(fds[1], 2);
        }
        close(fds[1]);
    }
}

#[cfg(not(target_os = "linux"))]
pub fn start() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_lines() {
        assert_eq!(stamp(1.5, b"tfctl: x\n"), b"[    1.500] tfctl: x\n");
        assert_eq!(stamp(1234.25, b"no newline"), b"[ 1234.250] no newline\n");
    }
}
