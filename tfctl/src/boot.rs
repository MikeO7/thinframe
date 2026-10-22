//! `tfctl boot`: per-boot setup, run once by `/etc/init.d/S90thinframe` before init starts the
//! kiosk. Every step logs and carries on: a failed step must never stop the frame from booting.
//!  1. runtime dirs in /run (tmpfs)
//!  2. first boot: apply /boot/provision.txt with PFBOOT remounted rw for the duration
//!  3. grow the data partition (p4) to the end of the card, then the ext4 on it (online)
//!  4. hostname = FRAME_ID, so the router's DHCP table names the frame
//!  5. udhcpd on the USB-C link, so the Mac gets 192.168.7.1
//!  6. touch firmware from the frame's own Android eMMC when the image has none (`touchfw`)
//!
//! `tfctl boot fsck` is a separate, earlier step: the inittab line between `mount proc` and
//! `mount -a`. It first links `/dev/thinframe-data` and `/dev/thinframe-boot` (devtmpfs) to p4
//! and p1 of the disk `slot::find_disk` returns (our MBR signature only, never an eMMC); fstab
//! mounts those links, not `LABEL=`, because the frame's Android eMMC may hold a file system
//! labelled `data` (Android build tools can label userdata after its mount point; not checked
//! on this frame's dump), and BusyBox mount takes the first label match it finds. No thinframe disk: no links, and /data and /boot stay
//! unmounted rather than landing on something else. /data (p4) is never mounted at that
//! point, so `e2fsck -p` can repair it after a power cut; if preen mode gives up (exit bit 4),
//! one `e2fsck -y` pass follows. Then `mount -a` mounts /data and the bind mounts over it in
//! fstab order, before rcS starts iwd and dropbear, which read their state from those binds.
use crate::config;
use crate::provision;
use crate::slot;
use crate::touchfw;
use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, FileExt};
use std::path::Path;
use std::process::Command;

/// cog's runtime dir is private; the status page is readable by anyone who logs in.
const RUN_DIRS: [(&str, u32); 2] = [("/run/user/1000", 0o700), ("/run/thinframe", 0o755)];
/// Partition table entry 4 of the MBR.
const P4: usize = 446 + 3 * 16;
/// Grow only when it gains at least 1 MiB.
const MIN_GAIN_SECTORS: u64 = 2048;
/// What /etc/fstab mounts: (link in devtmpfs, partition number on our card).
pub const DEV_LINKS: [(&str, u8); 2] = [("/dev/thinframe-data", 4), ("/dev/thinframe-boot", 1)];

