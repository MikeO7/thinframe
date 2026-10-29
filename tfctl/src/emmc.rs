//! `tfctl emmc`: thinframe on the frame's internal eMMC, so it runs with no card (TASKS E).
//! The boot ROM tries the card before the eMMC, so a card with U-Boot always wins: the card the
//! frame was installed from stays the rescue card (docs/decisions.md "Running from the eMMC").
//!
//! - `status`: what is on the eMMC and which disk this system booted from.
//! - `backup`: the whole eMMC, raw, to stdout (pipe it through zstd to the Mac or the server).
//!   Its SHA-256 goes to stderr and to `RECORD` once the last byte is out.
//! - `install --backup-sha256 HEX [--files DIR]`: the card's layout on the eMMC with the eMMC
//!   signature (`slot::EMMC_SIG`): the boot partition, the running root in both slots, a fresh
//!   /data with a copy of this one, and U-Boot with an eMMC slot. Runs only from the card, only when the eMMC is
//!   unmounted, the running slot is confirmed, and HEX is the hash `backup` recorded for this
//!   eMMC (not needed when the eMMC already holds thinframe).
//!
//! The vendor set the eMMC to boot from its boot0 hardware partition (EXT_CSD PARTITION_CONFIG
//! 0x48, BOOT_ACK on, BOOT_BUS_CONDITIONS 0x01; boot0 holds a copy of Android's boot0) [V
//! 2026-10-26]. U-Boot's sunxi code says the boot ROM tries such a boot partition before the
//! user area. The first installs switched that off (PARTITION_CONFIG 0) and left U-Boot only in
//! the user area at 8 KiB; with the card out the frame never started (the eMMC's /data was never
//! mounted) [V 2026-10-29]. So the install now ends the way the vendor's eMMC was set up, the one
//! layout this board is known to start from: U-Boot at the start of boot0 too, and
//! PARTITION_CONFIG 0x48 (boot from boot0, BOOT_ACK) with BOOT_BUS_CONDITIONS 0x01 (4-bit SDR).
//! The copy at 8 KiB stays: a boot ROM that reads the user area finds it, and the SPL loads
//! U-Boot proper from the user area either way (raw mode, sector 80).
//!
//! Write order: the vendor bootloader's eGON headers are cleared first, U-Boot is written last.
//! Until the last step the eMMC has no boot code, so the boot ROM skips it (card, else FEL):
//! a power cut or an error mid-install never leaves a half system that starts. Every copied
//! byte is read back from the eMMC (page cache dropped) and compared by hash.
use crate::sha256::Sha256;
use crate::slot::{self, Slot, UEnv};
use std::fs::{self, File, OpenOptions};
use std::io::{self, IsTerminal, Write};
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// Written by `backup` after a complete pass: `bytes=` and `sha256=` of the eMMC.
pub const RECORD: &str = "/data/thinframe/emmc-backup";
/// U-Boot with the eMMC slot (image/uboot-emmc.sh), boot.scr that reads `mmc_bootdev`, and the
/// DTBs with the eMMC enabled: post-build.sh puts them in the image. `--files` names another
/// directory with the same files (a frame whose image predates them).
const SHARE: &str = "/usr/share/thinframe/boot";
/// U-Boot sits 8 KiB into the disk; the boot ROM reads the eGON header there.
const UBOOT_OFFSET: u64 = 8192;
/// The layout needs 4 MiB + 256 MiB + 2 x 1 GiB before /data; ask for room for /data too.
const MIN_BYTES: u64 = 4 << 30;
const CHUNK: usize = 1 << 20;
const O_EXCL: i32 = 0o200;
const BLKRRPART: std::ffi::c_ulong = 0x125f;
/// _IOWR(MMC_BLOCK_MAJOR = 179, 0, struct mmc_ioc_cmd), <linux/mmc/ioctl.h>.
const MMC_IOC_CMD: std::ffi::c_ulong = 0xc048_b300;
/// EXT_CSD bytes (JEDEC JESD84): boot bus conditions, partition config.
pub const EXT_CSD_BOOT_BUS: usize = 177;
pub const EXT_CSD_PART_CONFIG: usize = 179;
/// Boot from boot0 with BOOT_ACK, access to the user area: the vendor's value on this frame.
pub const PART_CONFIG_BOOT0: u8 = 0x48;
/// 4-bit SDR boot bus, as the vendor left it (U-Boot: the boot ROM needs 0x01 or 0x09).
pub const BOOT_BUS_4BIT: u8 = 0x01;

/// What the install destroys and `backup` (the user area) does not cover, saved once before the
/// first change: boot0's first 64 KiB (the install zeroes them) and the EXT_CSD boot bytes (it
/// sets PARTITION_CONFIG to 0). An existing copy is never overwritten: a second install over
/// thinframe would save only zeros.
pub const ORIG_BOOT0: &str = "/data/thinframe/emmc-boot0-orig.bin";
pub const ORIG_CSD: &str = "/data/thinframe/emmc-ext-csd-orig.txt";
const BOOT0_CLEARED: usize = 64 << 10;

/// Held by `emmc install` for its whole run, with its pid: while a live tfctl holds it the agent
/// starts no software or system update, no rollback and no reboot (`reboot_soon` waits), since
/// each of them can change the root or the boot partition in the middle of the raw copy.
pub const INSTALL_LOCK: &str = "/run/thinframe/emmc-install";

type R<T> = io::Result<T>;

/// The install lock; removed on drop. A crash (panic=abort skips drop) leaves the file, which
/// `holder` then sees as stale: its pid is gone or no longer a tfctl.
pub struct InstallLock(PathBuf);

impl Drop for InstallLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// The pid in a lock file's text when that process still runs tfctl (`comm` reads its name).
pub fn live_holder(text: &str, comm: impl Fn(u32) -> Option<String>) -> Option<u32> {
    let pid: u32 = text.trim().parse().ok()?;
    (pid > 0 && comm(pid).is_some_and(|c| c.trim().starts_with("tfctl"))).then_some(pid)
}

