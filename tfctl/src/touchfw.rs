//! Touch firmware without shipping it: the Silead table `gsl_m86_hd` is proprietary, so release
//! images carry none. On boot, when the image has no firmware, `tfctl boot` reads it out of the
//! frame's own Android boot image on the eMMC (opened read-only; nothing here can write it),
//! checks the SHA-256 the project tested, keeps it on /data and re-probes the touch controller.
//! Method and layout: ormarch `docs/experiments.md` E025/E028/E030, `image/vendor/extract_gsl_fw.py`.
use crate::{png, sha256, slot};
use std::fs;
use std::io::{self, Error, ErrorKind};
use std::os::unix::fs::FileExt;
use std::path::Path;

type R<T> = Result<T, Error>;

/// What the silead driver asks for (`firmware-name` in the DTS, under `silead/`).
pub const FW_NAME: &str = "silead/gsl_m86_hd.fw";
/// Built into the image when the owner built it with `PF1007L_TOUCH_FIRMWARE`.
pub const IMAGE_FW: &str = "/lib/firmware/silead/gsl_m86_hd.fw";
/// Kept across OS updates; handed to the kernel via firmware_class.path.
pub const DATA_DIR: &str = "/data/thinframe/firmware";
const FW_PATH_PARAM: &str = "/sys/module/firmware_class/parameters/path";
/// The tested table: 4,620 records x 8 bytes (ormarch E028, image/vendor/README.md).
pub const SHA256: &str = "e11ffa8a33a56b78f8d2f57925fef1b67ea9a58c264bb5f3a8553ea52b14a9e6";
const TABLE: &str = "gsl_m86_hd";
const MODULE: &str = "gslX680new.ko";
/// Android `boot` then `recovery`, in sectors (sunxi partition table, ormarch E030).
const BOOT_IMAGES: [u64; 2] = [172_032, 2_400_256];
const MAX_RAMDISK: usize = 32 << 20;
const PAGE_REG: u32 = 0xf0;

fn bad(msg: &str) -> Error {
    Error::new(ErrorKind::InvalidData, format!("touch fw: {msg}"))
}

fn u32_at(d: &[u8], o: usize) -> R<u32> {
    let b = d.get(o..o + 4).ok_or_else(|| bad("truncated"))?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn u16_at(d: &[u8], o: usize) -> R<u16> {
    let b = d.get(o..o + 2).ok_or_else(|| bad("truncated"))?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}

/// Byte offset and length of the ramdisk in an Android boot image (header v0).
pub fn ramdisk_span(hdr: &[u8]) -> R<(u64, usize)> {
    if hdr.get(..8) != Some(b"ANDROID!") {
        return Err(bad("no Android boot image"));
    }
    let (kernel, ramdisk, page) = (u32_at(hdr, 8)?, u32_at(hdr, 16)?, u32_at(hdr, 36)?);
    if !(2048..=16384).contains(&page) || !page.is_power_of_two() {
        return Err(bad("odd page size"));
    }
    if ramdisk == 0 || ramdisk as usize > MAX_RAMDISK {
        return Err(bad("odd ramdisk size"));
    }
    let page = page as u64;
    Ok((
        page * (1 + (kernel as u64).div_ceil(page)),
        ramdisk as usize,
    ))
}

/// A gzip member's payload (RFC 1952), inflated with the PNG decoder's inflate.
pub fn gunzip(d: &[u8], limit: usize) -> R<Vec<u8>> {
    if d.len() < 18 || d[..3] != [0x1f, 0x8b, 8] {
        return Err(bad("not gzip"));
    }
    let flags = d[3];
    let mut p = 10;
    if flags & 4 != 0 {
        p += 2 + u16_at(d, p)? as usize;
    }
    for bit in [8, 16] {
        if flags & bit != 0 {
            p += 1 + d
                .get(p..)
                .and_then(|s| s.iter().position(|&c| c == 0))
                .ok_or_else(|| bad("gzip name"))?;
        }
    }
    if flags & 2 != 0 {
        p += 2;
    }
    png::inflate(d.get(p..).ok_or_else(|| bad("gzip header"))?, limit)
}

/// The data of the first file in a newc cpio archive whose path ends with `/name` (or is `name`).
pub fn cpio_find<'a>(a: &'a [u8], name: &str) -> Option<&'a [u8]> {
    let hex = |s: &[u8]| usize::from_str_radix(std::str::from_utf8(s).ok()?, 16).ok();
    let mut p = 0;
    while a.get(p..p + 6)? == b"070701" {
        let h = a.get(p..p + 110)?;
        let (size, nlen) = (hex(&h[54..62])?, hex(&h[94..102])?);
        let path = a.get(p + 110..p + 110 + nlen.checked_sub(1)?)?;
        let data = (p + 110 + nlen + 3) & !3;
        if path == b"TRAILER!!!" {
            return None;
        }
        let body = a.get(data..data + size)?;
        if path == name.as_bytes() || path.ends_with(format!("/{name}").as_bytes()) {
            return Some(body);
        }
        p = (data + size + 3) & !3;
    }
    None
}