fn sh(cmd: &str, args: &[&str]) -> io::Result<()> {
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

fn step(name: &str, r: io::Result<()>) {
    match r {
        Ok(()) => note(&format!("{name}: ok")),
        Err(e) => note(&format!("{name}: {e}")),
    }
}

/// A boot line goes to the console and to the kernel log: /var/log is not up yet (and is
/// tmpfs anyway), dmesg keeps it for the whole boot, and the report (report.rs) carries dmesg.
pub fn note(line: &str) {
    eprintln!("tfctl: boot: {line}");
    if let Ok(mut k) = fs::OpenOptions::new().write(true).open("/dev/kmsg") {
        use std::io::Write;
        let _ = writeln!(k, "<6>tfctl: boot: {line}");
    }
}

pub fn run(conf: &str) -> io::Result<()> {
    for (d, mode) in RUN_DIRS {
        let _ = fs::DirBuilder::new().recursive(true).mode(mode).create(d);
    }
    if Path::new("/boot").join(provision::FILE).exists() {
        step("provision", provision_rw());
    }
    step("grow data", grow_data());
    let cfg = config::load(conf)?;
    if provision::valid_hostname(&cfg.frame_id) {
        step("hostname", sh("hostname", &[&cfg.frame_id]));
    }
    if Path::new("/sys/class/net/usb0").exists() {
        step("usb dhcp", sh("udhcpd", &["/etc/udhcpd.conf"]));
    }
    step("touch firmware", touchfw::ensure());
    Ok(())
}

/// `tfctl boot fsck`: check the data partition before `mount -a` mounts it. Never fails the boot:
/// whatever e2fsck says, `mount -a` runs next and tries.
pub fn fsck_data() -> io::Result<()> {
    let cands = slot::DISKS.map(Path::new);
    let disk = slot::find_disk(&cands).ok_or_else(|| io::Error::other("no thinframe disk"))?;
    for (link, n) in DEV_LINKS {
        let target = slot::part_n(&disk, n);
        let r = fs::remove_file(link)
            .or_else(|e| match e.kind() {
                io::ErrorKind::NotFound => Ok(()),
                _ => Err(e),
            })
            .and_then(|()| std::os::unix::fs::symlink(&target, link));
        if let Err(e) = r {
            note(&format!("link {link} -> {}: {e}", target.display()));
        }
    }
    let p4 = slot::part_n(&disk, 4).to_string_lossy().to_string();
    let mounts = fs::read_to_string("/proc/mounts").unwrap_or_default();
    if is_mounted(&mounts, &p4) {
        note(&format!("fsck: {p4} is mounted, skipped"));
        return Ok(());
    }
    let code = e2fsck("-p", &p4)?;
    note(&format!("fsck: e2fsck -p {p4}: exit {code}"));
    if needs_forced_pass(code) {
        let code = e2fsck("-y", &p4)?;
        note(&format!("fsck: e2fsck -y {p4}: exit {code}"));
    }
    Ok(())
}

/// e2fsck's exit status (-1 when killed by a signal). Output goes to the console.
fn e2fsck(mode: &str, dev: &str) -> io::Result<i32> {
    let st = Command::new("e2fsck").args([mode, dev]).status()?;
    Ok(st.code().unwrap_or(-1))
}

/// After `e2fsck -p`: bit 4 means errors left uncorrected (preen refused), so answer yes to all.
/// 0 to 3 are clean or fixed; 8 and up alone are operational errors that `-y` would not change.
pub fn needs_forced_pass(code: i32) -> bool {
    code >= 0 && code & 4 != 0
}

/// Whether `dev` is the source of any line in /proc/mounts.
pub fn is_mounted(mounts: &str, dev: &str) -> bool {
    mounts
        .lines()
        .any(|l| l.split_whitespace().next() == Some(dev))
}

fn provision_rw() -> io::Result<()> {
    sh("mount", &["-o", "remount,rw", "/boot"])?;
    let r = provision::apply(&provision::Targets::system("/boot")).map(|_| ());
    let ro = sh("mount", &["-o", "remount,ro", "/boot"]);
    r.and(ro)
}

/// New length (sectors) for partition 4 when the disk has room past its end, else None.
pub fn p4_new_len(mbr: &[u8; 512], disk_sectors: u64) -> Option<u32> {
    if !slot::mbr_is_ours(mbr) || mbr[P4 + 4] != 0x83 {
        return None;
    }
    let le = |o: usize| u32::from_le_bytes([mbr[o], mbr[o + 1], mbr[o + 2], mbr[o + 3]]) as u64;
    let (start, len) = (le(P4 + 8), le(P4 + 12));
    let want = disk_sectors.checked_sub(start)?.min(u32::MAX as u64);
    (want >= len + MIN_GAIN_SECTORS).then_some(want as u32)
}

/// Entry 4 rewritten with `len` sectors; CHS end set to the LBA-only marker.
pub fn p4_entry(mbr: &[u8; 512], len: u32) -> [u8; 16] {
    let mut e = [0u8; 16];
    e.copy_from_slice(&mbr[P4..P4 + 16]);
    e[5..8].copy_from_slice(&[0xfe, 0xff, 0xff]);
    e[12..16].copy_from_slice(&len.to_le_bytes());
    e
}

#[repr(C)]
struct BlkpgPartition {
    start: i64,
    length: i64,
    pno: i32,
    devname: [u8; 64],
    volname: [u8; 64],
}

#[repr(C)]
struct BlkpgIoctlArg {
    op: i32,
    flags: i32,
    datalen: i32,
    data: *mut BlkpgPartition,
}

extern "C" {
    fn ioctl(fd: i32, request: std::ffi::c_ulong, ...) -> i32;
}

/// Tell the kernel partition 4 changed size while the disk is in use (BLKPG_RESIZE_PARTITION).
fn blkpg_resize(disk: &fs::File, start: u64, len: u64) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    const BLKPG: std::ffi::c_ulong = 0x1269;
    const BLKPG_RESIZE_PARTITION: i32 = 3;
    let mut p = BlkpgPartition {
        start: (start * 512) as i64,
        length: (len * 512) as i64,
        pno: 4,
        devname: [0; 64],
        volname: [0; 64],
    };
    let mut a = BlkpgIoctlArg {
        op: BLKPG_RESIZE_PARTITION,
        flags: 0,
        datalen: std::mem::size_of::<BlkpgPartition>() as i32,
        data: &mut p,
    };
    // SAFETY: both structs match <linux/blkpg.h>, live for the call, and the fd is open.
    let rc = unsafe { ioctl(disk.as_raw_fd(), BLKPG, &mut a as *mut BlkpgIoctlArg) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn grow_data() -> io::Result<()> {
    let cands = slot::DISKS.map(Path::new);
    let disk = slot::find_disk(&cands).ok_or_else(|| io::Error::other("no thinframe disk"))?;
    let name = disk.file_name().unwrap().to_string_lossy().to_string();
    let sectors: u64 = fs::read_to_string(format!("/sys/block/{name}/size"))?
        .trim()
        .parse()
        .map_err(io::Error::other)?;
    let f = fs::OpenOptions::new().read(true).write(true).open(&disk)?;
    let mut mbr = [0u8; 512];
    f.read_exact_at(&mut mbr, 0)?;
    if let Some(len) = p4_new_len(&mbr, sectors) {
        let start = u32::from_le_bytes([mbr[P4 + 8], mbr[P4 + 9], mbr[P4 + 10], mbr[P4 + 11]]);
        f.write_all_at(&p4_entry(&mbr, len), P4 as u64)?;
        f.sync_all()?;
        blkpg_resize(&f, start as u64, len as u64)?;
    }
    // The file system is grown separately from the table, so a power cut between the two (or
    // a failed resize2fs) is finished on a later boot instead of leaving /data at 64 MiB.
    let p4 = slot::part_n(&disk, 4);
    let p4_name = p4.file_name().unwrap().to_string_lossy().to_string();
    let part_sectors: u64 = fs::read_to_string(format!("/sys/class/block/{p4_name}/size"))?
        .trim()
        .parse()
        .map_err(io::Error::other)?;
    let mut sb = [0u8; 1024];
    fs::File::open(&p4)?.read_exact_at(&mut sb, 1024)?;
    if ext4_lags_partition(&sb, part_sectors) {
        sh("resize2fs", &[&p4.to_string_lossy()])?;
        note(&format!("data partition now {} MiB", part_sectors / 2048));
    }
    Ok(())
}

/// True when the ext4 superblock `sb` (the 1024 bytes at offset 1024 of the partition)
/// describes a file system at least 1 MiB smaller than a partition of `part_sectors`.
/// Not ext4 (bad magic): false, so resize2fs never runs on something unexpected.
pub fn ext4_lags_partition(sb: &[u8; 1024], part_sectors: u64) -> bool {
    let le32 = |o: usize| u32::from_le_bytes([sb[o], sb[o + 1], sb[o + 2], sb[o + 3]]) as u64;
    if u16::from_le_bytes([sb[0x38], sb[0x39]]) != 0xef53 {
        return false;
    }
    let log = le32(0x18);
    if log > 6 {
        return false; // block size above 64 KiB: not a superblock we wrote
    }
    let mut blocks = le32(0x04);
    if le32(0x60) & 0x80 != 0 {
        blocks |= le32(0x150) << 32; // INCOMPAT_64BIT: s_blocks_count_hi
    }
    let fs_sectors = blocks * ((1024u64 << log) / 512);
    part_sectors >= fs_sectors + MIN_GAIN_SECTORS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mbr(start: u32, len: u32) -> [u8; 512] {
        let mut m = [0u8; 512];
        m[440..444].copy_from_slice(&slot::DISK_SIG.to_le_bytes());
        m[P4 + 4] = 0x83;
        m[P4 + 8..P4 + 12].copy_from_slice(&start.to_le_bytes());
        m[P4 + 12..P4 + 16].copy_from_slice(&len.to_le_bytes());
        m[510] = 0x55;
        m[511] = 0xaa;
        m
    }

    #[test]
    fn grows_only_our_disk_and_only_when_it_gains() {
        let m = mbr(4_202_496, 131_072);
        assert_eq!(p4_new_len(&m, 31_116_288), Some(31_116_288 - 4_202_496));
        assert_eq!(p4_new_len(&m, 4_202_496 + 131_072), None);
        assert_eq!(p4_new_len(&m, 4_202_496 + 131_072 + 100), None);
        assert_eq!(p4_new_len(&m, 1000), None);
        let mut other = m;
        other[440] ^= 1;
        assert_eq!(p4_new_len(&other, 31_116_288), None);
        let mut empty = m;
        empty[P4 + 4] = 0;
        assert_eq!(p4_new_len(&empty, 31_116_288), None);
    }

    fn superblock(blocks: u64, log_block: u32, bit64: bool) -> [u8; 1024] {
        let mut sb = [0u8; 1024];
        sb[0x04..0x08].copy_from_slice(&(blocks as u32).to_le_bytes());
        sb[0x18..0x1c].copy_from_slice(&log_block.to_le_bytes());
        sb[0x38..0x3a].copy_from_slice(&0xef53u16.to_le_bytes());
        if bit64 {
            sb[0x60..0x64].copy_from_slice(&0x80u32.to_le_bytes());
            sb[0x150..0x154].copy_from_slice(&((blocks >> 32) as u32).to_le_bytes());
        }
        sb
    }

    #[test]
    fn fs_grows_when_it_lags_the_partition_even_after_the_table_grew() {
        // The image's data.ext4: 64 MiB in 1 KiB blocks (mkfs.ext4 -d ... 64M).
        let seed = superblock(65_536, 0, false);
        assert!(
            !ext4_lags_partition(&seed, 131_072),
            "same size: nothing to do"
        );
        assert!(
            !ext4_lags_partition(&seed, 131_072 + 2047),
            "under 1 MiB: nothing to do"
        );
        assert!(ext4_lags_partition(&seed, 131_072 + 2048));
        // Table already grown to a 32 GB card by an earlier boot, resize2fs never ran.
        assert!(ext4_lags_partition(&seed, 60_000_000));
        // After resize2fs: 4 KiB blocks filling the partition.
        let grown = superblock(60_000_000 / 8, 2, true);
        assert!(!ext4_lags_partition(&grown, 60_000_000));
        // 64-bit block count (the high word counts).
        let big = superblock(1 << 32, 2, true);
        assert!(!ext4_lags_partition(&big, 1 << 34));
        // Not ext4, or nonsense block size: never resize.
        let mut junk = seed;
        junk[0x38] = 0;
        assert!(!ext4_lags_partition(&junk, 60_000_000));
        assert!(!ext4_lags_partition(&superblock(1, 40, false), 60_000_000));
    }

    #[test]
    fn forced_pass_only_when_preen_left_errors() {
        for code in [0, 1, 2, 3, 8, 16, 32, 128, -1] {
            assert!(!needs_forced_pass(code), "{code}");
        }
        for code in [4, 5, 6, 12] {
            assert!(needs_forced_pass(code), "{code}");
        }
    }

    #[test]
    fn mounted_matches_the_source_column_only() {
        let m = "/dev/root / ext4 ro 0 0\n/dev/vda4 /data ext4 rw,noatime 0 0\n";
        assert!(is_mounted(m, "/dev/vda4"));
        assert!(!is_mounted(m, "/dev/vda1"));
        assert!(!is_mounted(m, "/data"));
        assert!(!is_mounted("", "/dev/mmcblk0p4"));
    }

    #[test]
    fn entry_keeps_start_and_type() {
        let m = mbr(4_202_496, 131_072);
        let e = p4_entry(&m, 999_999);
        assert_eq!(e[4], 0x83);
        assert_eq!(&e[8..12], &4_202_496u32.to_le_bytes());
        assert_eq!(&e[12..16], &999_999u32.to_le_bytes());
        assert_eq!(&e[5..8], &[0xfe, 0xff, 0xff]);
    }
}
