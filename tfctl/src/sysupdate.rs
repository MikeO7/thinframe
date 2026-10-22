//! System (OS) update, A/B (TASK 12). `system-<ver>.tar` = {zImage, rootfs.ext4.zst,
//! version.json} from `/api/artifacts/`. The standby slot's partition is rewritten with
//! `zstd -d | dd`, its kernel copied to `/boot/zImage.<slot>`, then `uEnv.txt` gets
//! `slot=<new> tries=3` and the frame reboots. U-Boot decrements `tries` before it loads the
//! kernel, and the start that takes it to 0 boots the other slot: tries=3 gives the new slot
//! two starts (tries left 2, then 1). The agent's first connect on the new slot writes `tries=0`.
//!
//! Safety: `dd` only ever targets a partition of the disk whose MBR carries our signature
//! (`slot::find_disk`), never the eMMC. `uEnv.txt` is rewritten only after the rootfs and
//! kernel are in place, so a failed download or write leaves the running slot untouched.
use crate::config::Config;
use crate::slot::{self, Slot, UEnv};
use crate::update::{self, sh, Artifacts, Paths};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Headroom the download + extraction needs on /data.
const MIN_FREE_KB: u64 = 300 * 1024;
/// U-Boot's `setexpr` prints its result in hex while `test -gt` reads decimal, so a count
/// above 9 would be misread after the first decrement ("10" - 1 = "f", and "f" -gt 0 is false).
const TRIES: u32 = 3;
const _: () = assert!(TRIES >= 2 && TRIES <= 9);

pub fn uenv_path(p: &Paths) -> std::path::PathBuf {
    p.boot.join("uEnv.txt")
}

/// The slot state U-Boot will act on: `uEnv.txt`, or `uEnv.bak` when that names no slot.
pub fn uenv(p: &Paths) -> io::Result<UEnv> {
    UEnv::load_effective(&uenv_path(p))
}

/// Slot the running root came from: kernel cmdline first, `uEnv.txt` as the fallback.
pub fn running_slot(p: &Paths) -> Option<Slot> {
    fs::read_to_string(&p.cmdline)
        .ok()
        .and_then(|c| slot::from_cmdline(&c))
        .or_else(|| uenv(p).ok()?.slot)
}