/// Just enough of a little-endian ELF32 relocatable file to follow pointers in `.data`.
struct Elf<'a> {
    d: &'a [u8],
    /// (name, offset, size, link) per section.
    secs: Vec<(String, usize, usize, usize)>,
    /// (name, value, size, info, shndx) per symbol.
    syms: Vec<(String, u32, u32, u8, usize)>,
}

fn cstr(d: &[u8], o: usize) -> R<String> {
    let s = d.get(o..).ok_or_else(|| bad("string"))?;
    let n = s
        .iter()
        .position(|&c| c == 0)
        .ok_or_else(|| bad("string"))?;
    Ok(String::from_utf8_lossy(&s[..n]).into_owned())
}

impl<'a> Elf<'a> {
    fn parse(d: &'a [u8]) -> R<Elf<'a>> {
        if d.get(..6) != Some(b"\x7fELF\x01\x01") {
            return Err(bad("module is not ELF32 LE"));
        }
        let (shoff, shent) = (u32_at(d, 0x20)? as usize, u16_at(d, 0x2e)? as usize);
        let (shnum, shstr) = (u16_at(d, 0x30)? as usize, u16_at(d, 0x32)? as usize);
        if shent < 40 || shstr >= shnum {
            return Err(bad("section headers"));
        }
        let mut raw = Vec::with_capacity(shnum);
        for i in 0..shnum {
            let o = shoff + i * shent;
            raw.push((
                u32_at(d, o)?,
                u32_at(d, o + 16)?,
                u32_at(d, o + 20)?,
                u32_at(d, o + 24)?,
            ));
        }
        let names_at = raw[shstr].1 as usize;
        let mut secs = Vec::with_capacity(shnum);
        for &(n, off, size, link) in &raw {
            let name = cstr(d, names_at + n as usize)?;
            secs.push((name, off as usize, size as usize, link as usize));
        }
        let mut e = Elf {
            d,
            secs,
            syms: Vec::new(),
        };
        let (_, symoff, symsize, link) = e.sec(".symtab")?.clone();
        let stroff = e.secs.get(link).ok_or_else(|| bad("strtab"))?.1;
        for i in 0..symsize / 16 {
            let o = symoff + i * 16;
            let name = cstr(d, stroff + u32_at(d, o)? as usize)?;
            let info = *d.get(o + 12).ok_or_else(|| bad("symbol"))?;
            e.syms.push((
                name,
                u32_at(d, o + 4)?,
                u32_at(d, o + 8)?,
                info,
                u16_at(d, o + 14)? as usize,
            ));
        }
        Ok(e)
    }

    fn sec(&self, name: &str) -> R<&(String, usize, usize, usize)> {
        self.secs
            .iter()
            .find(|s| s.0 == name)
            .ok_or_else(|| bad(&format!("no {name}")))
    }

    /// Bytes `off..off+len` of section `idx`, bounded by the section.
    fn bytes(&self, idx: usize, off: usize, len: usize) -> R<&'a [u8]> {
        let s = self.secs.get(idx).ok_or_else(|| bad("section index"))?;
        if off.checked_add(len).is_none_or(|end| end > s.2) {
            return Err(bad("outside its section"));
        }
        self.d
            .get(s.1 + off..s.1 + off + len)
            .ok_or_else(|| bad("outside the file"))
    }