fn proc_comm(pid: u32) -> Option<String> {
    fs::read_to_string(format!("/proc/{pid}/comm")).ok()
}

/// Who holds the install lock at `path`, if anyone alive does.
pub fn holder(path: &Path) -> Option<u32> {
    live_holder(&fs::read_to_string(path).ok()?, proc_comm)
}

/// Take the lock at `path` for `pid`; a stale file is replaced, a live holder refuses.
fn lock_with(path: &Path, pid: u32, comm: impl Fn(u32) -> Option<String>) -> R<InstallLock> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    for _ in 0..2 {
        match OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(mut f) => {
                f.write_all(format!("{pid}\n").as_bytes())?;
                return Ok(InstallLock(path.to_path_buf()));
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                let text = fs::read_to_string(path).unwrap_or_default();
                if let Some(p) = live_holder(&text, &comm) {
                    return Err(err(format!("another eMMC install is running (pid {p})")));
                }
                let _ = fs::remove_file(path);
            }
            Err(e) => return Err(e),
        }
    }
    Err(err(format!("cannot take {}", path.display())))
}

fn err(msg: impl Into<String>) -> io::Error {
    io::Error::other(msg.into())
}

pub fn run(args: &[String]) -> R<()> {
    match args.first().map(String::as_str) {
        Some("status") | None => status(),
        Some("backup") => backup(),
        Some("install") => {
            let opt = |k: &str| {
                args.iter()
                    .position(|a| a == k)
                    .and_then(|i| args.get(i + 1))
                    .cloned()
            };
            let sha = opt("--backup-sha256").map(|s| s.to_ascii_lowercase());
            let files = opt("--files").unwrap_or_else(|| SHARE.to_string());
            install(sha.as_deref(), Path::new(&files))
        }
        Some(x) => Err(err(format!(
            "emmc: unknown '{x}' (status | backup | install --backup-sha256 HEX [--files DIR])"
        ))),
    }
}

fn disks() -> Vec<&'static Path> {
    slot::DISKS.iter().map(Path::new).collect()
}

fn emmc_dev() -> R<PathBuf> {
    slot::find_emmc(&disks(), Path::new("/sys/block"))
        .ok_or_else(|| err("no eMMC block device (the DTB must enable mmc@1c11000)"))
}

fn sys_name(disk: &Path) -> String {
    disk.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn size_bytes(disk: &Path) -> R<u64> {
    let s = fs::read_to_string(format!("/sys/block/{}/size", sys_name(disk)))?;
    s.trim()
        .parse::<u64>()
        .map(|n| n * 512)
        .map_err(|e| err(format!("size of {}: {e}", disk.display())))
}

/// What a disk's first sectors say it holds.
#[derive(Debug, PartialEq)]
pub enum Holds {
    Thinframe,
    Card,
    Other { sig: u32, egon: bool },
}

pub fn holds(mbr: &[u8], egon: &[u8]) -> Holds {
    match slot::mbr_sig(mbr) {
        Some(slot::EMMC_SIG) => Holds::Thinframe,
        Some(slot::DISK_SIG) => Holds::Card,
        s => Holds::Other {
            sig: s.unwrap_or(0),
            egon: is_egon(egon),
        },
    }
}

/// An Allwinner boot0/SPL header: a branch, then "eGON.BT0".
pub fn is_egon(head: &[u8]) -> bool {
    head.get(4..12) == Some(b"eGON.BT0")
}

fn read_head(disk: &Path) -> R<([u8; 512], [u8; 16])> {
    let f = File::open(disk)?;
    let (mut mbr, mut egon) = ([0u8; 512], [0u8; 16]);
    f.read_exact_at(&mut mbr, 0)?;
    f.read_exact_at(&mut egon, UBOOT_OFFSET)?;
    Ok((mbr, egon))
}

fn status() -> R<()> {
    let from = if slot::running_from_emmc() {
        "eMMC"
    } else {
        "card"
    };
    println!("running from: {from}");
    let d = emmc_dev()?;
    let (mbr, egon) = read_head(&d)?;
    println!("eMMC: {} ({} bytes)", d.display(), size_bytes(&d)?);
    match holds(&mbr, &egon) {
        Holds::Thinframe => println!(
            "holds: thinframe (signature {:08x}, U-Boot {})",
            slot::EMMC_SIG,
            if is_egon(&egon) { "present" } else { "MISSING" }
        ),
        Holds::Card => println!("holds: a card image (card signature); not bootable as eMMC"),
        Holds::Other { sig, egon } => println!(
            "holds: not thinframe (signature {sig:08x}, vendor boot code {})",
            if egon { "present" } else { "absent" }
        ),
    }
    if let Ok(csd) = read_ext_csd(&File::open(&d)?) {
        println!(
            "EXT_CSD: PARTITION_CONFIG {}, BOOT_BUS_CONDITIONS {:#04x}",
            part_config_text(csd[EXT_CSD_PART_CONFIG]),
            csd[EXT_CSD_BOOT_BUS]
        );
    }
    let b0 = boot0_of(&d);
    let mut h = [0u8; 16];
    if File::open(&b0)
        .and_then(|f| f.read_exact_at(&mut h, 0))
        .is_ok()
    {
        println!(
            "{}: boot code {}",
            b0.display(),
            if is_egon(&h) { "present" } else { "absent" }
        );
    }
    match fs::read_to_string(RECORD) {
        Ok(r) => println!(
            "backup record: {}",
            r.split_whitespace().collect::<Vec<_>>().join(" ")
        ),
        Err(_) => println!("backup record: none ({RECORD})"),
    }
    Ok(())
}

/// The backup record's (bytes, sha256).
pub fn parse_record(text: &str) -> Option<(u64, String)> {
    let get = |k: &str| {
        text.lines()
            .find_map(|l| l.trim().strip_prefix(k)?.strip_prefix('='))
            .map(|v| v.trim().to_string())
    };
    let bytes = get("bytes")?.parse().ok()?;
    let sha = get("sha256")?.to_ascii_lowercase();
    (sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit())).then_some((bytes, sha))
}

