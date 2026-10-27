//! A/B root slots (TASK 12, docs/decisions.md "Full OS updates"). Pure helpers: which slot is
//! running, the `uEnv.txt` slot/tries lines U-Boot reads, and the guard that keeps `dd` away
//! from any disk the frame did not boot from. A system on the card (MBR signature `50463037`)
//! acts on the card and never on the eMMC; a system on the eMMC (`tfctl emmc install`,
//! signature `50463045`) acts on the eMMC and never on a card.
use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

/// MBR disk signature written by `image/build.sh assemble`; also the PARTUUID prefix.
pub const DISK_SIG: u32 = 0x5046_3037;
/// The same layout installed on the eMMC by `tfctl emmc install` ("PF0E"). A different
/// signature, so a rescue card and the eMMC never share a PARTUUID.
pub const EMMC_SIG: u32 = 0x5046_3045;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    A,
    B,
}

impl Slot {
    pub fn other(self) -> Slot {
        match self {
            Slot::A => Slot::B,
            Slot::B => Slot::A,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Slot::A => "a",
            Slot::B => "b",
        }
    }
    /// Partition number on the card: p2 = a, p3 = b.
    pub fn part(self) -> u8 {
        match self {
            Slot::A => 2,
            Slot::B => 3,
        }
    }
    pub fn parse(s: &str) -> Option<Slot> {
        match s.trim() {
            "a" | "A" => Some(Slot::A),
            "b" | "B" => Some(Slot::B),
            _ => None,
        }
    }
}

/// Running slot from the kernel command line: `tf.slot=`, else `root=PARTUUID=50463037-0N`,
/// else `root=/dev/mmcblkXpN`.
pub fn from_cmdline(cmdline: &str) -> Option<Slot> {
    let words: Vec<&str> = cmdline.split_whitespace().collect();
    if let Some(s) = words.iter().find_map(|w| w.strip_prefix("tf.slot=")) {
        return Slot::parse(s);
    }
    let root = words.iter().find_map(|w| w.strip_prefix("root="))?;
    let part: u8 = if let Some(uuid) = root.strip_prefix("PARTUUID=") {
        uuid.rsplit_once('-')?.1.parse().ok()?
    } else {
        root.rsplit_once('p')?.1.parse().ok()?
    };
    match part {
        2 => Some(Slot::A),
        3 => Some(Slot::B),
        _ => None,
    }
}

/// `uEnv.txt`: plain `name=value` lines plus comments. Only `slot` and `tries` matter to us;
/// everything else is preserved byte for byte.
#[derive(Debug, Clone, PartialEq)]
pub struct UEnv {
    pub slot: Option<Slot>,
    pub tries: u32,
    text: String,
}

impl UEnv {
    pub fn parse(text: &str) -> UEnv {
        // U-Boot's `env export -t` ends the file with a NUL byte.
        let text = text.trim_end_matches('\0');
        let get = |key: &str| {
            text.lines()
                .map(str::trim)
                .filter_map(|l| l.split_once('='))
                .find(|(k, _)| k.trim() == key)
                .map(|(_, v)| v.trim().to_string())
        };
        UEnv {
            slot: get("slot").and_then(|s| Slot::parse(&s)),
            tries: get("tries").and_then(|t| t.parse().ok()).unwrap_or(0),
            text: text.to_string(),
        }
    }

    pub fn load(path: &Path) -> io::Result<UEnv> {
        Ok(UEnv::parse(&fs::read_to_string(path)?))
    }

    /// What U-Boot acts on (boot.cmd.patch): `uEnv.txt` when it names a slot, else `uEnv.bak`
    /// (the copy `save` keeps), else `uEnv.txt` as it is (U-Boot then defaults to slot a,
    /// tries 0). Err only when neither file can be read.
    pub fn load_effective(path: &Path) -> io::Result<UEnv> {
        let main = UEnv::load(path);
        if matches!(&main, Ok(u) if u.slot.is_some()) {
            return main;
        }
        match UEnv::load(&path.with_extension("bak")) {
            Ok(b) if b.slot.is_some() => Ok(b),
            _ => main,
        }
    }