    /// The one object symbol called `name`: (section, value, size).
    fn object(&self, name: &str) -> R<(usize, u32, u32)> {
        let mut hits = self.syms.iter().filter(|s| s.0 == name && s.3 & 15 == 1);
        match (hits.next(), hits.next()) {
            (Some(s), None) => Ok((s.4, s.1, s.2)),
            _ => Err(bad(&format!("no single symbol {name}"))),
        }
    }

    /// A 32-bit pointer stored at section `sec` + `off`, resolved through its R_ARM_ABS32
    /// relocation: (target section, target offset).
    fn pointer(&self, sec: usize, off: u32) -> R<(usize, u32)> {
        let rel = self.sec(&format!(".rel{}", self.secs[sec].0))?;
        let addend = u32_at(self.bytes(sec, off as usize, 4)?, 0)?;
        for i in 0..rel.2 / 8 {
            let o = rel.1 + i * 8;
            if u32_at(self.d, o)? == off {
                let info = u32_at(self.d, o + 4)?;
                if info & 0xff != 2 {
                    return Err(bad("relocation is not R_ARM_ABS32"));
                }
                let s = self
                    .syms
                    .get((info >> 8) as usize)
                    .ok_or_else(|| bad("relocation symbol"))?;
                return Ok((s.4, s.1.wrapping_add(addend)));
            }
        }
        Err(bad("pointer without relocation"))
    }
}

/// The vendor array for table entry `table` in `gslx680_fw_grp[]` ({name, records, fw}, 12 bytes).
pub fn table_from_module(ko: &[u8], table: &str) -> R<Vec<u8>> {
    let elf = Elf::parse(ko)?;
    let (sec, start, size) = elf.object("gslx680_fw_grp")?;
    for o in (start..start + size / 12 * 12).step_by(12) {
        let (nsec, noff) = elf.pointer(sec, o)?;
        let s = elf.secs.get(nsec).ok_or_else(|| bad("name section"))?;
        if cstr(elf.d, s.1 + noff as usize)? != table {
            continue;
        }
        let records = u32_at(elf.bytes(sec, o as usize + 4, 4)?, 0)? as usize;
        let (fsec, foff) = elf.pointer(sec, o + 8)?;
        return Ok(elf.bytes(fsec, foff as usize, records * 8)?.to_vec());
    }
    Err(bad(&format!("no table entry {table}")))
}

/// Pages of {0xf0 page} + 32 registers 0x00..0x7c, as mainline silead.c expects; byte-identical
/// to the mainline `.fw` layout (extract_gsl_fw.py "check").
pub fn layout_ok(fw: &[u8]) -> bool {
    if fw.is_empty() || !fw.len().is_multiple_of(33 * 8) {
        return false;
    }
    let rec = |i: usize| {
        (
            u32_at(fw, i * 8).unwrap_or(u32::MAX),
            u32_at(fw, i * 8 + 4).unwrap_or(0),
        )
    };
    let mut pages = Vec::new();
    for p in 0..fw.len() / (33 * 8) {
        let (reg, page) = rec(p * 33);
        if reg != PAGE_REG || pages.contains(&page) {
            return false;
        }
        pages.push(page);
        if (0..32).any(|k| rec(p * 33 + 1 + k).0 != 4 * k as u32) {
            return false;
        }
    }
    true
}