fn backup() -> R<()> {
    let mut out = io::stdout().lock();
    if out.is_terminal() {
        return Err(err(
            "emmc backup writes the raw eMMC to stdout; pipe it somewhere",
        ));
    }
    let d = emmc_dev()?;
    let size = size_bytes(&d)?;
    let f = File::open(&d)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut off = 0u64;
    while off < size {
        let n = (size - off).min(CHUNK as u64) as usize;
        f.read_exact_at(&mut buf[..n], off)?;
        h.update(&buf[..n]);
        out.write_all(&buf[..n])?;
        off += n as u64;
        if off.is_multiple_of(1 << 30) {
            eprintln!(
                "tfctl: emmc backup: {} GiB of {:.1}",
                off >> 30,
                size as f64 / (1u64 << 30) as f64
            );
        }
    }
    out.flush()?;
    let sha = h.finish_hex();
    eprintln!("tfctl: emmc backup: {size} bytes, sha256 {sha}");
    fs::write(RECORD, format!("bytes={size}\nsha256={sha}\n"))?;
    Ok(())
}

/// MBR partition entry `i` (0-based): (start, length) in sectors.
pub fn entry(mbr: &[u8; 512], i: usize) -> (u64, u64) {
    let o = 446 + 16 * i;
    let le = |o: usize| u32::from_le_bytes([mbr[o], mbr[o + 1], mbr[o + 2], mbr[o + 3]]) as u64;
    (le(o + 8), le(o + 12))
}

/// The eMMC's MBR: the card's first three entries as they are, entry 4 to the end of the eMMC,
/// and the eMMC signature. Err when the card's table is not the layout we know.
pub fn emmc_mbr(card: &[u8; 512], emmc_sectors: u64) -> R<[u8; 512]> {
    if slot::mbr_sig(card) != Some(slot::DISK_SIG) || card[510..512] != [0x55, 0xaa] {
        return Err(err("the card's MBR is not thinframe's"));
    }
    let mut end = 0;
    for i in 0..4 {
        let (start, len) = entry(card, i);
        if start < end || len == 0 || (i == 0 && start * 512 < UBOOT_OFFSET + (1 << 20)) {
            return Err(err(format!(
                "card partition {} is not where it should be",
                i + 1
            )));
        }
        end = start + len;
    }
    let (p4, _) = entry(card, 3);
    if emmc_sectors < p4 + ((256 << 20) / 512) {
        return Err(err("the eMMC is too small for the card's layout"));
    }
    let len = (emmc_sectors - p4).min(u32::MAX as u64) as u32;
    let mut m = [0u8; 512];
    m[440..444].copy_from_slice(&slot::EMMC_SIG.to_le_bytes());
    m[446..510].copy_from_slice(&card[446..510]);
    let e4 = 446 + 48;
    m[e4 + 5..e4 + 8].copy_from_slice(&[0xfe, 0xff, 0xff]);
    m[e4 + 12..e4 + 16].copy_from_slice(&len.to_le_bytes());
    m[510] = 0x55;
    m[511] = 0xaa;
    Ok(m)
}

/// An extlinux.conf for the eMMC: every root on the card's signature moves to the eMMC's, and
/// the last-resort `min` label boots `pmic.dtb` instead. min.dtb has no eMMC node (it lacks the
/// PMIC regulators the eMMC needs, image/build.sh), so on the eMMC it would wait at rootwait
/// forever; pmic.dtb has the eMMC on and is a different file, so it still covers a full.dtb that
/// does not load, which is what the label is for. Its command line (debug log) stays.
pub fn retarget_extlinux(conf: &str) -> String {
    conf.replace(
        &format!("PARTUUID={:08x}-", slot::DISK_SIG),
        &format!("PARTUUID={:08x}-", slot::EMMC_SIG),
    )
    .lines()
    .map(|l| match l.trim() {
        "LABEL min" => l.replace("LABEL min", "LABEL pmic"),
        t if t.starts_with("FDT ") && t.ends_with("-min.dtb") => l.replace("-min.dtb", "-pmic.dtb"),
        _ => l.to_string(),
    })
    .collect::<Vec<_>>()
    .join("\n")
        + if conf.ends_with('\n') { "\n" } else { "" }
}

fn drop_caches() {
    let _ = Command::new("sync").status();
    let _ = fs::write("/proc/sys/vm/drop_caches", "3\n");
}

/// Hash of `len` bytes of `f` from `off`.
fn hash_range(f: &File, off: u64, len: u64) -> R<String> {
    let mut h = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    while done < len {
        let n = (len - done).min(CHUNK as u64) as usize;
        f.read_exact_at(&mut buf[..n], off + done)?;
        h.update(&buf[..n]);
        done += n as u64;
    }
    Ok(h.finish_hex())
}

/// Copy `len` bytes from `src` (offset 0) to `dst` at `off`; returns the source's hash.
fn copy_into(src: &File, dst: &File, off: u64, len: u64) -> R<String> {
    let mut h = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    while done < len {
        let n = (len - done).min(CHUNK as u64) as usize;
        src.read_exact_at(&mut buf[..n], done)?;
        h.update(&buf[..n]);
        dst.write_all_at(&buf[..n], off + done)?;
        done += n as u64;
    }
    Ok(h.finish_hex())
}

extern "C" {
    fn ioctl(fd: i32, request: std::ffi::c_ulong, ...) -> i32;
}

