//! Rendered pictures kept on /data across boots. One splash or backdrop takes about 4 s of the
//! A33's CPU [V 2026-10-22: `tfctl splash --png`, user 4.07 s], and a boot drew two before cog
//! started; from the second boot on both come from here in milliseconds.
//!
//! The key covers everything a picture depends on: the caller's inputs, the tfctl binary (size
//! and mtime, so an update re-renders) and the logo file. A file is written to a temporary name,
//! synced and renamed, so a power cut leaves the old entry or none; a read whose length is wrong
//! is treated as a miss. Any failure (no /data, full card) just means rendering again.
use crate::sha256::sha256_hex;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const DIR: &str = "/data/thinframe/cache";
/// Entries kept; the oldest go first. A few pictures of 4 MB at most.
const KEEP: usize = 6;

/// The cache key for `parts` plus the running binary and the logo at `logo`.
pub fn key(parts: &[&str], logo: &str) -> String {
    let stamp = |p: io::Result<PathBuf>| {
        p.and_then(fs::metadata)
            .map(|m| format!("{}:{:?}", m.len(), m.modified().ok()))
            .unwrap_or_default()
    };
    let mut s = parts.join("\u{1f}");
    s.push('\u{1f}');
    s.push_str(&stamp(std::env::current_exe()));
    s.push('\u{1f}');
    s.push_str(&stamp(Ok(PathBuf::from(logo))));
    sha256_hex(s.as_bytes())[..32].to_string()
}

/// The bytes stored under `key` in `dir` when there are `len` of them (None: any length),
/// otherwise `make()`, stored for next time.
pub fn get_or_make(
    dir: &Path,
    key: &str,
    len: Option<usize>,
    make: impl FnOnce() -> Vec<u8>,
) -> Vec<u8> {
    let path = dir.join(format!("{key}.bin"));
    if let Ok(b) = fs::read(&path) {
        if !b.is_empty() && len.is_none_or(|n| b.len() == n) {
            // Fresh again, so pruning keeps what boot uses.
            if let Ok(f) = fs::File::options().append(true).open(&path) {
                let _ = f.set_modified(SystemTime::now());
            }
            return b;
        }
    }
    let b = make();
    if let Err(e) = store(dir, &path, &b) {
        eprintln!("tfctl: render cache: {}: {e}", path.display());
    }
    b
}

fn store(dir: &Path, path: &Path, b: &[u8]) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let tmp = path.with_extension("tmp");
    let mut f = fs::File::create(&tmp)?;
    f.write_all(b)?;
    f.sync_all()?;
    drop(f);
    fs::rename(&tmp, path)?;
    prune(dir, KEEP);
    Ok(())
}

/// Keep the `keep` newest entries.
fn prune(dir: &Path, keep: usize) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut all: Vec<(SystemTime, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, e.path()))
        })
        .collect();
    all.sort_by_key(|e| std::cmp::Reverse(e.0));
    for (_, p) in all.into_iter().skip(keep) {
        let _ = fs::remove_file(p);
    }
}

/// XRGB pixels as bytes and back (little-endian, the dumb buffer's own layout).
pub fn to_bytes(px: &[u32]) -> Vec<u8> {
    px.iter().flat_map(|p| p.to_le_bytes()).collect()
}

pub fn from_bytes(b: &[u8]) -> Vec<u32> {
    b.as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_le_bytes(*c))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tfctl-rc-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn second_call_is_a_hit() {
        let d = scratch("hit");
        let made = Cell::new(0);
        let make = || {
            made.set(made.get() + 1);
            vec![1, 2, 3, 4]
        };
        assert_eq!(get_or_make(&d, "k", Some(4), make), [1, 2, 3, 4]);
        assert_eq!(get_or_make(&d, "k", Some(4), make), [1, 2, 3, 4]);
        assert_eq!(made.get(), 1);
        // A torn or foreign file of the wrong length is a miss, and is replaced.
        fs::write(d.join("k.bin"), [9]).unwrap();
        assert_eq!(get_or_make(&d, "k", Some(4), make), [1, 2, 3, 4]);
        assert_eq!(made.get(), 2);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn an_unwritable_cache_still_renders() {
        let d = Path::new("/proc/no-such-cache");
        assert_eq!(get_or_make(d, "k", None, || vec![7]), [7]);
    }

    #[test]
    fn keeps_only_the_newest() {
        let d = scratch("prune");
        for i in 0..(KEEP + 3) {
            get_or_make(&d, &format!("k{i}"), None, || vec![i as u8]);
            let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1000 + i as u64);
            let f = fs::File::options()
                .append(true)
                .open(d.join(format!("k{i}.bin")))
                .unwrap();
            f.set_modified(t).unwrap();
        }
        prune(&d, KEEP);
        let n = fs::read_dir(&d).unwrap().count();
        assert_eq!(n, KEEP);
        assert!(!d.join("k0.bin").exists());
        assert!(d.join(format!("k{}.bin", KEEP + 2)).exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn keys_differ_by_input_and_pixels_round_trip() {
        assert_ne!(key(&["a", "b"], "/x"), key(&["ab"], "/x"));
        assert_eq!(key(&["a"], "/x"), key(&["a"], "/x"));
        assert_eq!(key(&["a"], "/x").len(), 32);
        let px = [0x00112233, 0xffffffff, 0];
        assert_eq!(from_bytes(&to_bytes(&px)), px);
    }
}
