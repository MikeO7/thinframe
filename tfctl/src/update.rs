//! Software update (TASK 11): `tfctl-armv7` + `overlay.tar.gz` from the server's
//! `/api/artifacts/`, sha256 from the config object, `tfctl.prev` kept, then `exit(0)` so the
//! BusyBox `inittab` respawn line starts the new binary. Rollback: every start appends to
//! `/data/thinframe/starts`; when an update is pending (`update-pending` marker) and the two
//! previous starts were both within 60 s, the old binary is swapped back and an event is queued
//! for the agent. The system (A/B slot) update lives in `sysupdate.rs`.
//!
//! All file-system code takes a [`Paths`] so tests run against a temp dir. The frame's root is
//! mounted read-only; it is remounted rw only while an update writes to it.
use crate::config::Config;
use crate::http::{self, Url};
use crate::json::Value;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30);
const CRASH_WINDOW_S: u64 = 60;
const STARTS_KEEP: usize = 20;
/// Paths the overlay must never touch (provisioned once, live on the data partition).
pub const OVERLAY_EXCLUDES: [&str; 3] = ["etc/thinframe.conf", "etc/dropbear", "var/lib/iwd"];

/// Where things live. `Paths::system()` on the frame; tests point everything at a temp dir.
#[derive(Debug, Clone)]
pub struct Paths {
    /// `/`: holds usr/bin/tfctl, etc/thinframe.version, etc/thinframe.system-version.
    pub root: PathBuf,
    /// `/data/thinframe`: starts log, markers, queued events, download scratch space.
    pub data: PathBuf,
    /// `/boot` (PFBOOT): uEnv.txt, zImage.a, zImage.b.
    pub boot: PathBuf,
    pub cmdline: PathBuf,
    /// Whole-disk candidates for the card; see `slot::find_disk`.
    pub disks: Vec<PathBuf>,
}

impl Paths {
    pub fn system() -> Paths {
        Paths {
            root: "/".into(),
            data: "/data/thinframe".into(),
            boot: "/boot".into(),
            cmdline: "/proc/cmdline".into(),
            disks: crate::slot::DISKS.iter().map(PathBuf::from).collect(),
        }
    }
    /// Test trees under `base`: base/root, base/data, base/boot, base/cmdline.
    #[cfg(test)]
    pub fn under(base: &Path) -> Paths {
        Paths {
            root: base.join("root"),
            data: base.join("data"),
            boot: base.join("boot"),
            cmdline: base.join("cmdline"),
            disks: vec![base.join("mmcblk0"), base.join("mmcblk1")],
        }
    }
    /// Only the real frame gets `mount` calls and reboots.
    pub fn is_real(&self) -> bool {
        self.root == Path::new("/")
    }
    pub fn binary(&self) -> PathBuf {
        self.root.join("usr/bin/tfctl")
    }
    pub fn version_file(&self) -> PathBuf {
        self.root.join("etc/thinframe.version")
    }
    pub fn system_version_file(&self) -> PathBuf {
        self.root.join("etc/thinframe.system-version")
    }
    pub fn marker(&self) -> PathBuf {
        self.data.join("update-pending")
    }
    pub fn events_file(&self) -> PathBuf {
        self.data.join("events")
    }
    pub fn tmp(&self) -> PathBuf {
        self.data.join("tmp")
    }
}

/// `artifacts` from the config object: logical name -> (file name, sha256 hex).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Artifacts(pub HashMap<String, (String, String)>);

impl Artifacts {
    pub fn parse(v: &Value) -> Artifacts {
        let mut m = HashMap::new();
        if let Some(Value::Obj(kv)) = v.get("artifacts") {
            for (k, v) in kv {
                if let Some((file, sum)) = v.as_str().and_then(|s| s.split_once(':')) {
                    if !file.is_empty() && !file.contains('/') && sum.len() == 64 {
                        m.insert(k.clone(), (file.to_string(), sum.to_ascii_lowercase()));
                    }
                }
            }
        }
        Artifacts(m)
    }
    pub fn get(&self, name: &str) -> io::Result<(&str, &str)> {
        self.0
            .get(name)
            .map(|(f, s)| (f.as_str(), s.as_str()))
            .ok_or_else(|| io::Error::other(format!("server offers no '{name}' artifact")))
    }
}

