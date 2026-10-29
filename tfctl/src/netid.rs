//! A Wi-Fi address that is the same at every boot (docs/decisions.md "A stable Wi-Fi address").
//!
//! The AIC8800 has no address in its efuse, so its driver made one up at every load: 88:00:33:77
//! and 16 random bits (rwnx_main.c). The router saw a new client at each boot and handed out a
//! new IP: 88:00:33:77:a3:fa, :24:ed, :68:51 on three boots [V 2026-10-27]. The driver now
//! takes `mac=` (image/buildroot/package/aic8800/mac-param.diff), and this module derives it
//! from the SoC's Security ID: per chip, read-only, and the same after an A/B update, on a
//! fresh card and on the eMMC install. `speed::load_wifi_early` loads the driver with it.
//!
//! `tfctl netid` prints the address, its source, and what the driver and wlan0 have.
use std::fs;
use std::io;
use std::path::Path;

use crate::sha256::Sha256;

const MODULE: &str = "aic8800_fdrv";

/// Where the chip ID came from.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Source {
    /// The Security ID's root key, read through the sunxi-sid nvmem device. image/build.sh adds
    /// the node to the A33 device trees (mainline has none); the H3 that QEMU emulates has one.
    Sid,
    /// The serial number U-Boot puts in the device tree (`serial#`, which it derives from the
    /// SID or the PMIC). Only when there is no SID device.
    DtSerial,
}

impl Source {
    fn name(self) -> &'static str {
        match self {
            Source::Sid => "sunxi-sid",
            Source::DtSerial => "device-tree serial-number",
        }
    }
}

/// The chip ID under `root` ("/" on the frame): the first SID device that reads a usable
/// value, else U-Boot's serial number. None: the driver keeps its made-up address.
pub fn chip_id(root: &Path) -> Option<(Vec<u8>, Source)> {
    let dir = root.join("sys/bus/nvmem/devices");
    let mut sids: Vec<_> = fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("sunxi-sid"))
        .collect();
    sids.sort();
    for n in sids {
        if let Ok(mut b) = fs::read(dir.join(n).join("nvmem")) {
            b.truncate(16); // the root key; later words (H3: thermal calibration) are not an ID
            if usable(&b) {
                return Some((b, Source::Sid));
            }
        }
    }
    let mut s = fs::read(root.join("sys/firmware/devicetree/base/serial-number")).ok()?;
    while s.last() == Some(&0) {
        s.pop();
    }
    usable(&s).then_some((s, Source::DtSerial))
}

/// An ID worth deriving from: at least 8 bytes and not one byte repeated (an unprogrammed
/// efuse reads all zeros or all ones; U-Boot writes "0000000000000000" when it read nothing).
pub fn usable(id: &[u8]) -> bool {
    id.len() >= 8 && id.iter().any(|&b| b != id[0])
}

/// SHA-256 of a fixed label and the ID, first six bytes, as a locally administered unicast
/// address: bit 1 of the first byte set, bit 0 (multicast) clear. The hash keeps the raw SID
/// off the network, and the label keeps this address apart from any other derived later.
pub fn derive(id: &[u8]) -> [u8; 6] {
    let mut h = Sha256::new();
    h.update(b"thinframe wlan0\0");
    h.update(id);
    let d = h.finish();
    let mut m = [d[0], d[1], d[2], d[3], d[4], d[5]];
    m[0] = (m[0] & 0xfc) | 0x02;
    m
}