/// struct mmc_ioc_cmd, <linux/mmc/ioctl.h> (72 bytes on armv7 and aarch64).
#[repr(C)]
#[derive(Default)]
struct MmcIocCmd {
    write_flag: i32,
    is_acmd: i32,
    opcode: u32,
    arg: u32,
    response: [u32; 4],
    flags: u32,
    blksz: u32,
    blocks: u32,
    postsleep_min_us: u32,
    postsleep_max_us: u32,
    data_timeout_ns: u32,
    cmd_timeout_ms: u32,
    pad: u32,
    data_ptr: u64,
}
const _: () = assert!(std::mem::size_of::<MmcIocCmd>() == 72);

fn mmc_cmd(f: &File, c: &mut MmcIocCmd) -> R<()> {
    use std::os::fd::AsRawFd;
    // SAFETY: the struct matches the kernel's; data_ptr, when set, points at a live buffer of
    // blksz * blocks bytes for the duration of the call.
    if unsafe { ioctl(f.as_raw_fd(), MMC_IOC_CMD, c as *mut MmcIocCmd) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// CMD8 SEND_EXT_CSD: the card's 512-byte EXT_CSD register.
pub fn read_ext_csd(f: &File) -> R<[u8; 512]> {
    let mut b = [0u8; 512];
    let mut c = MmcIocCmd {
        opcode: 8,
        flags: 0xb5, // MMC_RSP_SPI_R1 | MMC_RSP_R1 | MMC_CMD_ADTC
        blksz: 512,
        blocks: 1,
        data_ptr: b.as_mut_ptr() as u64,
        ..Default::default()
    };
    mmc_cmd(f, &mut c)?;
    Ok(b)
}

/// CMD6 SWITCH, write byte: EXT_CSD[index] = value (what `mmc bootpart enable` sends).
fn write_ext_csd(f: &File, index: usize, value: u8) -> R<()> {
    let mut c = MmcIocCmd {
        write_flag: 1,
        opcode: 6,
        arg: (3 << 24) | ((index as u32) << 16) | ((value as u32) << 8),
        flags: 0x49d, // MMC_RSP_SPI_R1B | MMC_RSP_R1B | MMC_CMD_AC
        cmd_timeout_ms: 1000,
        ..Default::default()
    };
    mmc_cmd(f, &mut c)
}

/// "boot from boot0" etc. for a PARTITION_CONFIG value.
pub fn part_config_text(v: u8) -> String {
    let part = match (v >> 3) & 7 {
        0 => "boot from a partition: off".to_string(),
        7 => "boot from the user area".to_string(),
        n => format!("boot from boot{}", n - 1),
    };
    format!(
        "{v:#04x} ({part}, BOOT_ACK {})",
        if v & 0x40 != 0 { "on" } else { "off" }
    )
}

fn reread_partitions(f: &File) -> R<()> {
    use std::os::fd::AsRawFd;
    // SAFETY: BLKRRPART takes no argument; the fd is an open block device.
    if unsafe { ioctl(f.as_raw_fd(), BLKRRPART) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn wait_for(p: &Path) -> R<()> {
    for _ in 0..50 {
        if p.exists() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(err(format!("{} did not appear", p.display())))
}

/// The boot0 hardware partition of an eMMC: /dev/mmcblk2 -> /dev/mmcblk2boot0.
fn boot0_of(disk: &Path) -> PathBuf {
    PathBuf::from(format!("{}boot0", disk.display()))
}

fn say(s: &str) {
    eprintln!("tfctl: emmc install: {s}");
}

fn mounted(dev_prefix: &str) -> bool {
    fs::read_to_string("/proc/mounts")
        .unwrap_or_default()
        .lines()
        .any(|l| {
            l.split_whitespace()
                .next()
                .is_some_and(|d| d.starts_with(dev_prefix))
        })
}

/// The text saved next to boot0's copy: the EXT_CSD bytes the install changes, and what held
/// the eMMC then (`thinframe` when an earlier install already cleared them).
pub fn orig_csd_text(csd: &[u8; 512], boot0_sha: Option<&str>, held: &str, now: i64) -> String {
    format!(
        "# EXT_CSD before tfctl emmc install. Back to the vendor boot: write these bytes back\n\
         # (mmc-utils or the CMD6 tfctl uses) and emmc-boot0-orig.bin to mmcblk2boot0.\n\
         partition_config={:#04x}\nboot_bus_conditions={:#04x}\nboot0_first_64k_sha256={}\n\
         emmc_held={held}\nsaved_unix={now}\n",
        csd[EXT_CSD_PART_CONFIG],
        csd[EXT_CSD_BOOT_BUS],
        boot0_sha.unwrap_or("none (no boot0 device)"),
    )
}

/// Save boot0's head and the EXT_CSD text unless a copy is there already; each file is synced
/// before this returns. Err means the install must not go on.
fn save_originals(bin: &Path, txt: &Path, boot0: Option<&[u8]>, text: &str) -> R<()> {
    if let Some(dir) = txt.parent() {
        fs::create_dir_all(dir)?;
    }
    if let Some(b) = boot0 {
        if bin.exists() {
            say(&format!("{} kept from an earlier install", bin.display()));
        } else {
            crate::update::write_synced(bin, b)?;
            say(&format!(
                "boot0's first {} KiB saved to {}",
                b.len() >> 10,
                bin.display()
            ));
        }
    }
    if txt.exists() {
        say(&format!("{} kept from an earlier install", txt.display()));
    } else {
        crate::update::write_synced(txt, text.as_bytes())?;
        say(&format!("EXT_CSD boot bytes saved to {}", txt.display()));
    }
    Ok(())
}

fn install(backup_sha: Option<&str>, files: &Path) -> R<()> {
    // 0. Hold the agent off (updates, rollback, reboots) until this returns.
    let _lock = lock_with(Path::new(INSTALL_LOCK), std::process::id(), proc_comm)?;
    // 1. Where we are and what we would write.
    if slot::running_from_emmc() {
        return Err(err(
            "this system runs from the eMMC; start from the card to install",
        ));
    }
    let card = slot::find_disk(&disks()).ok_or_else(|| err("no thinframe card found"))?;
    let emmc = emmc_dev()?;
    if card == emmc {
        return Err(err("the card and the eMMC are the same device"));
    }
    let size = size_bytes(&emmc)?;
    if size < MIN_BYTES {
        return Err(err(format!("eMMC has {size} bytes; need {MIN_BYTES}")));
    }
    let (emmc_head, emmc_egon) = read_head(&emmc)?;
    match holds(&emmc_head, &emmc_egon) {
        Holds::Thinframe => say("the eMMC holds thinframe already; reinstalling"),
        _ => {
            let (bytes, sha) = fs::read_to_string(RECORD)
                .ok()
                .and_then(|t| parse_record(&t))
                .ok_or_else(|| {
                    err(format!(
                        "no backup record ({RECORD}); run `tfctl emmc backup` first"
                    ))
                })?;
            if bytes != size {
                return Err(err(format!(
                    "backup record is for {bytes} bytes; the eMMC has {size}"
                )));
            }
            match backup_sha {
                Some(s) if s == sha => {}
                Some(_) => return Err(err("--backup-sha256 does not match the backup record")),
                None => return Err(err(
                    "give --backup-sha256 HEX, the hash of the backup you verified off the frame",
                )),
            }
        }
    }
    let uenv = UEnv::load_effective(Path::new("/boot/uEnv.txt"))?;
    let running = slot::from_cmdline(&fs::read_to_string("/proc/cmdline")?)
        .ok_or_else(|| err("running slot unknown"))?;
    if uenv.tries != 0 || uenv.slot != Some(running) {
        return Err(err(
            "the running slot is not confirmed (uEnv tries > 0); try after it is",
        ));
    }
    let uboot = fs::read(files.join("u-boot-emmc.bin"))?;
    if !is_egon(&uboot) {
        return Err(err("u-boot-emmc.bin has no eGON header"));
    }
    let boot_scr = fs::read(files.join("boot.scr"))?;
    if !fs::read_dir(files)?
        .any(|e| e.is_ok_and(|e| e.path().extension().is_some_and(|x| x == "dtb")))
    {
        return Err(err(format!("no DTBs in {}", files.display())));
    }
    let mut card_mbr = [0u8; 512];
    File::open(&card)?.read_exact_at(&mut card_mbr, 0)?;
    let mbr = emmc_mbr(&card_mbr, size / 512)?;
    let (p1, _) = entry(&mbr, 0);
    if UBOOT_OFFSET + uboot.len() as u64 > p1 * 512 {
        return Err(err("U-Boot does not fit before partition 1"));
    }
    let edev = emmc.to_string_lossy().to_string();
    if mounted(&edev) {
        return Err(err(format!("something on {edev} is mounted")));
    }
    // What no backup holds, before anything changes it (read-only: EXT_CSD by CMD8, boot0).
    let csd = read_ext_csd(&File::open(&emmc)?)?;
    let b0 = boot0_of(&emmc);
    let boot0_head = if b0.exists() {
        let mut v = vec![0u8; BOOT0_CLEARED];
        File::open(&b0)?.read_exact_at(&mut v, 0)?;
        Some(v)
    } else {
        None
    };
    let sha = boot0_head.as_deref().map(|b| {
        let mut h = Sha256::new();
        h.update(b);
        h.finish_hex()
    });
    let held = match holds(&emmc_head, &emmc_egon) {
        Holds::Thinframe => "thinframe",
        _ => "vendor",
    };
    let text = orig_csd_text(&csd, sha.as_deref(), held, crate::clock::now());
    save_originals(
        Path::new(ORIG_BOOT0),
        Path::new(ORIG_CSD),
        boot0_head.as_deref(),
        &text,
    )
    .map_err(|e| {
        err(format!(
            "cannot save boot0 and EXT_CSD to /data ({e}); not installing"
        ))
    })?;

    // 2. Open the eMMC for writing, exclusively (fails if any of it is in use).
    let force_ro = format!("/sys/block/{}/force_ro", sys_name(&emmc));
    fs::write(&force_ro, "0\n")?;
    let w = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(O_EXCL)
        .open(&emmc)?;

    // 3. No boot code on the eMMC from here on, then the new table. The boot ROM is told to
    // read the user area (no boot partition), and boot0's copy of the vendor code goes too.
    say("clearing the vendor boot code and partition table");
    w.write_all_at(&vec![0u8; CHUNK], UBOOT_OFFSET)?;
    w.sync_all()?;
    let csd = read_ext_csd(&w)?;
    if csd[EXT_CSD_PART_CONFIG] & 0x78 != 0 {
        say(&format!(
            "PARTITION_CONFIG {} -> 0",
            part_config_text(csd[EXT_CSD_PART_CONFIG])
        ));
        write_ext_csd(&w, EXT_CSD_PART_CONFIG, 0)?;
        let now = read_ext_csd(&w)?[EXT_CSD_PART_CONFIG];
        if now & 0x78 != 0 {
            return Err(err(format!("PARTITION_CONFIG still {now:#04x}")));
        }
    }
    if b0.exists() {
        let ro = format!("/sys/block/{}/force_ro", sys_name(&b0));
        fs::write(&ro, "0\n")?;
        let r = OpenOptions::new().write(true).open(&b0).and_then(|f| {
            f.write_all_at(&vec![0u8; BOOT0_CLEARED], 0)
                .and_then(|_| f.sync_all())
        });
        let _ = fs::write(&ro, "1\n");
        r?;
    }
    w.write_all_at(&vec![0u8; (p1 * 512) as usize], 0)?;
    w.write_all_at(&mbr, 0)?;
    w.sync_all()?;

    // 4. The boot partition from the card as it is (mounted read-only), and the running root
    // into both slots: the card's other slot may be older than the eMMC support, and a
    // fallback to it would come up without /data.
    let mut want = Vec::new();
    for n in 1..=3u8 {
        let (start, len) = entry(&mbr, n as usize - 1);
        let from = if n == 1 { 1 } else { running.part() };
        let src = File::open(slot::part_n(&card, from))?;
        say(&format!(
            "copying card partition {from} to eMMC partition {n} ({} MiB)",
            len / 2048
        ));
        want.push((
            start * 512,
            len * 512,
            copy_into(&src, &w, start * 512, len * 512)?,
        ));
    }
    w.sync_all()?;
    drop_caches();
    for (n, (off, len, sha)) in want.iter().enumerate() {
        if hash_range(&w, *off, *len)? != *sha {
            return Err(err(format!("partition {} reads back different", n + 1)));
        }
    }
    say("partitions 1 to 3 read back equal");

    // 5. The kernel's view of the new table, then the boot partition's files for the eMMC. Our
    // exclusive hold on the whole disk would make the partition mounts fail (EBUSY): let go.
    reread_partitions(&w)?;
    drop(w);
    let (ep1, ep4) = (slot::part_n(&emmc, 1), slot::part_n(&emmc, 4));
    wait_for(&ep1)?;
    wait_for(&ep4)?;
    let mnt = Path::new("/run/thinframe/emmc");
    fs::create_dir_all(mnt)?;
    crate::update::sh(
        "mount",
        &["-t", "vfat", &ep1.to_string_lossy(), &mnt.to_string_lossy()],
    )?;
    let r = fix_boot(mnt, files, &boot_scr, &uenv, running);
    let u = crate::update::sh("umount", &[&mnt.to_string_lossy()]);
    r.and(u)?;

    // 6. A fresh /data with a copy of this one.
    say("making /data on the eMMC and copying this one");
    let p4 = ep4.to_string_lossy().to_string();
    crate::update::sh("mkfs.ext4", &["-F", "-q", "-L", "data", &p4])?;
    crate::update::sh("mount", &["-t", "ext4", &p4, &mnt.to_string_lossy()])?;
    let r = crate::update::sh("cp", &["-a", "/data/.", &mnt.to_string_lossy()]);
    let u = crate::update::sh("umount", &[&mnt.to_string_lossy()]);
    r.and(u)?;

    // 7. A read-only check of what was written; any doubt and the eMMC gets no boot code.
    check_filesystems(&emmc)?;

    // 8. Last: U-Boot. Only now can the eMMC start.
    say("writing U-Boot");
    let w = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(O_EXCL)
        .open(&emmc)?;
    w.write_all_at(&uboot, UBOOT_OFFSET)?;
    w.sync_all()?;
    drop_caches();
    let mut back = vec![0u8; uboot.len()];
    w.read_exact_at(&mut back, UBOOT_OFFSET)?;
    if back != uboot {
        w.write_all_at(&vec![0u8; 512], UBOOT_OFFSET)?;
        w.sync_all()?;
        return Err(err("U-Boot reads back different; cleared it again"));
    }
    drop(w);

    // 9. The same U-Boot at the start of boot0, and the boot ROM told to start from there.
    if b0.exists() {
        boot_from_boot0(&emmc, &b0, &uboot)?;
    }
    let _ = fs::write(&force_ro, "1\n");
    println!(
        "thinframe is on the eMMC (slot {}). Power off, remove the card and power on.\n\
         Keep the card: with it inserted the frame starts from the card again.",
        running.name()
    );
    Ok(())
}

/// U-Boot at offset 0 of boot0 (read back), then BOOT_BUS_CONDITIONS and PARTITION_CONFIG as
/// the vendor had them. If boot0 cannot be written, PARTITION_CONFIG stays 0 (the user area copy
/// is still there) and the install reports it.
fn boot_from_boot0(emmc: &Path, b0: &Path, uboot: &[u8]) -> R<()> {
    say(&format!("writing U-Boot to {} as well", b0.display()));
    let ro = format!("/sys/block/{}/force_ro", sys_name(b0));
    fs::write(&ro, "0\n")?;
    let r = (|| -> R<()> {
        let f = OpenOptions::new().read(true).write(true).open(b0)?;
        f.write_all_at(uboot, 0)?;
        f.sync_all()?;
        drop_caches();
        let mut back = vec![0u8; uboot.len()];
        f.read_exact_at(&mut back, 0)?;
        if back != uboot {
            f.write_all_at(&[0u8; 512], 0)?;
            f.sync_all()?;
            return Err(err(
                "U-Boot in boot0 reads back different; cleared it again",
            ));
        }
        Ok(())
    })();
    let _ = fs::write(&ro, "1\n");
    r?;
    let f = File::open(emmc)?;
    if read_ext_csd(&f)?[EXT_CSD_BOOT_BUS] != BOOT_BUS_4BIT {
        write_ext_csd(&f, EXT_CSD_BOOT_BUS, BOOT_BUS_4BIT)?;
    }
    write_ext_csd(&f, EXT_CSD_PART_CONFIG, PART_CONFIG_BOOT0)?;
    let csd = read_ext_csd(&f)?;
    if csd[EXT_CSD_PART_CONFIG] != PART_CONFIG_BOOT0 || csd[EXT_CSD_BOOT_BUS] != BOOT_BUS_4BIT {
        let _ = write_ext_csd(&f, EXT_CSD_PART_CONFIG, 0);
        return Err(err(format!(
            "EXT_CSD reads PARTITION_CONFIG {:#04x}, BOOT_BUS_CONDITIONS {:#04x}; set back to 0",
            csd[EXT_CSD_PART_CONFIG], csd[EXT_CSD_BOOT_BUS]
        )));
    }
    say(&format!(
        "PARTITION_CONFIG {}, BOOT_BUS_CONDITIONS {:#04x}",
        part_config_text(PART_CONFIG_BOOT0),
        BOOT_BUS_4BIT
    ));
    Ok(())
}

/// e2fsck -f -n on partitions 2 to 4 (exit 0 only), fsck.vfat -n on partition 1 when the image
/// has it (it ships e2fsprogs; dosfstools is a host tool only, and p1 was compared by hash).
fn check_filesystems(emmc: &Path) -> R<()> {
    say("checking the file systems on the eMMC (read-only)");
    let run = |tool: &str, args: &[&str], dev: &Path| -> R<Option<()>> {
        let d = dev.to_string_lossy().to_string();
        let out = match Command::new(tool).args(args).arg(&d).output() {
            Ok(o) => o,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        if out.status.success() {
            return Ok(Some(()));
        }
        let text = String::from_utf8_lossy(&out.stdout).to_string()
            + &String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = text.lines().rev().take(4).collect();
        Err(err(format!(
            "{tool} {} {d}: {}: {}; U-Boot not written, the eMMC does not start",
            args.join(" "),
            out.status,
            tail.into_iter().rev().collect::<Vec<_>>().join(" / ")
        )))
    };
    for n in 2..=4u8 {
        if run("e2fsck", &["-f", "-n"], &slot::part_n(emmc, n))?.is_none() {
            return Err(err("no e2fsck in this image; U-Boot not written"));
        }
    }
    if run("fsck.vfat", &["-n"], &slot::part_n(emmc, 1))?.is_none() {
        say("no fsck.vfat in this image; partition 1 was checked by hash");
    }
    say("file systems clean");
    Ok(())
}

/// The eMMC's boot partition after the raw copy: roots on the eMMC signature, the boot script
/// that reads `mmc_bootdev`, DTBs with the eMMC on, the running kernel for both slots, and the
/// running slot confirmed.
fn fix_boot(mnt: &Path, files: &Path, boot_scr: &[u8], uenv: &UEnv, running: Slot) -> R<()> {
    let ext = mnt.join("extlinux");
    for e in fs::read_dir(&ext)? {
        let p = e?.path();
        if p.extension().is_some_and(|x| x == "conf") {
            let t = fs::read_to_string(&p)?;
            fs::write(&p, retarget_extlinux(&t))?;
        }
    }
    fs::write(mnt.join("boot.scr"), boot_scr)?;
    // Both slots carry the running system (install step 4), so both get its kernel.
    let (k, other) = (
        format!("zImage.{}", running.name()),
        format!("zImage.{}", running.other().name()),
    );
    fs::copy(mnt.join(&k), mnt.join(&other))?;
    for e in fs::read_dir(files)? {
        let p = e?.path();
        if p.extension().is_some_and(|x| x == "dtb") {
            fs::copy(&p, mnt.join(p.file_name().unwrap()))?;
        }
    }
    fs::write(mnt.join("uEnv.txt"), uenv.render(running, 0))?;
    for stale in ["uEnv.bak", "uEnv.tmp", "pf/uboot.env", "pf/linux-ok.env"] {
        let _ = fs::remove_file(mnt.join(stale));
    }
    // boot.scr counts its starts in pf/uboot.env (fatwrite makes no directories): with the
    // directory there, a start from the eMMC leaves a mark the card can read afterwards.
    fs::create_dir_all(mnt.join("pf"))?;
    let _ = Command::new("sync").status();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card_mbr() -> [u8; 512] {
        let mut m = [0u8; 512];
        m[440..444].copy_from_slice(&slot::DISK_SIG.to_le_bytes());
        // image/buildroot/board/pf1007l/genimage.cfg: 4M+256M, 1G, 1G, data.
        let parts: [(u8, u32, u32); 4] = [
            (0x0c, 8192, 524_288),
            (0x83, 532_480, 2_097_152),
            (0x83, 2_629_632, 2_097_152),
            (0x83, 4_726_784, 57_607_168),
        ];
        for (i, (t, s, l)) in parts.iter().enumerate() {
            let o = 446 + 16 * i;
            m[o + 4] = *t;
            m[o + 8..o + 12].copy_from_slice(&s.to_le_bytes());
            m[o + 12..o + 16].copy_from_slice(&l.to_le_bytes());
        }
        m[446] = 0x80;
        m[510] = 0x55;
        m[511] = 0xaa;
        m
    }

    #[test]
    fn emmc_table_keeps_the_card_layout_and_fills_the_emmc() {
        let card = card_mbr();
        // This frame's eMMC: 30,777,344 sectors [V 2026-10-26, /sys/block/mmcblk2/size].
        let m = emmc_mbr(&card, 30_777_344).unwrap();
        assert_eq!(slot::mbr_sig(&m), Some(slot::EMMC_SIG));
        assert_eq!(&m[446..494], &card[446..494], "p1 to p3 as on the card");
        assert_eq!(entry(&m, 3), (4_726_784, 30_777_344 - 4_726_784));
        assert_eq!(m[446 + 48 + 4], 0x83);
        assert_eq!(&m[510..], &[0x55, 0xaa]);
        assert!(
            emmc_mbr(&card, 4_726_784 + 1000).is_err(),
            "no room for data"
        );
        let mut other = card;
        other[440] ^= 1;
        assert!(
            emmc_mbr(&other, 30_777_344).is_err(),
            "not a thinframe card"
        );
        let mut early = card;
        early[446 + 8..446 + 12].copy_from_slice(&100u32.to_le_bytes());
        assert!(emmc_mbr(&early, 30_777_344).is_err(), "p1 over U-Boot");
    }

    #[test]
    fn the_boot0_setting_is_the_vendors() {
        assert_eq!(
            part_config_text(PART_CONFIG_BOOT0),
            "0x48 (boot from boot0, BOOT_ACK on)"
        );
        assert_eq!(BOOT_BUS_4BIT & 0x1b, 0x01);
    }

    #[test]
    fn what_a_disk_holds() {
        let card = card_mbr();
        let emmc = emmc_mbr(&card, 30_777_344).unwrap();
        let mut egon = [0u8; 16];
        egon[4..12].copy_from_slice(b"eGON.BT0");
        assert_eq!(holds(&emmc, &egon), Holds::Thinframe);
        assert_eq!(holds(&card, &egon), Holds::Card);
        // This frame's Android MBR has signature 0 and boot0 at 8 KiB (backup emmc-00.bin).
        assert_eq!(
            holds(&[0u8; 512], &egon),
            Holds::Other { sig: 0, egon: true }
        );
        assert_eq!(
            holds(&[0u8; 512], &[0u8; 16]),
            Holds::Other {
                sig: 0,
                egon: false
            }
        );
    }

    #[test]
    fn extlinux_roots_move_to_the_emmc() {
        let c = "LABEL thinframe\n\tAPPEND root=PARTUUID=50463037-03 ro\nLABEL other-slot\n\tAPPEND root=PARTUUID=50463037-02 ro\n";
        let e = retarget_extlinux(c);
        assert!(e.contains("root=PARTUUID=50463045-03 ro"));
        assert!(e.contains("root=PARTUUID=50463045-02 ro"));
        assert!(!e.contains("50463037"));
        assert!(e.ends_with('\n'));
        // The card's conf as post-image.sh renders it: min.dtb has no eMMC, so pmic.dtb.
        let card = include_str!("../../image/boot/extlinux.conf.in")
            .replace("@SLOT@", "b")
            .replace("@PART@", "3")
            .replace("@OTHER@", "a")
            .replace("@OTHERPART@", "2")
            .replace("@CMDLINE@", "quiet");
        let e = retarget_extlinux(&card);
        assert!(!e.contains("-min.dtb") && !e.contains("LABEL min"), "{e}");
        assert!(e.contains("LABEL pmic\n\tKERNEL /zImage.b\n\tFDT /sun8i-a33-pf1007l-pmic.dtb\n"));
        assert!(
            e.contains("\tFDT /sun8i-a33-pf1007l-full.dtb\n"),
            "full untouched"
        );
        assert!(e.contains("root=PARTUUID=50463045-03 rootfstype=ext4 rootwait ro console=ttyGS0"));
        assert_eq!(e.lines().count(), card.lines().count());
        assert_eq!(
            slot::from_cmdline("root=PARTUUID=50463045-03 ro"),
            Some(Slot::B)
        );
    }

    #[test]
    fn install_lock_refuses_a_live_holder_and_replaces_a_stale_one() {
        let dir = std::env::temp_dir().join(format!("tfctl-emmc-lock-{}", std::process::id()));
        let path = dir.join("run/thinframe/emmc-install");
        let alive = |p: u32| (p == 4242).then(|| "tfctl\n".to_string());
        let l = lock_with(&path, 4242, alive).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "4242\n");
        let e = lock_with(&path, 5000, alive).err().unwrap().to_string();
        assert!(e.contains("pid 4242"), "{e}");
        drop(l);
        assert!(!path.exists(), "released on drop");
        // Left by a crashed install: the pid is gone, or now runs something else.
        fs::write(&path, "4243\n").unwrap();
        let l = lock_with(&path, 5000, |p| (p == 4243).then(|| "sh\n".into())).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "5000\n");
        drop(l);
        assert_eq!(live_holder("7\n", |_| Some("tfctl".into())), Some(7));
        assert_eq!(live_holder("7\n", |_| None), None, "gone");
        assert_eq!(live_holder("x", |_| Some("tfctl".into())), None);
        assert_eq!(live_holder("0", |_| Some("tfctl".into())), None);
        assert_eq!(holder(&path), None, "no file");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn originals_are_saved_once_and_never_overwritten() {
        let dir = std::env::temp_dir().join(format!("tfctl-emmc-orig-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let (bin, txt) = (dir.join("thinframe/b0.bin"), dir.join("thinframe/csd.txt"));
        let mut csd = [0u8; 512];
        csd[EXT_CSD_PART_CONFIG] = 0x48;
        csd[EXT_CSD_BOOT_BUS] = 0x01;
        let t = orig_csd_text(&csd, Some("ab"), "vendor", 1_793_000_000);
        assert!(
            t.contains("partition_config=0x48\nboot_bus_conditions=0x01\n"),
            "{t}"
        );
        assert!(
            t.contains("boot0_first_64k_sha256=ab\nemmc_held=vendor\n"),
            "{t}"
        );
        let vendor = vec![7u8; BOOT0_CLEARED];
        save_originals(&bin, &txt, Some(&vendor), &t).unwrap();
        assert_eq!(fs::read(&bin).unwrap(), vendor);
        assert_eq!(fs::read_to_string(&txt).unwrap(), t);
        // A later install (boot0 already zeroed, PARTITION_CONFIG 0) keeps the first copy.
        csd[EXT_CSD_PART_CONFIG] = 0;
        let t2 = orig_csd_text(&csd, None, "thinframe", 1_793_000_100);
        save_originals(&bin, &txt, Some(&[0u8; BOOT0_CLEARED]), &t2).unwrap();
        assert_eq!(fs::read(&bin).unwrap(), vendor);
        assert_eq!(fs::read_to_string(&txt).unwrap(), t);
        // Nowhere to write (/data read-only or gone): the install must stop.
        let blocked = dir.join("file");
        fs::write(&blocked, "x").unwrap();
        assert!(save_originals(&blocked.join("b"), &blocked.join("t"), Some(&vendor), &t).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn backup_record() {
        let sha = "ab".repeat(32);
        assert_eq!(
            parse_record(&format!(
                "bytes=15758000128\nsha256={}\n",
                sha.to_uppercase()
            )),
            Some((15_758_000_128, sha.clone()))
        );
        assert_eq!(parse_record("bytes=1\nsha256=abc\n"), None, "short hash");
        assert_eq!(parse_record(&format!("sha256={sha}\n")), None, "no size");
    }
}