/// Download `/api/artifacts/<file>` to `dest`, verify the sha256, or remove it and fail.
pub fn fetch(cfg: &Config, file: &str, sha: &str, dest: &Path) -> io::Result<()> {
    let base = Url::parse(&cfg.server_url)
        .ok_or_else(|| io::Error::other(format!("bad SERVER_URL '{}'", cfg.server_url)))?;
    let url = base.with_path(&format!("/api/artifacts/{file}"));
    if let Some(d) = dest.parent() {
        fs::create_dir_all(d)?;
    }
    eprintln!("tfctl: update: downloading {file}");
    let got = http::download(&url, &cfg.token, DOWNLOAD_TIMEOUT, dest)?;
    if got != sha {
        let _ = fs::remove_file(dest);
        return Err(io::Error::other(format!(
            "{file}: sha256 mismatch (got {got}, config says {sha})"
        )));
    }
    Ok(())
}

pub fn sh(cmd: &str, args: &[&str]) -> io::Result<()> {
    let out = Command::new(cmd).args(args).output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{cmd} {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

/// Remount the read-only root for the duration of a write. No-op in tests.
pub fn remount_root(p: &Paths, rw: bool) -> io::Result<()> {
    if !p.is_real() {
        return Ok(());
    }
    sh(
        "mount",
        &["-o", if rw { "remount,rw" } else { "remount,ro" }, "/"],
    )
}

pub fn sync_disks() {
    let _ = Command::new("sync").status();
}

pub fn now_s() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Queue an event for the agent to send on its next connect (kind, text).
pub fn queue_event(p: &Paths, kind: &str, text: &str) {
    let _ = fs::create_dir_all(&p.data);
    let line = format!("{kind}\t{}\n", text.replace(['\t', '\n'], " "));
    let mut cur = fs::read_to_string(p.events_file()).unwrap_or_default();
    cur.push_str(&line);
    let _ = fs::write(p.events_file(), cur);
}

pub fn drain_events(p: &Paths) -> Vec<(String, String)> {
    let text = fs::read_to_string(p.events_file()).unwrap_or_default();
    let _ = fs::remove_file(p.events_file());
    text.lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(k, t)| (k.to_string(), t.to_string()))
        .collect()
}

/// TASK 11. Installs tfctl (and the overlay when offered), writes the version file and the
/// pending marker. The caller exits(0) afterwards; the init respawn starts the new binary.
pub fn software(cfg: &Config, arts: &Artifacts, version: &str, p: &Paths) -> io::Result<()> {
    let (bin_file, bin_sha) = arts.get("tfctl")?;
    let overlay = arts.get("overlay").ok();
    fs::create_dir_all(p.tmp())?;
    let new_bin = p.tmp().join("tfctl.new");
    fetch(cfg, bin_file, bin_sha, &new_bin)?;
    fs::set_permissions(&new_bin, fs::Permissions::from_mode(0o755))?;
    let overlay_tgz = p.tmp().join("overlay.tar.gz");
    if let Some((file, sha)) = overlay {
        fetch(cfg, file, sha, &overlay_tgz)?;
    }
    remount_root(p, true)?;
    let res = install(p, &new_bin, overlay.map(|_| overlay_tgz.as_path()), version);
    sync_disks();
    let ro = remount_root(p, false);
    res.and(ro)
}

/// Order matters: the overlay is extracted first, while the old binary still runs, so a failed
/// `tar` leaves the known-good binary and no marker. Then the binary is swapped and the version
/// file and marker written; if any of that fails, the old binary and version are put back.
fn install(p: &Paths, new_bin: &Path, overlay: Option<&Path>, version: &str) -> io::Result<()> {
    let bin = p.binary();
    fs::create_dir_all(bin.parent().unwrap())?;
    // Same file system as the target so the final rename is atomic.
    let staged = bin.with_extension("new");
    fs::copy(new_bin, &staged)?;
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))?;
    if let Some(tgz) = overlay {
        if let Err(e) = extract_overlay(p, tgz) {
            let _ = fs::remove_file(&staged);
            return Err(e);
        }
    }
    fs::create_dir_all(p.version_file().parent().unwrap())?;
    let ver_prev = version_prev(p);
    if p.version_file().is_file() {
        fs::copy(p.version_file(), &ver_prev)?;
    } else {
        let _ = fs::remove_file(&ver_prev);
    }
    // The old binary is linked, not moved, to `tfctl.prev`, and the new one renamed over it:
    // `/usr/bin/tfctl` exists at every instant. A gap there (a rename pair) and a power cut
    // between them would leave init respawning nothing, with the card the only way back.
    let prev = bin.with_extension("prev");
    let had_bin = bin.exists();
    if had_bin {
        keep_copy(&bin, &prev)?;
    }
    let res = (|| {
        fs::rename(&staged, &bin)?;
        fs::write(p.version_file(), format!("{version}\n"))?;
        fs::create_dir_all(&p.data)?;
        fs::write(p.marker(), format!("{version}\n"))
    })();
    if let Err(e) = res {
        let _ = fs::remove_file(p.marker());
        let _ = fs::remove_file(&staged);
        if had_bin {
            let _ = fs::rename(&prev, &bin);
        }
        if ver_prev.is_file() {
            let _ = fs::rename(&ver_prev, p.version_file());
        }
        return Err(e);
    }
    let _ = fs::remove_file(new_bin);
    if let Some(tgz) = overlay {
        let _ = fs::remove_file(tgz);
    }
    eprintln!("tfctl: update: software {version} installed, restarting");
    Ok(())
}

