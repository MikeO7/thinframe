//! The kiosk log (`/var/log/thinframe.log`, tmpfs) dies with the boot, and a boot that goes wrong
//! is exactly the one whose log matters. A thread copies what is new in it to
//! `/data/thinframe/log/current.log` every 15 s (the only card writes the log causes); the first
//! run of a new boot moves the last boot's file to `previous.log`. Both are plain 0644 files, so
//! `frame` reads them over ssh, and the report (report.rs) carries their tails. `current.log` is
//! capped like the live log, but by a synced copy renamed over it (a power cut mid-trim keeps
//! the old file, where truncating in place would leave an empty one); `/data` not mounted means nothing is written.
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::thread;
use std::time::Duration;

pub const DIR: &str = "/data/thinframe/log";
const EVERY: Duration = Duration::from_secs(15);
const MAX: u64 = 512 << 10;
const KEEP: u64 = 256 << 10;

/// Start the mirror thread for `src` into `dir`. Never fails: a missing /data just means no copy.
pub fn start(src: &'static str, dir: &'static str) {
    let boot_id = fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap_or_default();
    let _ = thread::Builder::new()
        .name("logmirror".into())
        .spawn(move || {
            let dir = Path::new(dir);
            if let Err(e) = rotate(dir, boot_id.trim()) {
                eprintln!("tfctl: logmirror: {}: {e}", dir.display());
                return;
            }
            let dst = dir.join("current.log");
            let mut offset = 0;
            loop {
                if let Err(e) = copy_new(Path::new(src), &dst, &mut offset) {
                    eprintln!("tfctl: logmirror: {e}");
                }
                let _ = cap(&dst, MAX, KEEP);
                thread::sleep(EVERY);
            }
        });
}

/// Over `max` bytes: the last `keep` replace the file (this thread is its only writer).
fn cap(path: &Path, max: u64, keep: u64) -> io::Result<()> {
    let len = fs::metadata(path)?.len();
    if len <= max {
        return Ok(());
    }
    let mut f = fs::File::open(path)?;
    f.seek(SeekFrom::Start(len - keep.min(len)))?;
    let mut tail = Vec::new();
    f.read_to_end(&mut tail)?;
    crate::update::write_synced(path, &tail)
}

/// A new boot id: current.log becomes previous.log. The same boot (tfctl respawned): nothing.
pub fn rotate(dir: &Path, boot_id: &str) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let id_file = dir.join("boot_id");
    let last = fs::read_to_string(&id_file).unwrap_or_default();
    if last.trim() == boot_id && dir.join("current.log").exists() {
        return Ok(());
    }
    match fs::rename(dir.join("current.log"), dir.join("previous.log")) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    crate::update::write_synced(&id_file, format!("{boot_id}\n").as_bytes())
}

/// Append the bytes of `src` past `*offset` to `dst`. A `src` shorter than the offset was
/// truncated by `cap_log`: start again from its beginning.
pub fn copy_new(src: &Path, dst: &Path, offset: &mut u64) -> io::Result<()> {
    let mut f = match fs::File::open(src) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let len = f.metadata()?.len();
    if len < *offset {
        *offset = 0;
    }
    if len == *offset {
        return Ok(());
    }
    f.seek(SeekFrom::Start(*offset))?;
    let mut buf = Vec::with_capacity((len - *offset) as usize);
    f.read_to_end(&mut buf)?;
    let mut out = fs::OpenOptions::new().append(true).create(true).open(dst)?;
    out.write_all(&buf)?;
    *offset += buf.len() as u64;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_on_a_new_boot_and_copies_only_new_bytes() {
        let dir = std::env::temp_dir().join(format!("tfctl-mirror-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let src = dir.join("live.log");
        let log = dir.join("log");
        fs::create_dir_all(&dir).unwrap();
        rotate(&log, "boot-1").unwrap();
        let dst = log.join("current.log");
        let mut off = 0;
        fs::write(&src, "one\n").unwrap();
        copy_new(&src, &dst, &mut off).unwrap();
        fs::write(&src, "one\ntwo\n").unwrap();
        copy_new(&src, &dst, &mut off).unwrap();
        assert_eq!(fs::read_to_string(&dst).unwrap(), "one\ntwo\n");
        // Same boot, tfctl respawned: current.log stays.
        rotate(&log, "boot-1").unwrap();
        assert_eq!(fs::read_to_string(&dst).unwrap(), "one\ntwo\n");
        // cap_log truncated the live file: the copy restarts from its start.
        fs::write(&src, "x\n").unwrap();
        copy_new(&src, &dst, &mut off).unwrap();
        assert_eq!(fs::read_to_string(&dst).unwrap(), "one\ntwo\nx\n");
        // A new boot: previous.log holds the old one, current.log starts empty.
        rotate(&log, "boot-2").unwrap();
        assert_eq!(
            fs::read_to_string(log.join("previous.log")).unwrap(),
            "one\ntwo\nx\n"
        );
        assert!(!dst.exists());
        // A missing live log is not an error.
        fs::remove_file(&src).unwrap();
        copy_new(&src, &dst, &mut off).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cap_keeps_the_tail() {
        let dir = std::env::temp_dir().join(format!("tfctl-cap-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("current.log");
        fs::write(&p, "0123456789").unwrap();
        cap(&p, 20, 4).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"0123456789");
        cap(&p, 8, 4).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"6789");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "no temp file left");
        let _ = fs::remove_dir_all(&dir);
    }
}