    /// Rendered with `slot` and `tries` set; missing keys are appended.
    pub fn render(&self, slot: Slot, tries: u32) -> String {
        let mut out = String::with_capacity(self.text.len() + 32);
        let (mut saw_slot, mut saw_tries) = (false, false);
        for line in self.text.lines() {
            let key = line.split_once('=').map(|(k, _)| k.trim());
            match key {
                Some("slot") if !saw_slot => {
                    saw_slot = true;
                    out.push_str(&format!("slot={}\n", slot.name()));
                }
                Some("tries") if !saw_tries => {
                    saw_tries = true;
                    out.push_str(&format!("tries={tries}\n"));
                }
                Some("slot") | Some("tries") => {}
                _ => {
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
        if !saw_slot {
            out.push_str(&format!("slot={}\n", slot.name()));
        }
        if !saw_tries {
            out.push_str(&format!("tries={tries}\n"));
        }
        out
    }

    /// Write a temp file and fsync it, copy the current file to `uEnv.bak` (fsynced too), then
    /// rename the temp file over it; the caller syncs the file system afterwards. FAT has no
    /// journal, so a power cut mid-rename (or mid-`fatwrite` in U-Boot) can lose `uEnv.txt`;
    /// boot.cmd.patch then reads `uEnv.bak`, the last state that was in force. A current file
    /// that names no slot (torn, garbage) never replaces a `.bak` that does.
    pub fn save(&self, path: &Path, slot: Slot, tries: u32) -> io::Result<()> {
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, self.render(slot, tries))?;
        fs::File::open(&tmp)?.sync_all()?;
        if UEnv::load(path).is_ok_and(|u| u.slot.is_some()) {
            let bak = path.with_extension("bak");
            fs::copy(path, &bak)?;
            fs::File::open(&bak)?.sync_all()?;
        }
        fs::rename(&tmp, path)
    }
}

/// The disk signature of a 512-byte MBR (bytes 440..444, little endian).
pub fn mbr_sig(mbr: &[u8]) -> Option<u32> {
    (mbr.len() >= 444).then(|| u32::from_le_bytes([mbr[440], mbr[441], mbr[442], mbr[443]]))
}

/// True when the MBR is one of ours: the card's or the installed eMMC's.
pub fn mbr_is_ours(mbr: &[u8]) -> bool {
    matches!(mbr_sig(mbr), Some(DISK_SIG | EMMC_SIG))
}

/// Which of our disks the running root is on: the signature in `root=PARTUUID=<sig>-0N`.
/// Anything else (`root=/dev/vda2` under QEMU, a cmdline we cannot read) means the card.
pub fn root_sig(cmdline: &str) -> u32 {
    cmdline
        .split_whitespace()
        .find_map(|w| w.strip_prefix("root=PARTUUID="))
        .and_then(|u| u.split_once('-'))
        .and_then(|(sig, _)| u32::from_str_radix(sig, 16).ok())
        .filter(|s| *s == EMMC_SIG)
        .unwrap_or(DISK_SIG)
}

/// True when this system runs from the eMMC.
pub fn running_from_emmc() -> bool {
    root_sig(&fs::read_to_string("/proc/cmdline").unwrap_or_default()) == EMMC_SIG
}

/// The whole-disk device our root lives on. Booted from the card: the first candidate with the
/// card signature that sysfs does not report as eMMC (`/sys/block/<name>/device/type` = `MMC`;
/// SD cards say `SD`), so the eMMC is never returned, whatever it holds, and `dd` cannot reach
/// it. Booted from the eMMC: the eMMC with the eMMC signature, never a card.
pub fn find_disk(candidates: &[&Path]) -> Option<std::path::PathBuf> {
    find_disk_for(candidates, Path::new("/proc/cmdline"))
}

/// `find_disk` with the kernel command line read from `cmdline`.
pub fn find_disk_for(candidates: &[&Path], cmdline: &Path) -> Option<std::path::PathBuf> {
    let want = root_sig(&fs::read_to_string(cmdline).unwrap_or_default());
    find_disk_in(candidates, Path::new("/sys/block"), want)
}

/// `find_disk` with the sysfs block directory and the wanted signature as parameters (tests).
pub fn find_disk_in(
    candidates: &[&Path],
    sys_block: &Path,
    want: u32,
) -> Option<std::path::PathBuf> {
    candidates.iter().find_map(|p| {
        if is_emmc(sys_block, p) != (want == EMMC_SIG) {
            return None;
        }
        let mut f = fs::File::open(p).ok()?;
        let mut mbr = [0u8; 512];
        f.seek(SeekFrom::Start(0)).ok()?;
        f.read_exact(&mut mbr).ok()?;
        (mbr_sig(&mbr) == Some(want)).then(|| p.to_path_buf())
    })
}

/// The eMMC among `candidates`, by sysfs, whatever it holds.
pub fn find_emmc(candidates: &[&Path], sys_block: &Path) -> Option<std::path::PathBuf> {
    candidates
        .iter()
        .find(|p| is_emmc(sys_block, p))
        .map(|p| p.to_path_buf())
}

/// True when sysfs says the device `disk` names is an eMMC (`device/type` reads `MMC`).
pub fn is_emmc(sys_block: &Path, disk: &Path) -> bool {
    let Some(name) = disk.file_name() else {
        return false;
    };
    fs::read_to_string(sys_block.join(name).join("device/type")).is_ok_and(|t| t.trim() == "MMC")
}

/// Where the card can be: the frame's SD slot (mmcblk*), or virtio-blk under `qemu -M virt` (M2).
/// `find_disk` still takes only the one with the running root's signature. With the DTS
/// aliases (mmc0 = SD, mmc1 = SDIO Wi-Fi, mmc2 = eMMC) the card is mmcblk0 and the eMMC is
/// mmcblk2, whichever of them booted.
pub const DISKS: [&str; 4] = ["/dev/mmcblk0", "/dev/mmcblk1", "/dev/mmcblk2", "/dev/vda"];

/// `/dev/mmcblk0` + slot -> `/dev/mmcblk0p2`.
pub fn part_dev(disk: &Path, slot: Slot) -> std::path::PathBuf {
    part_n(disk, slot.part())
}

/// Partition `n` of a whole disk: `/dev/mmcblk0` + 1 -> `/dev/mmcblk0p1`, `/dev/vda` + 1 -> `/dev/vda1`.
pub fn part_n(disk: &Path, n: u8) -> std::path::PathBuf {
    let s = disk.to_string_lossy();
    let sep = if s.ends_with(|c: char| c.is_ascii_digit()) {
        "p"
    } else {
        ""
    };
    std::path::PathBuf::from(format!("{s}{sep}{n}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_from_cmdline_forms() {
        assert_eq!(
            from_cmdline("root=PARTUUID=50463037-02 rootfstype=ext4 ro quiet"),
            Some(Slot::A)
        );
        assert_eq!(from_cmdline("root=PARTUUID=50463037-03 ro"), Some(Slot::B));
        assert_eq!(from_cmdline("root=/dev/mmcblk0p3"), Some(Slot::B));
        assert_eq!(from_cmdline("root=/dev/mmcblk1p2 tf.slot=b"), Some(Slot::B));
        assert_eq!(from_cmdline("root=/dev/mmcblk0p4"), None);
        assert_eq!(from_cmdline("console=tty1"), None);
        assert_eq!(Slot::A.other(), Slot::B);
        assert_eq!(Slot::B.part(), 3);
    }

    #[test]
    fn root_signature_names_the_boot_disk() {
        assert_eq!(root_sig("root=PARTUUID=50463037-03 ro"), DISK_SIG);
        assert_eq!(root_sig("ro root=PARTUUID=50463045-02 quiet"), EMMC_SIG);
        assert_eq!(root_sig("root=/dev/vda2"), DISK_SIG);
        assert_eq!(root_sig("root=PARTUUID=deadbeef-02"), DISK_SIG);
        assert_eq!(root_sig(""), DISK_SIG);
        assert_eq!(from_cmdline("root=PARTUUID=50463045-03 ro"), Some(Slot::B));
    }

    #[test]
    fn uenv_round_trip_preserves_other_lines() {
        let src = "# comment\nslot=a\ntries=0\n#pf_mode=auto\nslot=b\n";
        let u = UEnv::parse(src);
        assert_eq!((u.slot, u.tries), (Some(Slot::A), 0));
        assert_eq!(
            u.render(Slot::B, 3),
            "# comment\nslot=b\ntries=3\n#pf_mode=auto\n"
        );
        let u = UEnv::parse("pf_mode=ums\n");
        assert_eq!(u.slot, None);
        assert_eq!(u.render(Slot::A, 0), "pf_mode=ums\nslot=a\ntries=0\n");
        assert_eq!(UEnv::parse("slot = b\ntries = 2\n").tries, 2);
    }

    #[test]
    fn uenv_save_and_load() {
        let dir = std::env::temp_dir().join(format!("tfctl-uenv-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("uEnv.txt");
        fs::write(&p, "slot=a\ntries=0\n").unwrap();
        UEnv::load(&p).unwrap().save(&p, Slot::B, 3).unwrap();
        let u = UEnv::load(&p).unwrap();
        assert_eq!((u.slot, u.tries), (Some(Slot::B), 3));
        assert!(!dir.join("uEnv.tmp").exists());
        assert_eq!(
            fs::read_to_string(dir.join("uEnv.bak")).unwrap(),
            "slot=a\ntries=0\n",
            ".bak holds the previous file"
        );
        UEnv::load(&p).unwrap().save(&p, Slot::B, 0).unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("uEnv.bak")).unwrap(),
            "slot=b\ntries=3\n"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    /// Mirrors boot.cmd.patch: a uEnv.txt without a usable slot falls back to uEnv.bak, and
    /// such a file never overwrites a good .bak.
    #[test]
    fn uenv_backup_is_what_uboot_falls_back_to() {
        let dir = std::env::temp_dir().join(format!("tfctl-uenvbak-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let (p, bak) = (dir.join("uEnv.txt"), dir.join("uEnv.bak"));
        assert!(UEnv::load_effective(&p).is_err(), "neither file");
        fs::write(&bak, "slot=b\ntries=0\npf_mode=linux\n").unwrap();
        // Missing, empty, torn or garbage uEnv.txt: U-Boot and tfctl both use the .bak.
        for main in [
            None,
            Some(""),
            Some("slo"),
            Some("slot=c\ntries=2\n"),
            Some("\0\0\0"),
        ] {
            match main {
                None => {
                    let _ = fs::remove_file(&p);
                }
                Some(t) => fs::write(&p, t).unwrap(),
            }
            let u = UEnv::load_effective(&p).unwrap();
            assert_eq!((u.slot, u.tries), (Some(Slot::B), 0), "{main:?}");
        }
        // Saving over a garbage uEnv.txt keeps the good .bak.
        fs::write(&p, "garbage").unwrap();
        UEnv::load_effective(&p)
            .unwrap()
            .save(&p, Slot::A, 3)
            .unwrap();
        assert_eq!(
            fs::read_to_string(&bak).unwrap(),
            "slot=b\ntries=0\npf_mode=linux\n"
        );
        assert_eq!(
            fs::read_to_string(&p).unwrap(),
            "slot=a\ntries=3\npf_mode=linux\n"
        );
        // A good uEnv.txt wins over the .bak.
        let u = UEnv::load_effective(&p).unwrap();
        assert_eq!((u.slot, u.tries), (Some(Slot::A), 3));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn disk_guard() {
        let mut mbr = vec![0u8; 512];
        assert!(!mbr_is_ours(&mbr));
        mbr[440..444].copy_from_slice(&DISK_SIG.to_le_bytes());
        assert!(mbr_is_ours(&mbr));
        assert_eq!(&mbr[440..444], &[0x37, 0x30, 0x46, 0x50]);
        let dir = std::env::temp_dir().join(format!("tfctl-disk-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let (emmc, sd) = (dir.join("mmcblk0"), dir.join("mmcblk1"));
        fs::write(&emmc, vec![0u8; 512]).unwrap();
        fs::write(&sd, &mbr).unwrap();
        let none = dir.join("nosys");
        assert_eq!(
            find_disk_in(&[&emmc, &sd], &none, DISK_SIG),
            Some(sd.clone())
        );
        assert_eq!(
            find_disk_in(&[&emmc, &dir.join("missing")], &none, DISK_SIG),
            None
        );
        // An eMMC that somehow carries our signature is still never the card.
        let sys = dir.join("sys");
        fs::create_dir_all(sys.join("mmcblk0/device")).unwrap();
        fs::create_dir_all(sys.join("mmcblk1/device")).unwrap();
        fs::write(sys.join("mmcblk0/device/type"), "MMC\n").unwrap();
        fs::write(sys.join("mmcblk1/device/type"), "SD\n").unwrap();
        fs::write(&emmc, &mbr).unwrap();
        assert_eq!(
            find_disk_in(&[&emmc, &sd], &sys, DISK_SIG),
            Some(sd.clone())
        );
        assert_eq!(find_disk_in(&[&emmc], &sys, DISK_SIG), None);
        assert!(is_emmc(&sys, &emmc) && !is_emmc(&sys, &sd));
        assert_eq!(find_emmc(&[&sd, &emmc], &sys), Some(emmc.clone()));
        // Booted from the eMMC: only the eMMC with the eMMC signature, never the card.
        let mut emmc_mbr = mbr.clone();
        emmc_mbr[440..444].copy_from_slice(&EMMC_SIG.to_le_bytes());
        assert!(mbr_is_ours(&emmc_mbr));
        assert_eq!(
            find_disk_in(&[&sd, &emmc], &sys, EMMC_SIG),
            None,
            "eMMC has card sig"
        );
        fs::write(&emmc, &emmc_mbr).unwrap();
        fs::write(&sd, &emmc_mbr).unwrap();
        assert_eq!(
            find_disk_in(&[&sd, &emmc], &sys, EMMC_SIG),
            Some(emmc.clone())
        );
        assert_eq!(
            find_disk_in(&[&sd, &emmc], &sys, DISK_SIG),
            None,
            "never the eMMC"
        );
        assert!(
            !is_emmc(&sys, Path::new("/dev/vda")),
            "no sysfs entry: not eMMC"
        );
        assert_eq!(
            part_dev(Path::new("/dev/mmcblk0"), Slot::B)
                .to_str()
                .unwrap(),
            "/dev/mmcblk0p3"
        );
        assert_eq!(
            part_dev(Path::new("/dev/sda"), Slot::A).to_str().unwrap(),
            "/dev/sda2"
        );
        assert_eq!(
            part_n(Path::new("/dev/vda"), 1).to_str().unwrap(),
            "/dev/vda1"
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