pub fn system_version(p: &Paths) -> Option<String> {
    fs::read_to_string(p.system_version_file())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// `df -Pk DIR`: available KiB from the second line, 4th column.
pub fn parse_df(out: &str) -> Option<u64> {
    out.lines().nth(1)?.split_whitespace().nth(3)?.parse().ok()
}

pub fn free_kb(dir: &Path) -> Option<u64> {
    let out = Command::new("df")
        .args(["-Pk", dir.to_str()?])
        .output()
        .ok()?;
    parse_df(&String::from_utf8_lossy(&out.stdout))
}

/// `zstd -d -c SRC | dd of=DEV bs=4M`. Both are BusyBox applets on the frame.
pub fn write_rootfs(src: &Path, dev: &Path, fsync: bool) -> io::Result<()> {
    let mut zstd = Command::new("zstd")
        .args(["-d", "-c", "-q", src.to_str().unwrap()])
        .stdout(Stdio::piped())
        .spawn()?;
    let pipe = zstd.stdout.take().unwrap();
    let mut cmd = Command::new("dd");
    cmd.arg(format!("of={}", dev.to_str().unwrap()))
        .arg("bs=4194304")
        .stdin(Stdio::from(pipe))
        .stderr(Stdio::piped());
    if fsync {
        cmd.arg("conv=fsync");
    }
    let dd = cmd.output();
    // The Command owns the pipe's read end. Drop it now, or a dd that quit early (ENOSPC, a
    // device that will not open) leaves zstd blocked on a full pipe and this thread waits forever.
    drop(cmd);
    let dd = dd?;
    if !dd.status.success() {
        let _ = zstd.kill();
    }
    let z = zstd.wait()?;
    if !dd.status.success() {
        return Err(io::Error::other(format!(
            "dd of={}: {}",
            dev.display(),
            String::from_utf8_lossy(&dd.stderr).trim()
        )));
    }
    if !z.success() {
        return Err(io::Error::other(format!("zstd -d {}: {z}", src.display())));
    }
    Ok(())
}

/// The card's whole-disk device, found by our MBR signature. Never the eMMC.
fn card(p: &Paths) -> io::Result<PathBuf> {
    let disks: Vec<&Path> = p.disks.iter().map(PathBuf::as_path).collect();
    slot::find_disk(&disks).ok_or_else(|| {
        io::Error::other("no disk with the thinframe MBR signature; refusing to write")
    })
}

/// Mount PFBOOT read-write at /boot (remount if it is already there). When /boot is not
/// mounted, the partition comes from `card`, so a guess can never land on the eMMC.
/// No-op in tests.
fn boot_rw(p: &Paths) -> io::Result<()> {
    if !p.is_real() {
        return Ok(());
    }
    if sh("mount", &["-o", "remount,rw", "/boot"]).is_ok() {
        return Ok(());
    }
    let dev = slot::part_n(&card(p)?, 1);
    sh(
        "mount",
        &["-t", "vfat", "-o", "rw", &dev.to_string_lossy(), "/boot"],
    )
}

fn boot_ro(p: &Paths) {
    update::sync_disks();
    if p.is_real() {
        let _ = sh("mount", &["-o", "remount,ro", "/boot"]);
    }
}

/// Download, verify, write the standby slot, point U-Boot at it. Returns the new slot; the
/// caller acks and reboots. Idempotent: a version that is already running is a no-op.
pub fn system(cfg: &Config, arts: &Artifacts, version: &str, p: &Paths) -> io::Result<Slot> {
    let current =
        running_slot(p).ok_or_else(|| io::Error::other("cannot tell which slot is running"))?;
    if system_version(p).as_deref() == Some(version) {
        return Err(io::Error::other(format!(
            "system {version} is already running on slot {}",
            current.name()
        )));
    }
    // While U-Boot still counts tries for this slot, the other one is the only image known to
    // boot; overwriting it now could leave no good slot at all.
    if let Ok(u) = uenv(p) {
        if u.tries > 0 && u.slot == Some(current) {
            return Err(io::Error::other(format!(
                "slot {} is not confirmed yet (tries={}); refusing to overwrite the other slot",
                current.name(),
                u.tries
            )));
        }
    }
    let (file, sha) = arts.get("system")?;
    let disk = card(p)?;
    let standby = current.other();
    let dev = slot::part_dev(&disk, standby);
    fs::create_dir_all(p.tmp())?;
    if let Some(free) = free_kb(&p.tmp()) {
        if free < MIN_FREE_KB {
            return Err(io::Error::other(format!(
                "{} KiB free on {}, need {MIN_FREE_KB}",
                free,
                p.tmp().display()
            )));
        }
    }
    let tar = p.tmp().join("system.tar");
    let dir = p.tmp().join("system");
    let _ = fs::remove_dir_all(&dir);
    let res = (|| {
        update::fetch(cfg, file, sha, &tar)?;
        fs::create_dir_all(&dir)?;
        sh(
            "tar",
            &["-xf", tar.to_str().unwrap(), "-C", dir.to_str().unwrap()],
        )?;
        let _ = fs::remove_file(&tar);
        let (zimage, rootfs) = (dir.join("zImage"), dir.join("rootfs.ext4.zst"));
        for f in [&zimage, &rootfs] {
            if !f.is_file() {
                return Err(io::Error::other(format!(
                    "{file} lacks {}",
                    f.file_name().unwrap().to_string_lossy()
                )));
            }
        }
        eprintln!(
            "tfctl: update: writing {} to {} (slot {})",
            file,
            dev.display(),
            standby.name()
        );
        write_rootfs(&rootfs, &dev, p.is_real())?;
        boot_rw(p)?;
        let res = (|| {
            fs::copy(&zimage, p.boot.join(format!("zImage.{}", standby.name())))?;
            let u = uenv(p).unwrap_or_else(|_| UEnv::parse(""));
            u.save(&uenv_path(p), standby, TRIES)
        })();
        boot_ro(p);
        res
    })();
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_file(&tar);
    res?;
    eprintln!(
        "tfctl: update: system {version} on slot {}, tries={TRIES}; rebooting",
        standby.name()
    );
    Ok(standby)
}

/// `rollback` action: boot the other slot next time. It gets the same retry counter as a fresh
/// update: if it cannot reach the server, U-Boot comes back here instead of looping on it;
/// `confirm_slot` zeroes the counter once it has connected. Refused while the other slot has
/// no kernel on PFBOOT (a fresh card: slot b was never written), since there is nothing there
/// to roll back to; U-Boot would only spend tries on the extlinux fallback label.
pub fn rollback(p: &Paths) -> io::Result<Slot> {
    let current =
        running_slot(p).ok_or_else(|| io::Error::other("cannot tell which slot is running"))?;
    let target = current.other();
    let kernel = p.boot.join(format!("zImage.{}", target.name()));
    if !kernel.is_file() {
        return Err(io::Error::other(format!(
            "slot {} was never installed ({} missing); nothing to roll back to",
            target.name(),
            kernel.display()
        )));
    }
    boot_rw(p)?;
    let res = uenv(p)
        .unwrap_or_else(|_| UEnv::parse(""))
        .save(&uenv_path(p), target, TRIES);
    boot_ro(p);
    res?;
    Ok(target)
}

/// Version the automatic path last wrote to the standby slot (`/data/thinframe/system-attempt`).
/// If we are not running it, that boot failed and U-Boot rolled back: never auto-retry it, or a
/// broken bundle would be flashed again on every boot. The `update-system` action still can.
pub fn attempted(p: &Paths) -> Option<String> {
    fs::read_to_string(p.data.join("system-attempt"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn mark_attempt(p: &Paths, version: &str) -> io::Result<()> {
    fs::create_dir_all(&p.data)?;
    fs::write(p.data.join("system-attempt"), format!("{version}\n"))
}

/// On every agent connect: if U-Boot is still counting tries for the slot we booted from,
/// the slot works; write `tries=0` and report it.
pub fn confirm_slot(p: &Paths) -> Option<(String, String)> {
    let u = uenv(p).ok()?;
    let running = running_slot(p)?;
    if u.tries == 0 || u.slot != Some(running) {
        return None;
    }
    boot_rw(p).ok()?;
    let res = u.save(&uenv_path(p), running, 0);
    boot_ro(p);
    res.ok()?;
    Some((
        "slot-confirmed".into(),
        format!(
            "slot {} booted and reached the server; tries reset (system {})",
            running.name(),
            system_version(p).unwrap_or_else(|| "?".into())
        ),
    ))
}

/// `reboot` after a short delay so the ack leaves first.
pub fn reboot_soon() {
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(2));
        update::sync_disks();
        let _ = Command::new("reboot").spawn();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::tests::{artifact_server, tmp_base};

    fn fake_card(base: &Path) -> Paths {
        let p = Paths::under(base);
        fs::create_dir_all(&p.boot).unwrap();
        fs::create_dir_all(p.root.join("etc")).unwrap();
        fs::write(&p.cmdline, "root=PARTUUID=50463037-02 ro quiet\n").unwrap();
        fs::write(uenv_path(&p), "# boot\nslot=a\ntries=0\n").unwrap();
        fs::write(p.system_version_file(), "2026.10.1\n").unwrap();
        let mut mbr = vec![0u8; 512];
        mbr[440..444].copy_from_slice(&slot::DISK_SIG.to_le_bytes());
        fs::write(&p.disks[0], vec![0u8; 512]).unwrap(); // "eMMC": no signature
        fs::write(&p.disks[1], mbr).unwrap();
        p
    }

    /// Every /boot mount goes through `card`: with no signed disk it refuses, it never guesses mmcblk0.
    #[test]
    fn card_is_the_signed_disk_or_nothing() {
        let p = fake_card(&tmp_base("card"));
        assert_eq!(card(&p).unwrap(), p.disks[1]);
        fs::write(&p.disks[1], vec![0u8; 512]).unwrap();
        let err = card(&p).unwrap_err().to_string();
        assert!(err.contains("refusing"), "{err}");
    }

    #[test]
    fn df_and_slot_helpers() {
        let out = "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/mmcblk0p4 1000000 100 899900 1% /data\n";
        assert_eq!(parse_df(out), Some(899900));
        assert_eq!(parse_df("junk"), None);
        let base = tmp_base("slot");
        let p = fake_card(&base);
        assert_eq!(running_slot(&p), Some(Slot::A));
        fs::remove_file(&p.cmdline).unwrap();
        assert_eq!(running_slot(&p), Some(Slot::A));
        fs::remove_file(uenv_path(&p)).unwrap();
        assert_eq!(running_slot(&p), None);
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn rollback_and_confirm() {
        let base = tmp_base("rbk");
        let p = fake_card(&base);
        assert_eq!(confirm_slot(&p), None, "tries=0: nothing to confirm");
        // Fresh card: slot b was never written, so there is nothing to roll back to.
        let e = rollback(&p).unwrap_err().to_string();
        assert!(e.contains("never installed"), "{e}");
        let u = UEnv::load(&uenv_path(&p)).unwrap();
        assert_eq!((u.slot, u.tries), (Some(Slot::A), 0), "uEnv untouched");
        fs::write(p.boot.join("zImage.b"), b"kernel").unwrap();
        assert_eq!(rollback(&p).unwrap(), Slot::B);
        let u = UEnv::load(&uenv_path(&p)).unwrap();
        assert_eq!((u.slot, u.tries), (Some(Slot::B), TRIES));
        // Pretend U-Boot booted b with tries pending and we are now running b.
        fs::write(&p.cmdline, "root=PARTUUID=50463037-03").unwrap();
        u.save(&uenv_path(&p), Slot::B, 2).unwrap();
        let ev = confirm_slot(&p).unwrap();
        assert_eq!(ev.0, "slot-confirmed");
        assert!(ev.1.contains("slot b"));
        assert_eq!(UEnv::load(&uenv_path(&p)).unwrap().tries, 0);
        // Running a while uEnv says b with tries left: U-Boot fell back, do not touch it.
        fs::write(&p.cmdline, "root=PARTUUID=50463037-02").unwrap();
        u.save(&uenv_path(&p), Slot::B, 1).unwrap();
        assert_eq!(confirm_slot(&p), None);
        // uEnv.txt lost (power cut mid-rename): U-Boot boots from uEnv.bak, and so does the
        // confirm, which writes a fresh uEnv.txt.
        fs::write(&p.cmdline, "root=PARTUUID=50463037-03").unwrap();
        fs::write(uenv_path(&p).with_extension("bak"), "slot=b\ntries=2\n").unwrap();
        fs::remove_file(uenv_path(&p)).unwrap();
        assert_eq!(confirm_slot(&p).unwrap().0, "slot-confirmed");
        let u = UEnv::load(&uenv_path(&p)).unwrap();
        assert_eq!((u.slot, u.tries), (Some(Slot::B), 0));
        fs::remove_dir_all(&base).unwrap();
    }

    fn have(tool: &str) -> bool {
        Command::new(tool).arg("--version").output().is_ok()
    }

    #[test]
    fn system_update_writes_standby_slot() {
        if !have("zstd") {
            eprintln!("zstd not installed, skipping");
            return;
        }
        let base = tmp_base("sys");
        let p = fake_card(&base);
        let src = base.join("bundle");
        fs::create_dir_all(&src).unwrap();
        let rootfs: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        fs::write(src.join("rootfs.ext4"), &rootfs).unwrap();
        assert!(Command::new("zstd")
            .args(["-q", "--rm", src.join("rootfs.ext4").to_str().unwrap()])
            .status()
            .unwrap()
            .success());
        fs::write(src.join("zImage"), b"kernel").unwrap();
        fs::write(src.join("version.json"), r#"{"version":"2026.10.2"}"#).unwrap();
        let tar = base.join("system.tar");
        assert!(Command::new("tar")
            .args([
                "-cf",
                tar.to_str().unwrap(),
                "-C",
                src.to_str().unwrap(),
                "zImage",
                "rootfs.ext4.zst",
                "version.json"
            ])
            .status()
            .unwrap()
            .success());
        let (cfg, arts) = artifact_server(vec![("system", fs::read(&tar).unwrap())]);
        let e = system(&cfg, &arts, "2026.10.1", &p).unwrap_err();
        assert!(e.to_string().contains("already running"));
        // Running a while U-Boot still counts tries for a: b is the only known-good image.
        fs::write(uenv_path(&p), "# boot\nslot=a\ntries=2\n").unwrap();
        let e = system(&cfg, &arts, "2026.10.2", &p).unwrap_err();
        assert!(e.to_string().contains("not confirmed"), "{e}");
        fs::write(uenv_path(&p), "# boot\nslot=a\ntries=0\n").unwrap();
        assert_eq!(system(&cfg, &arts, "2026.10.2", &p).unwrap(), Slot::B);
        let dev = slot::part_dev(&p.disks[1], Slot::B);
        assert_eq!(
            fs::read(&dev).unwrap(),
            rootfs,
            "rootfs landed on the card's p3"
        );
        assert!(
            !slot::part_dev(&p.disks[0], Slot::B).exists(),
            "eMMC untouched"
        );
        assert_eq!(fs::read(p.boot.join("zImage.b")).unwrap(), b"kernel");
        let u = UEnv::load(&uenv_path(&p)).unwrap();
        assert_eq!((u.slot, u.tries), (Some(Slot::B), 3));
        assert!(u.render(Slot::B, 3).starts_with("# boot\n"));
        assert!(!p.tmp().join("system").exists());
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn system_update_refuses_without_our_disk() {
        let base = tmp_base("nodisk");
        let p = fake_card(&base);
        fs::write(&p.disks[1], vec![0u8; 512]).unwrap();
        let (cfg, arts) = artifact_server(vec![]);
        let mut arts = arts;
        arts.0
            .insert("system".into(), ("system-x.tar".into(), "0".repeat(64)));
        let e = system(&cfg, &arts, "2026.10.2", &p).unwrap_err();
        assert!(e.to_string().contains("MBR signature"), "{e}");
        fs::remove_dir_all(&base).unwrap();
    }
}