/// Boot image -> ramdisk -> module -> table, verified against `want` (SHA-256 hex).
pub fn extract(
    read: &dyn Fn(u64, &mut [u8]) -> io::Result<()>,
    base: u64,
    want: &str,
) -> R<Vec<u8>> {
    let mut hdr = [0u8; 64];
    read(base, &mut hdr)?;
    let (off, len) = ramdisk_span(&hdr)?;
    let mut rd = vec![0u8; len];
    read(base + off, &mut rd)?;
    let cpio = gunzip(&rd, 4 * MAX_RAMDISK)?;
    let ko = cpio_find(&cpio, MODULE).ok_or_else(|| bad("no gslX680new.ko in the ramdisk"))?;
    let fw = table_from_module(ko, TABLE)?;
    if !layout_ok(&fw) {
        return Err(bad("table layout is not the silead one"));
    }
    let got = sha256::sha256_hex(&fw);
    if got != want {
        return Err(bad(&format!("sha256 {got}, not the tested firmware")));
    }
    Ok(fw)
}

/// The firmware from the first non-thinframe disk that has it. Opened read-only.
fn from_emmc() -> R<Vec<u8>> {
    let mut why = String::from("no eMMC");
    for d in slot::DISKS {
        let Ok(f) = fs::File::open(d) else { continue };
        let mut mbr = [0u8; 512];
        if f.read_exact_at(&mut mbr, 0).is_err() || slot::mbr_is_ours(&mbr) {
            continue;
        }
        for sector in BOOT_IMAGES {
            match extract(&|o, b| f.read_exact_at(b, o), sector * 512, SHA256) {
                Ok(fw) => {
                    eprintln!("tfctl: boot: touch firmware read from {d} sector {sector}");
                    return Ok(fw);
                }
                Err(e) => why = format!("{d} sector {sector}: {e}"),
            }
        }
    }
    Err(bad(&why))
}

