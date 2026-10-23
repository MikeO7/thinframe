//! Dark room = screen off. The PF1007L's light sensor is an analog part behind the small hole at
//! the bottom left of the bezel, wired to LRADC0 of the A33, not an I2C chip (ormarch E063/E064,
//! docs/hardware.md). No mainline driver reports it, so tfctl reads the converter's two registers
//! through /dev/mem as the vendor module did. The kernel's key driver for that pin is off
//! (linux.fragment), or it would hold the converter in its own mode and report volume keys.
//!
//! Upstream measured 60..62 in a lit room and 0..2 with the hole covered (6-bit value).
//! `Dark` turns that into one bit with hysteresis; the kiosk combines it with the schedule.
use std::io;
use std::time::{Duration, Instant};

/// At or below this the room counts as dark.
pub const DARK_MAX: u8 = 3;
/// At or above this the room counts as lit again (the vendor's first brightness band).
pub const LIGHT_MIN: u8 = 10;
/// Dark this long before the screen goes off: a hand or a shadow over the hole is not night.
pub const DARK_FOR: Duration = Duration::from_secs(120);
/// Lit this long before it comes back on: a passing headlight is not morning.
pub const LIGHT_FOR: Duration = Duration::from_secs(3);
/// Median over this many samples (one a second).
const WINDOW: usize = 5;

/// The light bit, from one sample a second.
pub struct Dark {
    window: Vec<u8>,
    dark: bool,
    since: Option<Instant>,
}

impl Default for Dark {
    fn default() -> Self {
        Self::new()
    }
}

impl Dark {
    pub fn new() -> Dark {
        Dark {
            window: Vec::with_capacity(WINDOW),
            dark: false,
            since: None,
        }
    }

    pub fn is_dark(&self) -> bool {
        self.dark
    }

    /// Feed one reading. Some(new state) when the room changed from lit to dark or back.
    pub fn step(&mut self, value: u8, now: Instant) -> Option<bool> {
        if self.window.len() == WINDOW {
            self.window.remove(0);
        }
        self.window.push(value);
        let mut s = self.window.clone();
        s.sort_unstable();
        let median = s[s.len() / 2];
        let (other, hold) = if self.dark {
            (median >= LIGHT_MIN, LIGHT_FOR)
        } else {
            (median <= DARK_MAX, DARK_FOR)
        };
        if !other {
            self.since = None;
            return None;
        }
        let since = *self.since.get_or_insert(now);
        if now.duration_since(since) < hold {
            return None;
        }
        self.dark = !self.dark;
        self.since = None;
        Some(self.dark)
    }
}

/// True on the PF1007L board (not QEMU's orangepi-pc, not the sim, not a Mac): the LRADC address
/// below is only the light sensor there.
pub fn on_pf1007l() -> bool {
    std::fs::read("/proc/device-tree/compatible")
        .is_ok_and(|c| c.split(|b| *b == 0).any(|s| s == b"efercro,pf1007l"))
}

/// `tfctl light`: one reading, for tuning the thresholds at the frame.
pub fn run() -> io::Result<()> {
    let mut s = Sensor::open()?;
    let v = s.read();
    println!(
        "light {v} (0..63; dark at <= {DARK_MAX} for {} s, lit again at >= {LIGHT_MIN})",
        DARK_FOR.as_secs()
    );
    Ok(())
}

#[cfg(target_os = "linux")]
mod linux {
    use std::ffi::{c_int, c_void};
    use std::fs::OpenOptions;
    use std::io;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    use std::time::Duration;

    extern "C" {
        fn mmap(
            addr: *mut c_void,
            len: usize,
            prot: c_int,
            flags: c_int,
            fd: c_int,
            off: i64,
        ) -> *mut c_void;
    }

    /// The A33 LRADC page; control at +0x800, channel 0 data at +0x80c (6 bits).
    const PAGE: i64 = 0x01C2_2000;
    const CTRL: usize = 0x800;
    const DATA0: usize = 0x80C;
    /// What the vendor sunxi-keyboard.ko writes: channel 0 continuous, as measured upstream.
    const VENDOR_CTRL: u32 = 0x00C0_0041;
    const O_SYNC: i32 = 0o4010000;

    pub struct Sensor {
        base: *mut u8,
    }

    // SAFETY: the mapping is device memory owned by this value; no aliasing Rust references.
    unsafe impl Send for Sensor {}

    impl Sensor {
        pub fn open() -> io::Result<Sensor> {
            if !super::on_pf1007l() {
                return Err(io::Error::other("no PF1007L light sensor on this board"));
            }
            let f = OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(O_SYNC)
                .open("/dev/mem")?;
            // PROT_READ|PROT_WRITE, MAP_SHARED. The fd may close after mmap.
            // SAFETY: a fresh 4 KiB mapping of one MMIO page; checked for MAP_FAILED below.
            let p = unsafe { mmap(std::ptr::null_mut(), 4096, 3, 1, f.as_raw_fd(), PAGE) };
            if p as isize == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(Sensor { base: p.cast() })
        }

        fn reg(&self, off: usize) -> *mut u32 {
            // SAFETY: off is one of the two register offsets, inside the 4 KiB page.
            unsafe { self.base.add(off).cast() }
        }