pub fn format(m: &[u8; 6]) -> String {
    m.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// The module arguments for the Wi-Fi driver: `mac=` when the chip has an ID, else none.
pub fn module_args() -> Vec<String> {
    match chip_id(Path::new("/")) {
        Some((id, src)) => {
            let mac = format(&derive(&id));
            crate::boot::note(&format!("netid: wlan0 {mac} (from {})", src.name()));
            vec![format!("mac={mac}")]
        }
        None => {
            crate::boot::note("netid: no chip ID; the Wi-Fi driver picks a random address");
            Vec::new()
        }
    }
}

/// `tfctl netid`: the derived address and what the driver and wlan0 actually have.
pub fn run() -> io::Result<()> {
    let read = |p: &str| fs::read_to_string(p).map(|s| s.trim().to_string()).ok();
    let want = chip_id(Path::new("/")).map(|(id, src)| (format(&derive(&id)), src.name()));
    match &want {
        Some((mac, src)) => println!("address : {mac} (from {src})"),
        None => println!("address : none (no chip ID)"),
    }
    let param = read(&format!("/sys/module/{MODULE}/parameters/mac"));
    println!("driver  : {}", param.as_deref().unwrap_or("not loaded"));
    let wlan = read("/sys/class/net/wlan0/address");
    println!("wlan0   : {}", wlan.as_deref().unwrap_or("absent"));
    match (want, wlan) {
        (Some((m, _)), Some(w)) if m != w => Err(io::Error::other("wlan0 has another address")),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sysroot(tag: &str) -> PathBuf {
        let r = std::env::temp_dir().join(format!("tfctl-netid-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&r);
        fs::create_dir_all(r.join("sys/bus/nvmem/devices")).unwrap();
        fs::create_dir_all(r.join("sys/firmware/devicetree/base")).unwrap();
        r
    }

    fn put(root: &Path, rel: &str, data: &[u8]) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, data).unwrap();
    }

    #[test]
    fn address_is_local_unicast_and_fixed() {
        let id = [
            0x16, 0x67, 0x00, 0x4a, 0x81, 0x1c, 0x23, 0x80, 1, 2, 3, 4, 5, 6, 7, 8,
        ];
        let m = derive(&id);
        assert_eq!(m[0] & 0x01, 0, "unicast");
        assert_eq!(m[0] & 0x02, 0x02, "locally administered");
        assert_eq!(m, derive(&id), "same ID, same address");
        // Pinned (Python hashlib): a change here moves every frame to a new address, and so
        // to a new DHCP lease and IP.
        assert_eq!(format(&derive(b"0123456789abcdef")), "46:0d:db:f1:d8:b4");
    }

    #[test]
    fn one_bit_of_id_changes_the_address() {
        let a = *b"0123456789abcdef";
        let mut b = a;
        b[15] ^= 1;
        assert_ne!(derive(&a), derive(&b));
        assert_ne!(derive(b"serial-A"), derive(b"serial-B"));
    }

    #[test]
    fn blank_ids_are_refused() {
        assert!(!usable(&[0u8; 16]), "unprogrammed efuse");
        assert!(!usable(&[0xffu8; 16]), "unprogrammed efuse, other polarity");
        assert!(
            !usable(b"0000000000000000"),
            "U-Boot's serial when it read nothing"
        );
        assert!(!usable(&[1, 2, 3]), "too short");
        assert!(usable(&[0x16, 0x67, 0, 0, 0, 0, 0, 1]));
    }

    #[test]
    fn sid_first_then_device_tree_serial() {
        let r = sysroot("order");
        assert_eq!(chip_id(&r), None, "nothing to read");

        put(
            &r,
            "sys/firmware/devicetree/base/serial-number",
            b"1667a0b1c2d3e4f5\0",
        );
        assert_eq!(
            chip_id(&r),
            Some((b"1667a0b1c2d3e4f5".to_vec(), Source::DtSerial))
        );

        // H3-sized SID (256 bytes): only the root key counts, so calibration words never move it.
        let mut sid = vec![0u8; 256];
        sid[..16]
            .copy_from_slice(b"\x02\xc0\x00\x81\x11\x11\x22\x22\x33\x33\x00\x00\x44\x55\x66\x77");
        sid[200] = 0x5a;
        put(&r, "sys/bus/nvmem/devices/sunxi-sid0/nvmem", &sid);
        let (id, src) = chip_id(&r).unwrap();
        assert_eq!((id.len(), src), (16, Source::Sid));
        assert_eq!(id, sid[..16]);

        // Another nvmem device (the RTC) is never an ID.
        put(&r, "sys/bus/nvmem/devices/rtc0/nvmem", b"0123456789abcdef");
        assert_eq!(chip_id(&r).unwrap().1, Source::Sid);
        let _ = fs::remove_dir_all(&r);
    }

    #[test]
    fn blank_sid_falls_back_to_the_serial() {
        let r = sysroot("blank");
        put(&r, "sys/bus/nvmem/devices/sunxi-sid0/nvmem", &[0u8; 16]);
        assert_eq!(
            chip_id(&r),
            None,
            "blank SID and no serial: no ID, not a shared address"
        );
        put(
            &r,
            "sys/firmware/devicetree/base/serial-number",
            b"0000000000000000\0",
        );
        assert_eq!(chip_id(&r), None, "U-Boot found nothing either");
        put(
            &r,
            "sys/firmware/devicetree/base/serial-number",
            b"8100c0020b2c5a8e\0",
        );
        assert_eq!(chip_id(&r).unwrap().1, Source::DtSerial);
        let _ = fs::remove_dir_all(&r);
    }
}
