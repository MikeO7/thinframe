//! Read the browser's files into the page cache while boot waits for Wi-Fi. cog's first start
//! took 6.4 s against 2.5 s for a second one [V 2026-10-22]: the difference is ~120 MB of
//! libraries paged in from the card by page faults, in small random reads. Read in order and in
//! big blocks while the frame waits for the network anyway, they are in memory when cog starts.
//! Reading only: nothing is written, and a missing file is skipped.
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Most needed first: what cog maps at start, then the rest of the libraries, ICU's data last.
const FIRST: &[&str] = &[
    "/usr/bin/cog",
    "/usr/lib/libWPEWebKit-2.0.so",
    "/usr/libexec/wpe-webkit-2.0",
    "/usr/lib/dri",
    "/usr/lib/cog/modules",
];
const LAST: &str = "libicudata";

/// The files to read, in order, from `lib` (the library directory) and the FIRST entries.
pub fn plan(lib: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let push = |p: PathBuf, out: &mut Vec<PathBuf>| {
        if p.is_file() && !out.contains(&p) {
            out.push(p);
        }
    };
    for f in FIRST {
        let p = Path::new(f);
        if p.is_dir() {
            for e in sorted(p) {
                push(e, &mut out);
            }
        } else if let Some(parent) = p.parent() {
            // A library name: every regular file that starts with it (the versioned .so).
            let stem = p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            for e in sorted(parent) {
                if e.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with(&stem))
                {
                    push(e, &mut out);
                }
            }
        }
    }
    let mut last = Vec::new();
    for e in sorted(lib) {
        let name = e
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if !name.contains(".so") {
            continue;
        }
        if name.starts_with(LAST) {
            last.push(e);
        } else {
            push(e, &mut out);
        }
    }
    for e in last {
        push(e, &mut out);
    }
    out
}

/// Regular files (not symlinks) in `dir`, sorted.
fn sorted(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Start reading in the background.
pub fn start() {
    let r = std::thread::Builder::new()
        .name("prewarm".into())
        .spawn(|| {
            let t = std::time::Instant::now();
            let mut buf = vec![0u8; 1 << 20];
            let mut total = 0u64;
            for p in plan(Path::new("/usr/lib")) {
                if let Ok(mut f) = fs::File::open(&p) {
                    while let Ok(n) = f.read(&mut buf) {
                        if n == 0 {
                            break;
                        }
                        total += n as u64;
                    }
                }
            }
            eprintln!(
                "tfctl: prewarm: {} MB in {:.1} s",
                total >> 20,
                t.elapsed().as_secs_f64()
            );
        });
    if let Err(e) = r {
        eprintln!("tfctl: prewarm: thread: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn libraries_in_order_icu_last() {
        let d = std::env::temp_dir().join(format!("tfctl-prewarm-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        for n in ["libicudata.so.73", "liba.so.1", "libz.so.1", "notes.txt"] {
            fs::write(d.join(n), b"x").unwrap();
        }
        let names: Vec<String> = plan(&d)
            .iter()
            .filter(|p| p.starts_with(&d))
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, ["liba.so.1", "libz.so.1", "libicudata.so.73"]);
        let _ = fs::remove_dir_all(&d);
    }
}