        /// 0..63. Puts the converter in the vendor mode first if something changed it.
        pub fn read(&mut self) -> u8 {
            // SAFETY: aligned 32-bit MMIO registers inside our mapping.
            unsafe {
                if self.reg(CTRL).read_volatile() != VENDOR_CTRL {
                    self.reg(CTRL).write_volatile(VENDOR_CTRL);
                    std::thread::sleep(Duration::from_millis(100));
                }
                (self.reg(DATA0).read_volatile() & 0x3F) as u8
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::Sensor;

#[cfg(not(target_os = "linux"))]
pub struct Sensor;

#[cfg(not(target_os = "linux"))]
impl Sensor {
    pub fn open() -> io::Result<Sensor> {
        Err(io::Error::other("no light sensor on this host"))
    }
    pub fn read(&mut self) -> u8 {
        63
    }
}

/// The touch panel's event device, read without blocking: a touch wakes an off screen.
pub struct Touch {
    f: Option<std::fs::File>,
    next_try: Instant,
}

impl Touch {
    /// Not open yet; `touched` looks for the device once a minute until it is there.
    pub fn closed() -> Touch {
        Touch {
            f: None,
            next_try: Instant::now(),
        }
    }

    /// True when the panel was touched since the last call.
    pub fn touched(&mut self) -> bool {
        use std::io::Read;
        if self.f.is_none() && Instant::now() >= self.next_try {
            self.next_try = Instant::now() + Duration::from_secs(60);
            self.f = open_touch();
        }
        let Some(f) = &mut self.f else {
            return false;
        };
        let mut buf = [0u8; 1024];
        let mut seen = false;
        loop {
            match f.read(&mut buf) {
                Ok(0) => break,
                Ok(_) => seen = true,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => {
                    self.f = None; // device gone (module reload): stop trying
                    break;
                }
            }
        }
        seen
    }
}

fn open_touch() -> Option<std::fs::File> {
    let devices = std::fs::read_to_string("/proc/bus/input/devices").ok()?;
    let ev = touch_event(&devices)?;
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        const O_NONBLOCK: i32 = 0o4000;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(O_NONBLOCK)
            .open(format!("/dev/input/{ev}"))
            .ok()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = ev;
        None
    }
}

/// The `eventN` handler of the silead touch panel in /proc/bus/input/devices.
fn touch_event(devices: &str) -> Option<String> {
    devices
        .split("\n\n")
        .find(|b| b.contains("silead"))?
        .lines()
        .find(|l| l.starts_with("H: Handlers="))?
        .split_whitespace()
        .find(|w| w.starts_with("event"))
        .map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(d: &mut Dark, v: u8, t0: Instant, secs: u64) -> Vec<(u64, bool)> {
        (0..secs)
            .filter_map(|s| d.step(v, t0 + Duration::from_secs(s)).map(|x| (s, x)))
            .collect()
    }

    #[test]
    fn goes_dark_after_two_minutes_and_lit_after_three_seconds() {
        let t0 = Instant::now();
        let mut d = Dark::new();
        assert!(feed(&mut d, 61, t0, 30).is_empty());
        // The median needs 3 of 5 dark samples first (t=32), then DARK_FOR.
        let t1 = t0 + Duration::from_secs(30);
        let ch = feed(&mut d, 1, t1, 200);
        assert_eq!(ch, vec![(122, true)]);
        assert!(d.is_dark());
        let t2 = t1 + Duration::from_secs(200);
        let ch = feed(&mut d, 60, t2, 10);
        assert_eq!(ch, vec![(5, false)]);
        assert!(!d.is_dark());
    }

    #[test]
    fn a_shadow_or_a_flash_does_not_count() {
        let t0 = Instant::now();
        let mut d = Dark::new();
        assert!(feed(&mut d, 0, t0, 100).is_empty(), "100 s of hand");
        assert!(feed(&mut d, 60, t0 + Duration::from_secs(100), 5).is_empty());
        assert!(feed(&mut d, 0, t0 + Duration::from_secs(105), 100).is_empty());
        assert!(!d.is_dark());
        let mut d = Dark::new();
        feed(&mut d, 0, t0, 200);
        assert!(d.is_dark());
        // One bright sample is outvoted by the median; dim light (5) is neither.
        let t1 = t0 + Duration::from_secs(200);
        assert_eq!(d.step(60, t1), None);
        assert!(feed(&mut d, 5, t1 + Duration::from_secs(1), 60).is_empty());
        assert!(d.is_dark());
    }

    #[test]
    fn finds_the_silead_event_device() {
        let devs = "I: Bus=0019 Vendor=0001\nN: Name=\"axp20x-pek\"\nH: Handlers=kbd event0 \n\n\
                    I: Bus=0018 Vendor=0000\nN: Name=\"silead_ts\"\nP: Phys=input/ts\n\
                    H: Handlers=mouse0 event1 \nB: EV=b\n";
        assert_eq!(touch_event(devs).as_deref(), Some("event1"));
        assert_eq!(touch_event("N: Name=\"x\"\nH: Handlers=event0\n"), None);
    }

    #[test]
    fn not_a_pf1007l_here() {
        // Mac, CI and the sim have no such device tree; the sensor must refuse to open.
        if !on_pf1007l() {
            assert!(Sensor::open().is_err());
        }
    }
}