/// Touch controllers in the device tree that no driver holds yet.
fn unbound_touch() -> Vec<String> {
    let Ok(rd) = fs::read_dir("/sys/bus/i2c/devices") else {
        return Vec::new();
    };
    rd.flatten()
        .filter(|e| {
            let p = e.path();
            fs::read_to_string(p.join("modalias")).is_ok_and(|m| m.contains("silead"))
                && !p.join("driver").exists()
        })
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

/// The boot step: nothing to do when the image has the firmware or there is no touch controller.
pub fn ensure() -> io::Result<()> {
    if Path::new(IMAGE_FW).exists() {
        return Ok(());
    }
    let devs = unbound_touch();
    if devs.is_empty() {
        return Ok(());
    }
    let cached = Path::new(DATA_DIR).join(FW_NAME);
    let fresh = fs::read(&cached).is_ok_and(|b| sha256::sha256_hex(&b) == SHA256);
    if !fresh {
        let fw = from_emmc()?;
        fs::create_dir_all(cached.parent().unwrap())?;
        let tmp = cached.with_extension("tmp");
        fs::write(&tmp, &fw)?;
        fs::rename(&tmp, &cached)?;
    }
    fs::write(FW_PATH_PARAM, DATA_DIR)?;
    let _ = std::process::Command::new("modprobe")
        .arg("silead")
        .status();
    for d in &devs {
        let _ = fs::write("/sys/bus/i2c/drivers_probe", d);
    }
    match unbound_touch() {
        left if left.is_empty() => Ok(()),
        left => Err(bad(&format!("{} did not bind", left.join(", ")))),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// One page: {0xf0, 0} + registers 0x00..0x7c.
    fn page_fw() -> Vec<u8> {
        let mut v = Vec::new();
        for (r, val) in std::iter::once((PAGE_REG, 0)).chain((0..32).map(|k| (4 * k, 0x1000 + k))) {
            v.extend_from_slice(&r.to_le_bytes());
            v.extend_from_slice(&val.to_le_bytes());
        }
        v
    }

    fn sh(name: u32, kind: u32, off: usize, size: usize, link: u32) -> Vec<u8> {
        let mut h = Vec::new();
        for w in [name, kind, 0, 0, off as u32, size as u32, link, 0, 4, 0] {
            h.extend_from_slice(&w.to_le_bytes());
        }
        h
    }

    /// A relocatable ELF32 with `gslx680_fw_grp` = [{"other", 1, junk}, {"gsl_m86_hd", n, fw}].
    pub(crate) fn module(fw: &[u8]) -> Vec<u8> {
        // .rodata: names, then the array at 32.
        let mut ro = b"other\0gsl_m86_hd\0".to_vec();
        ro.resize(32, 0);
        ro.extend_from_slice(fw);
        let mut data = Vec::new();
        for (name_off, n, fw_off) in [(0u32, 1u32, 32u32), (6, (fw.len() / 8) as u32, 32)] {
            for w in [name_off, n, fw_off] {
                data.extend_from_slice(&w.to_le_bytes());
            }
        }
        // Relocations at .data+0, +8, +12, +20 against symbol 1 (the .rodata section symbol).
        let mut rel = Vec::new();
        for off in [0u32, 8, 12, 20] {
            rel.extend_from_slice(&off.to_le_bytes());
            rel.extend_from_slice(&((1u32 << 8) | 2).to_le_bytes());
        }
        let strtab = b"\0gslx680_fw_grp\0".to_vec();
        let mut sym = vec![0u8; 16];
        let mut s = |name: u32, value: u32, size: u32, info: u8, shndx: u16| {
            sym.extend_from_slice(&name.to_le_bytes());
            sym.extend_from_slice(&value.to_le_bytes());
            sym.extend_from_slice(&size.to_le_bytes());
            sym.extend_from_slice(&[info, 0]);
            sym.extend_from_slice(&shndx.to_le_bytes());
        };
        s(0, 0, 0, 3, 2); // section symbol for .rodata
        s(1, 0, 24, 0x11, 1); // global object gslx680_fw_grp in .data
        let shstr = b"\0.data\0.rodata\0.symtab\0.strtab\0.rel.data\0.shstrtab\0".to_vec();
        let parts = [&data, &ro, &sym, &strtab, &rel, &shstr];
        let mut out = vec![0u8; 52];
        let mut offs = Vec::new();
        for p in parts {
            offs.push(out.len());
            out.extend_from_slice(p);
            out.resize((out.len() + 3) & !3, 0);
        }
        let shoff = out.len();
        out.extend(sh(0, 0, 0, 0, 0));
        let names = [1u32, 7, 15, 23, 31, 41];
        let kinds = [1u32, 1, 2, 3, 9, 3];
        for i in 0..6 {
            let link = if i == 2 {
                4
            } else if i == 4 {
                3
            } else {
                0
            };
            out.extend(sh(names[i], kinds[i], offs[i], parts[i].len(), link));
        }
        out[..6].copy_from_slice(b"\x7fELF\x01\x01");
        out[0x20..0x24].copy_from_slice(&(shoff as u32).to_le_bytes());
        out[0x2e..0x30].copy_from_slice(&40u16.to_le_bytes());
        out[0x30..0x32].copy_from_slice(&7u16.to_le_bytes());
        out[0x32..0x34].copy_from_slice(&6u16.to_le_bytes());
        out
    }

    fn cpio(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut a = Vec::new();
        let trailer: &[u8] = &[];
        for (name, body) in files
            .iter()
            .copied()
            .chain(std::iter::once(("TRAILER!!!", trailer)))
        {
            a.extend_from_slice(b"070701");
            for v in [
                0,
                0o100644,
                0,
                0,
                1,
                0,
                body.len(),
                0,
                0,
                0,
                0,
                name.len() + 1,
                0,
            ] {
                a.extend_from_slice(format!("{v:08x}").as_bytes());
            }
            a.extend_from_slice(name.as_bytes());
            a.push(0);
            a.resize((a.len() + 3) & !3, 0);
            a.extend_from_slice(body);
            a.resize((a.len() + 3) & !3, 0);
        }
        a
    }

    /// gzip with stored deflate blocks and a file name.
    fn gzip(d: &[u8]) -> Vec<u8> {
        let mut g = vec![0x1f, 0x8b, 8, 8, 0, 0, 0, 0, 0, 3];
        g.extend_from_slice(b"ramdisk.cpio\0");
        let chunks: Vec<&[u8]> = d.chunks(65535).collect();
        for (i, c) in chunks.iter().enumerate() {
            g.push((i + 1 == chunks.len()) as u8);
            g.extend_from_slice(&(c.len() as u16).to_le_bytes());
            g.extend_from_slice(&(!(c.len() as u16)).to_le_bytes());
            g.extend_from_slice(c);
        }
        g.extend_from_slice(&[0; 8]);
        g
    }

    fn boot_image(ramdisk: &[u8]) -> Vec<u8> {
        let kernel = vec![0xaa; 5000];
        let mut b = vec![0u8; 2048];
        b[..8].copy_from_slice(b"ANDROID!");
        b[8..12].copy_from_slice(&(kernel.len() as u32).to_le_bytes());
        b[16..20].copy_from_slice(&(ramdisk.len() as u32).to_le_bytes());
        b[36..40].copy_from_slice(&2048u32.to_le_bytes());
        b.extend_from_slice(&kernel);
        b.resize(2048 * 4, 0);
        b.extend_from_slice(ramdisk);
        b
    }

    #[test]
    fn reads_the_table_out_of_a_boot_image() {
        let fw: Vec<u8> = [page_fw(), {
            let mut p = page_fw();
            p[4] = 1; // second page number
            p
        }]
        .concat();
        let ko = module(&fw);
        assert_eq!(table_from_module(&ko, "gsl_m86_hd").unwrap(), fw);
        assert_eq!(table_from_module(&ko, "other").unwrap(), fw[..8].to_vec());
        assert!(table_from_module(&ko, "gsl_x").is_err());
        let rd = gzip(&cpio(&[
            ("init", b"#!"),
            ("lib/modules/gslX680new.ko", &ko),
        ]));
        let mut disk = vec![0u8; 4096];
        disk.extend(boot_image(&rd));
        let read = |o: u64, b: &mut [u8]| -> io::Result<()> {
            let s = disk
                .get(o as usize..o as usize + b.len())
                .ok_or(ErrorKind::UnexpectedEof)?;
            b.copy_from_slice(s);
            Ok(())
        };
        let want = sha256::sha256_hex(&fw);
        assert_eq!(extract(&read, 4096, &want).unwrap(), fw);
        let e = extract(&read, 4096, SHA256).unwrap_err().to_string();
        assert!(e.contains("not the tested firmware"), "{e}");
        assert!(extract(&read, 0, &want).is_err(), "no ANDROID! magic at 0");
    }

    #[test]
    fn layout_checks_pages() {
        let p = page_fw();
        assert!(layout_ok(&p));
        assert!(!layout_ok(&[p.clone(), p.clone()].concat()), "page twice");
        let mut bad_reg = p.clone();
        bad_reg[8] = 8;
        assert!(!layout_ok(&bad_reg));
        assert!(!layout_ok(&p[..p.len() - 8]));
        assert!(!layout_ok(&[]));
    }

    #[test]
    fn rejects_odd_headers_and_archives() {
        let mut h = vec![0u8; 64];
        assert!(ramdisk_span(&h).is_err());
        h[..8].copy_from_slice(b"ANDROID!");
        h[8..12].copy_from_slice(&10_221_784u32.to_le_bytes());
        h[16..20].copy_from_slice(&5_660_297u32.to_le_bytes());
        h[36..40].copy_from_slice(&2048u32.to_le_bytes());
        // The frame's image (E025): header page + kernel 10,221,784 bytes in 4,992 pages.
        assert_eq!(ramdisk_span(&h).unwrap(), (2048 * 4993, 5_660_297));
        h[36..40].copy_from_slice(&1000u32.to_le_bytes());
        assert!(ramdisk_span(&h).is_err());
        assert!(gunzip(b"not gzip at all, really", 100).is_err());
        assert!(cpio_find(b"garbage", "x").is_none());
        let a = cpio(&[("a/gslX680new.ko.bak", b"no"), ("b", b"x")]);
        assert!(cpio_find(&a, MODULE).is_none());
        assert_eq!(cpio_find(&a, "b"), Some(&b"x"[..]));
        assert!(Elf::parse(b"\x7fELF\x02\x01").is_err());
    }
}
