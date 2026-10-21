//! `tfctl boot`: per-boot setup, run once by `/etc/init.d/S90thinframe` before init starts the
//! kiosk. Every step logs and carries on: a failed step must never stop the frame from booting.
//!  1. runtime dirs in /run (tmpfs)
//!  2. first boot: apply /boot/provision.txt with PFBOOT remounted rw for the duration
//!  3. grow the data partition (p4) to the end of the card, then the ext4 on it (online)
//!  4. hostname = FRAME_ID, so the router's DHCP table names the frame
//!  5. udhcpd on the USB-C link, so the Mac gets 192.168.7.1
use crate::config;
use crate::provision;
use crate::slot;
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
        Ok(()) => eprintln!("tfctl: boot: {name}: ok"),
        Err(e) => eprintln!("tfctl: boot: {name}: {e}"),
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
    Ok(())
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
    let Some(len) = p4_new_len(&mbr, sectors) else {
        return Ok(()); // already fills the card
    };
    let start = u32::from_le_bytes([mbr[P4 + 8], mbr[P4 + 9], mbr[P4 + 10], mbr[P4 + 11]]);
    f.write_all_at(&p4_entry(&mbr, len), P4 as u64)?;
    f.sync_all()?;
    blkpg_resize(&f, start as u64, len as u64)?;
    let p4 = slot::part_n(&disk, 4);
    sh("resize2fs", &[&p4.to_string_lossy()])?;
    eprintln!("tfctl: boot: data partition now {} MiB", len as u64 / 2048);
    Ok(())
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