/// `dst` becomes another name for `src` (a hard link, or a copy on a file system without
/// them); `src` stays in place.
fn keep_copy(src: &Path, dst: &Path) -> io::Result<()> {
    let _ = fs::remove_file(dst);
    if fs::hard_link(src, dst).is_ok() {
        return Ok(());
    }
    fs::copy(src, dst).map(|_| ())
}

fn extract_overlay(p: &Paths, tgz: &Path) -> io::Result<()> {
    // `gunzip -c | tar -x`: the frame's BusyBox tar has no -z (measured 2026-10-22), and a
    // failed overlay blocked every software update.
    let ex: Vec<String> = OVERLAY_EXCLUDES
        .iter()
        .flat_map(|e| [format!("--exclude={e}"), format!("--exclude=./{e}")])
        .collect();
    let mut gz = Command::new("gunzip")
        .arg("-c")
        .arg(tgz)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let tar = Command::new("tar")
        .args(["-xf", "-", "-C"])
        .arg(&p.root)
        .args(&ex)
        .stdin(
            gz.stdout
                .take()
                .ok_or_else(|| io::Error::other("gunzip: no stdout"))?,
        )
        .output()?;
    let gz = gz.wait_with_output()?;
    if !gz.status.success() {
        return Err(io::Error::other(format!(
            "gunzip {}: {}",
            tgz.display(),
            String::from_utf8_lossy(&gz.stderr).trim()
        )));
    }
    if !tar.status.success() {
        return Err(io::Error::other(format!(
            "tar -x into {}: {}",
            p.root.display(),
            String::from_utf8_lossy(&tar.stderr).trim()
        )));
    }
    Ok(())
}

/// `etc/thinframe.version.prev`: the version `tfctl.prev` carries, restored with it.
fn version_prev(p: &Paths) -> PathBuf {
    p.version_file().with_extension("version.prev")
}

/// Version a crash-loop rollback took out (`/data/thinframe/software-failed`). The automatic
/// path does not install it again, or a binary that crashes at start would be downloaded and
/// rolled back forever. The `update-software` action still can.
pub fn failed_version(p: &Paths) -> Option<String> {
    fs::read_to_string(p.data.join("software-failed"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[derive(Debug, PartialEq)]
pub enum StartCheck {
    Normal,
    /// The previous binary was put back; the process should exit so init runs it.
    RolledBack(String),
}

/// Called once at `tfctl kiosk` start. Appends to the starts log and decides whether the
/// pending update has crashed the binary twice in a row.
pub fn record_start(p: &Paths) -> StartCheck {
    record_start_at(p, now_s())
}

pub fn record_start_at(p: &Paths, now: u64) -> StartCheck {
    let _ = fs::create_dir_all(&p.data);
    let log = p.data.join("starts");
    let mut starts: Vec<u64> = fs::read_to_string(&log)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect();
    let recent = starts
        .iter()
        .rev()
        .take(2)
        // A start logged after `now` is from before a reboot: the A33 has no RTC battery and
        // comes up in 1970, so with saturating_sub every earlier start would look recent.
        .filter(|&&t| t <= now && now - t <= CRASH_WINDOW_S)
        .count();
    starts.push(now);
    let keep = starts.len().saturating_sub(STARTS_KEEP);
    let text: String = starts[keep..].iter().map(|t| format!("{t}\n")).collect();
    let _ = fs::write(&log, text);
    let pending = fs::read_to_string(p.marker()).ok();
    match pending {
        Some(ver) if recent >= 2 => {
            let ver = ver.trim().to_string();
            match swap_back(p) {
                Ok(()) => {
                    let _ = fs::remove_file(p.marker());
                    let _ = fs::write(p.data.join("software-failed"), format!("{ver}\n"));
                    queue_event(
                        p,
                        "software-rollback",
                        &format!(
                            "{ver} crashed twice within {CRASH_WINDOW_S} s, tfctl.prev restored"
                        ),
                    );
                    StartCheck::RolledBack(ver)
                }
                Err(e) => {
                    eprintln!("tfctl: update: rollback failed: {e}");
                    StartCheck::Normal
                }
            }
        }
        _ => StartCheck::Normal,
    }
}

fn swap_back(p: &Paths) -> io::Result<()> {
    let bin = p.binary();
    let prev = bin.with_extension("prev");
    if !prev.exists() {
        return Err(io::Error::other("no tfctl.prev to restore"));
    }
    remount_root(p, true)?;
    // Keep the crashing binary as `tfctl.bad` for a look later, but never by moving it: the
    // rename of `.prev` over `tfctl` is the one step, atomic, so the path always has a binary.
    let _ = keep_copy(&bin, &bin.with_extension("bad"));
    let res = fs::rename(&prev, &bin);
    // The version file goes back with the binary, so heartbeats report what runs and the
    // agent sees the server's version as not installed.
    if res.is_ok() && version_prev(p).is_file() {
        let _ = fs::rename(version_prev(p), p.version_file());
    }
    sync_disks();
    remount_root(p, false)?;
    res
}

/// Events the agent sends right after connecting: a pending software update that has now
/// survived long enough to talk to the server is confirmed (marker removed).
pub fn confirm_software(p: &Paths) -> Option<(String, String)> {
    let ver = fs::read_to_string(p.marker()).ok()?;
    let _ = fs::remove_file(p.marker());
    let _ = fs::remove_file(p.binary().with_extension("bad"));
    Some(("software-updated".into(), ver.trim().to_string()))
}

/// Software version this root carries (written by `software()`; the image writes the initial one).
pub fn installed_version(p: &Paths) -> Option<String> {
    fs::read_to_string(p.version_file())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// `tfctl update [--force]`: fetch the config over HTTP and apply the software update it
/// names. The agent does the same automatically; this is for a shell session.
pub fn run(cfg: &Config, force: bool) -> io::Result<()> {
    if cfg.server_url.is_empty() {
        return Err(io::Error::other(
            "SERVER_URL is empty; nothing to update from",
        ));
    }
    let base = Url::parse(&cfg.server_url).ok_or_else(|| io::Error::other("bad SERVER_URL"))?;
    let url = base.with_path(&format!("/api/frames/{}/config", cfg.frame_id));
    let r = http::get(&url, &cfg.token, DOWNLOAD_TIMEOUT)?;
    if r.status != 200 {
        return Err(io::Error::other(format!("GET config: HTTP {}", r.status)));
    }
    let v = crate::json::parse(&String::from_utf8_lossy(&r.body)).map_err(io::Error::other)?;
    let p = Paths::system();
    let want = v.field_string("software_version").unwrap_or_default();
    if want.is_empty() {
        return Err(io::Error::other("config has no software_version"));
    }
    if !force && installed_version(&p).as_deref() == Some(want.as_str()) {
        println!("already at {want}");
        return Ok(());
    }
    software(cfg, &Artifacts::parse(&v), &want, &p)?;
    println!("installed {want}; restart the service (or reboot) to run it");
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::json;
    use crate::sha256::sha256_hex;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    pub fn tmp_base(tag: &str) -> PathBuf {
        let b = std::env::temp_dir().join(format!("tfctl-upd-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&b);
        fs::create_dir_all(&b).unwrap();
        b
    }

    /// Serve named artifacts over HTTP (one connection each, any order) and return a config.
    pub fn artifact_server(files: Vec<(&'static str, Vec<u8>)>) -> (Config, Artifacts) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let mut arts = HashMap::new();
        for (name, body) in &files {
            arts.insert(name.to_string(), (format!("{name}.bin"), sha256_hex(body)));
        }
        std::thread::spawn(move || {
            for _ in 0..files.len() {
                let (mut c, _) = l.accept().unwrap();
                let mut req = vec![0u8; 4096];
                let n = c.read(&mut req).unwrap();
                let req = String::from_utf8_lossy(&req[..n]).to_string();
                let path = req.split_whitespace().nth(1).unwrap().to_string();
                assert!(req.contains("Authorization: Bearer tok"));
                let body = files
                    .iter()
                    .find(|(n, _)| path == format!("/api/artifacts/{n}.bin"))
                    .map(|(_, b)| b.clone());
                match body {
                    Some(b) => {
                        write!(c, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", b.len())
                            .unwrap();
                        c.write_all(&b).unwrap();
                    }
                    None => {
                        write!(c, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n").unwrap()
                    }
                }
            }
        });
        let cfg = Config {
            server_url: format!("http://127.0.0.1:{port}"),
            token: "tok".into(),
            ..Config::default()
        };
        (cfg, Artifacts(arts))
    }

    fn tar_gz(dir: &Path, members: &[(&str, &str)]) -> Vec<u8> {
        let src = dir.join("ovl");
        for (path, content) in members {
            let f = src.join(path);
            fs::create_dir_all(f.parent().unwrap()).unwrap();
            fs::write(f, content).unwrap();
        }
        let tgz = dir.join("overlay.tar.gz");
        let st = Command::new("tar")
            .args([
                "-czf",
                tgz.to_str().unwrap(),
                "-C",
                src.to_str().unwrap(),
                ".",
            ])
            .status()
            .unwrap();
        assert!(st.success());
        fs::read(tgz).unwrap()
    }

    #[test]
    fn parses_artifact_map() {
        let v = json::parse(&format!(
            r#"{{"artifacts":{{"tfctl":"tfctl-armv7:{0}","bad":"x","evil":"../x:{0}","system":"system-1.tar:{1}"}}}}"#,
            "a".repeat(64),
            "B".repeat(64)
        ))
        .unwrap();
        let a = Artifacts::parse(&v);
        assert_eq!(
            a.get("tfctl").unwrap(),
            ("tfctl-armv7", "a".repeat(64).as_str())
        );
        assert_eq!(a.get("system").unwrap().1, "b".repeat(64));
        assert!(a.get("bad").is_err());
        assert!(a.get("evil").is_err());
        assert!(Artifacts::parse(&Value::obj()).0.is_empty());
    }

    #[test]
    fn software_update_installs_and_keeps_prev_and_excludes() {
        let base = tmp_base("sw");
        let p = Paths::under(&base);
        fs::create_dir_all(p.root.join("usr/bin")).unwrap();
        fs::create_dir_all(p.root.join("etc")).unwrap();
        fs::write(p.binary(), b"old binary").unwrap();
        fs::write(p.root.join("etc/thinframe.conf"), "FRAME_ID=keep\n").unwrap();
        let ovl = tar_gz(
            &base,
            &[
                ("etc/thinframe.conf", "FRAME_ID=clobbered\n"),
                ("etc/motd", "hello\n"),
                ("var/lib/iwd/x.psk", "secret"),
            ],
        );
        let (cfg, arts) =
            artifact_server(vec![("tfctl", b"new binary".to_vec()), ("overlay", ovl)]);
        software(&cfg, &arts, "2026.10.2", &p).unwrap();
        assert_eq!(fs::read(p.binary()).unwrap(), b"new binary");
        assert_eq!(
            fs::read(p.binary().with_extension("prev")).unwrap(),
            b"old binary"
        );
        assert_eq!(
            fs::metadata(p.binary()).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            fs::read_to_string(p.root.join("etc/motd")).unwrap(),
            "hello\n"
        );
        assert_eq!(
            fs::read_to_string(p.root.join("etc/thinframe.conf")).unwrap(),
            "FRAME_ID=keep\n"
        );
        assert!(!p.root.join("var/lib/iwd/x.psk").exists());
        assert_eq!(installed_version(&p).unwrap(), "2026.10.2");
        assert_eq!(fs::read_to_string(p.marker()).unwrap().trim(), "2026.10.2");
        assert!(!p.tmp().join("tfctl.new").exists());
        // A connect confirms it.
        assert_eq!(
            confirm_software(&p),
            Some(("software-updated".into(), "2026.10.2".into()))
        );
        assert!(!p.marker().exists());
        assert_eq!(confirm_software(&p), None);
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn bad_sha_leaves_everything_alone() {
        let base = tmp_base("sha");
        let p = Paths::under(&base);
        fs::create_dir_all(p.root.join("usr/bin")).unwrap();
        fs::write(p.binary(), b"old binary").unwrap();
        let (cfg, mut arts) = artifact_server(vec![("tfctl", b"new binary".to_vec())]);
        arts.0.get_mut("tfctl").unwrap().1 = "0".repeat(64);
        let e = software(&cfg, &arts, "x", &p).unwrap_err();
        assert!(e.to_string().contains("sha256 mismatch"), "{e}");
        assert_eq!(fs::read(p.binary()).unwrap(), b"old binary");
        assert!(!p.marker().exists());
        assert!(software(&cfg, &Artifacts::default(), "x", &p).is_err());
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn two_fast_crashes_after_update_roll_back() {
        let base = tmp_base("rb");
        let p = Paths::under(&base);
        fs::create_dir_all(p.root.join("usr/bin")).unwrap();
        fs::write(p.binary(), b"new").unwrap();
        fs::write(p.binary().with_extension("prev"), b"old").unwrap();
        // No marker: fast restarts are just cog trouble, never a rollback.
        assert_eq!(record_start_at(&p, 1000), StartCheck::Normal);
        assert_eq!(record_start_at(&p, 1005), StartCheck::Normal);
        assert_eq!(record_start_at(&p, 1010), StartCheck::Normal);
        fs::write(p.marker(), "9.9\n").unwrap();
        // Two starts long ago do not count.
        assert_eq!(record_start_at(&p, 5000), StartCheck::Normal);
        assert_eq!(record_start_at(&p, 5010), StartCheck::Normal);
        assert_eq!(
            record_start_at(&p, 5020),
            StartCheck::RolledBack("9.9".into())
        );
        assert_eq!(fs::read(p.binary()).unwrap(), b"old");
        assert!(!p.marker().exists());
        let ev = drain_events(&p);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].0, "software-rollback");
        assert!(ev[0].1.contains("9.9"));
        assert!(drain_events(&p).is_empty());
        let log = fs::read_to_string(p.data.join("starts")).unwrap();
        assert_eq!(log.lines().count(), 6);
        fs::remove_dir_all(&base).unwrap();
    }

    fn old_install(base: &Path) -> Paths {
        let p = Paths::under(base);
        fs::create_dir_all(p.root.join("usr/bin")).unwrap();
        fs::create_dir_all(p.root.join("etc")).unwrap();
        fs::create_dir_all(p.tmp()).unwrap();
        fs::write(p.binary(), b"old binary").unwrap();
        fs::write(p.binary().with_extension("prev"), b"older binary").unwrap();
        fs::write(p.version_file(), "1.0\n").unwrap();
        fs::write(p.tmp().join("tfctl.new"), b"new binary").unwrap();
        p
    }

    #[test]
    fn failed_overlay_leaves_old_binary_and_no_marker() {
        let base = tmp_base("badtar");
        let p = old_install(&base);
        let tgz = p.tmp().join("overlay.tar.gz");
        fs::write(&tgz, b"not a tarball").unwrap();
        let e = install(&p, &p.tmp().join("tfctl.new"), Some(&tgz), "2.0").unwrap_err();
        assert!(e.to_string().contains("tar"), "{e}");
        assert_eq!(fs::read(p.binary()).unwrap(), b"old binary");
        assert_eq!(
            fs::read(p.binary().with_extension("prev")).unwrap(),
            b"older binary",
            "known-good prev kept"
        );
        assert!(!p.binary().with_extension("new").exists());
        assert!(!p.marker().exists());
        assert_eq!(installed_version(&p).unwrap(), "1.0");
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn failure_after_the_swap_puts_the_old_binary_back() {
        let base = tmp_base("postswap");
        let p = old_install(&base);
        // A directory where the marker goes: the last write fails after the swap.
        fs::create_dir_all(p.marker()).unwrap();
        assert!(install(&p, &p.tmp().join("tfctl.new"), None, "2.0").is_err());
        assert_eq!(fs::read(p.binary()).unwrap(), b"old binary");
        assert_eq!(installed_version(&p).unwrap(), "1.0");
        assert!(!p.binary().with_extension("new").exists());
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn rollback_restores_the_version_file() {
        let base = tmp_base("rbver");
        let p = old_install(&base);
        install(&p, &p.tmp().join("tfctl.new"), None, "2.0").unwrap();
        assert_eq!(installed_version(&p).unwrap(), "2.0");
        assert_eq!(fs::read_to_string(version_prev(&p)).unwrap(), "1.0\n");
        assert_eq!(failed_version(&p), None);
        assert_eq!(record_start_at(&p, 100), StartCheck::Normal);
        assert_eq!(record_start_at(&p, 110), StartCheck::Normal);
        assert_eq!(
            record_start_at(&p, 120),
            StartCheck::RolledBack("2.0".into())
        );
        assert_eq!(fs::read(p.binary()).unwrap(), b"old binary");
        assert_eq!(installed_version(&p).unwrap(), "1.0");
        assert!(!version_prev(&p).exists());
        assert_eq!(failed_version(&p).as_deref(), Some("2.0"));
        fs::remove_dir_all(&base).unwrap();
    }

    /// Power cut after a good update: the next boot has no RTC and `now` is tiny, so the earlier
    /// starts lie in the future. They are not a crash loop.
    #[test]
    fn starts_from_before_a_reboot_are_not_recent() {
        let base = tmp_base("rtc");
        let p = Paths::under(&base);
        fs::create_dir_all(p.root.join("usr/bin")).unwrap();
        fs::create_dir_all(&p.data).unwrap();
        fs::write(p.binary(), b"new").unwrap();
        fs::write(p.binary().with_extension("prev"), b"old").unwrap();
        fs::write(p.marker(), "9.9\n").unwrap();
        assert_eq!(record_start_at(&p, 1_760_000_000), StartCheck::Normal);
        assert_eq!(record_start_at(&p, 1_760_000_005), StartCheck::Normal);
        assert_eq!(record_start_at(&p, 20), StartCheck::Normal);
        assert_eq!(fs::read(p.binary()).unwrap(), b"new");
        assert!(p.marker().exists());
        fs::remove_dir_all(&base).unwrap();
    }
}
