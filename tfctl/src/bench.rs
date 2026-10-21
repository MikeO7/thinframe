//! `tfctl bench [TAPS]` (TASK 13): touch to first paint, measured on the frame itself. A virtual
//! touchscreen (/dev/uinput) taps the middle of the panel through the same udev + libinput path
//! as the real one. The page's beacon (page.js) tells the server when its response was on screen
//! (`painted_at_ms`, the frame's wall clock); this reads that back and prints tap -> paint per tap.
//! Left out: the touch controller itself (I2C read and IRQ before the kernel has the event).
use crate::config::Config;
use crate::http::{self, Url};
use crate::json;
use std::io;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_ABS: u16 = 3;
const BTN_TOUCH: u16 = 0x14a;
const ABS_X: u16 = 0;
const ABS_Y: u16 = 1;
const ABS_MT_SLOT: u16 = 0x2f;
const ABS_MT_POSITION_X: u16 = 0x35;
const ABS_MT_POSITION_Y: u16 = 0x36;
const ABS_MT_TRACKING_ID: u16 = 0x39;
/// Axis range of the virtual screen; libinput scales it to the output, so the middle is RANGE/2.
const RANGE: i32 = 4096;
/// libinput needs a moment to see the new device (udev add, then its own probe).
const SETTLE: Duration = Duration::from_secs(3);
const PERIOD: Duration = Duration::from_secs(1);
const HOLD: Duration = Duration::from_millis(60);
const WAIT: Duration = Duration::from_secs(5);
/// The paint time comes from the page, so polling slowly costs no accuracy, only frame CPU.
const POLL: Duration = Duration::from_millis(100);

/// One touch frame of a single finger: (type, code, value), ending in SYN_REPORT.
fn touch_events(id: i32, x: i32, y: i32, down: bool) -> Vec<(u16, u16, i32)> {
    let mut e = vec![(EV_ABS, ABS_MT_SLOT, 0)];
    if down {
        e.extend([
            (EV_ABS, ABS_MT_TRACKING_ID, id),
            (EV_ABS, ABS_MT_POSITION_X, x),
            (EV_ABS, ABS_MT_POSITION_Y, y),
            (EV_KEY, BTN_TOUCH, 1),
            (EV_ABS, ABS_X, x),
            (EV_ABS, ABS_Y, y),
        ]);
    } else {
        e.extend([(EV_ABS, ABS_MT_TRACKING_ID, -1), (EV_KEY, BTN_TOUCH, 0)]);
    }
    e.push((EV_SYN, 0, 0));
    e
}

/// `struct input_event` as the kernel reads it from this process: two native longs of time
/// (ignored by uinput, the kernel stamps the event), type, code, value.
fn encode(events: &[(u16, u16, i32)]) -> Vec<u8> {
    let mut b = Vec::new();
    for &(t, c, v) in events {
        b.extend_from_slice(&[0u8; 2 * size_of::<usize>()]);
        b.extend_from_slice(&t.to_ne_bytes());
        b.extend_from_slice(&c.to_ne_bytes());
        b.extend_from_slice(&v.to_ne_bytes());
    }
    b
}

/// The page's last measurement in a GET /api/frames/{id} reply: (painted_at_ms, page_ms).
fn touch_of(body: &str) -> Option<(i64, i64)> {
    let t = json::parse(body).ok()?.get("touch")?.clone();
    Some((
        t.get("painted_at_ms")?.as_i64()?,
        t.get("last_ms")?.as_i64()?,
    ))
}

/// Median and maximum.
fn summary(v: &[i64]) -> Option<(i64, i64)> {
    let mut s = v.to_vec();
    s.sort_unstable();
    Some((*s.get(s.len() / 2)?, *s.last()?))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn painted_after(url: &Url, token: &str, t0: i64) -> io::Result<Option<(i64, i64)>> {
    let start = Instant::now();
    while start.elapsed() < WAIT {
        let r = http::get(url, token, WAIT)?;
        if r.status != 200 {
            return Err(io::Error::other(format!(
                "GET {}: HTTP {}",
                url.path, r.status
            )));
        }
        match touch_of(&String::from_utf8_lossy(&r.body)) {
            Some((at, page)) if at >= t0 => return Ok(Some((at - t0, page))),
            _ => thread::sleep(POLL),
        }
    }
    Ok(None)
}

pub fn run(cfg: &Config, args: &[String]) -> io::Result<()> {
    let taps = match args.first() {
        Some(n) => n
            .parse()
            .map_err(|_| io::Error::other("usage: tfctl bench [TAPS]"))?,
        None => TAPS,
    };
    println!("{}", measure(cfg, taps, &mut |l| println!("{l}"))?);
    Ok(())
}

pub const TAPS: usize = 10;

/// Taps `taps` times, one line per tap to `line`; returns the summary line. Needs root
/// (/dev/uinput), so over the network it runs as the agent's `bench` action.
pub fn measure(cfg: &Config, taps: usize, line: &mut dyn FnMut(&str)) -> io::Result<String> {
    let base = Url::parse(&cfg.server_url).ok_or_else(|| {
        io::Error::other("bench needs SERVER_URL: the page reports to the server")
    })?;
    let url = base.with_path(&format!("/api/frames/{}", cfg.frame_id));
    let dev = uinput::Device::create(RANGE)?;
    thread::sleep(SETTLE);
    let (mut total, mut page) = (Vec::new(), Vec::new());
    for i in 0..taps {
        let t0 = now_ms();
        dev.write(&encode(&touch_events(
            i as i32 + 1,
            RANGE / 2,
            RANGE / 2,
            true,
        )))?;
        thread::sleep(HOLD);
        dev.write(&encode(&touch_events(0, 0, 0, false)))?;
        match painted_after(&url, &cfg.token, t0)? {
            Some((ms, p)) => {
                line(&format!(
                    "tap {:2}: {ms} ms tap -> paint (page: {p} ms)",
                    i + 1
                ));
                total.push(ms);
                page.push(p);
            }
            None => line(&format!(
                "tap {:2}: no paint reported within {WAIT:?}",
                i + 1
            )),
        }
        thread::sleep(PERIOD);
    }
    match (summary(&total), summary(&page)) {
        (Some((p50, max)), Some((pp50, pmax))) => Ok(format!(
            "{} of {taps} taps painted; tap -> paint p50 {p50} ms, max {max} ms; \
             in the page p50 {pp50} ms, max {pmax} ms",
            total.len()
        )),
        _ => Err(io::Error::other(
            "no tap reached the page: is the dashboard up and the frame paired?",
        )),
    }
}

#[cfg(target_os = "linux")]
mod uinput {
    use super::{
        ABS_MT_POSITION_X, ABS_MT_POSITION_Y, ABS_MT_SLOT, ABS_X, ABS_Y, BTN_TOUCH, EV_ABS, EV_KEY,
    };
    use std::ffi::{c_int, c_ulong};
    use std::fs::{File, OpenOptions};
    use std::io::{self, Write};
    use std::os::fd::AsRawFd;

    extern "C" {
        fn ioctl(fd: c_int, req: c_ulong, ...) -> c_int;
    }

    // include/uapi/linux/uinput.h
    const fn io(nr: u32) -> c_ulong {
        (((b'U' as u32) << 8) | nr) as c_ulong
    }
    const fn iow(nr: u32, size: usize) -> c_ulong {
        ((1u32 << 30) | ((size as u32) << 16) | ((b'U' as u32) << 8) | nr) as c_ulong
    }
    const INPUT_PROP_DIRECT: c_int = 1;
    const AXES: [u16; 5] = [
        ABS_X,
        ABS_Y,
        ABS_MT_SLOT,
        ABS_MT_POSITION_X,
        ABS_MT_POSITION_Y,
    ];

    #[repr(C)]
    struct Setup {
        bustype: u16,
        vendor: u16,
        product: u16,
        version: u16,
        name: [u8; 80],
        ff_effects_max: u32,
    }
    #[repr(C)]
    struct AbsSetup {
        code: u16,
        _pad: u16,
        value: i32,
        minimum: i32,
        maximum: i32,
        fuzz: i32,
        flat: i32,
        resolution: i32,
    }

    pub struct Device(File);

    impl Device {
        fn ctl(&self, req: c_ulong, arg: c_ulong, what: &str) -> io::Result<()> {
            // SAFETY: `arg` is an int or a pointer to a live repr(C) struct of the request's size.
            if unsafe { ioctl(self.0.as_raw_fd(), req, arg) } == 0 {
                Ok(())
            } else {
                let e = io::Error::last_os_error();
                Err(io::Error::new(e.kind(), format!("uinput {what}: {e}")))
            }
        }

        /// A direct-touch screen with one slot, axes 0..range.
        pub fn create(range: i32) -> io::Result<Device> {
            let f = OpenOptions::new()
                .write(true)
                .open("/dev/uinput")
                .map_err(|e| io::Error::new(e.kind(), format!("/dev/uinput: {e}")))?;
            let d = Device(f);
            let set = |nr, v: u16| d.ctl(iow(nr, 4), v as c_ulong, "set bit");
            set(100, EV_KEY)?;
            set(100, EV_ABS)?;
            set(101, BTN_TOUCH)?;
            d.ctl(iow(110, 4), INPUT_PROP_DIRECT as c_ulong, "set prop")?;
            for code in AXES {
                set(103, code)?;
                let slot = code == ABS_MT_SLOT;
                let a = AbsSetup {
                    code,
                    _pad: 0,
                    value: 0,
                    minimum: 0,
                    maximum: if slot { 0 } else { range - 1 },
                    fuzz: 0,
                    flat: 0,
                    resolution: if slot { 0 } else { 16 },
                };
                d.ctl(
                    iow(4, size_of::<AbsSetup>()),
                    &a as *const _ as c_ulong,
                    "abs setup",
                )?;
            }
            let mut s = Setup {
                bustype: 0x06, // BUS_VIRTUAL
                vendor: 0x7466,
                product: 0x0001,
                version: 1,
                name: [0; 80],
                ff_effects_max: 0,
            };
            let name = b"tfctl bench touchscreen";
            s.name[..name.len()].copy_from_slice(name);
            d.ctl(
                iow(3, size_of::<Setup>()),
                &s as *const _ as c_ulong,
                "setup",
            )?;
            d.ctl(io(1), 0, "create")?;
            Ok(d)
        }

        pub fn write(&self, bytes: &[u8]) -> io::Result<()> {
            (&self.0).write_all(bytes)
        }
    }

    impl Drop for Device {
        fn drop(&mut self) {
            let _ = self.ctl(io(2), 0, "destroy");
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod uinput {
    use std::io;
    pub struct Device;
    impl Device {
        pub fn create(_range: i32) -> io::Result<Device> {
            Err(io::Error::other("bench needs Linux (/dev/uinput)"))
        }
        pub fn write(&self, _bytes: &[u8]) -> io::Result<()> {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tap_is_down_then_up_each_closed_by_a_report() {
        let down = touch_events(7, 2048, 2048, true);
        assert_eq!(down.first(), Some(&(EV_ABS, ABS_MT_SLOT, 0)));
        assert!(down.contains(&(EV_ABS, ABS_MT_TRACKING_ID, 7)));
        assert!(down.contains(&(EV_KEY, BTN_TOUCH, 1)));
        assert_eq!(down.last(), Some(&(EV_SYN, 0, 0)));
        let up = touch_events(0, 0, 0, false);
        assert!(up.contains(&(EV_ABS, ABS_MT_TRACKING_ID, -1)));
        assert!(up.contains(&(EV_KEY, BTN_TOUCH, 0)));
        assert_eq!(up.last(), Some(&(EV_SYN, 0, 0)));
    }

    #[test]
    fn input_event_layout_matches_the_kernel() {
        // 16 bytes on armv7 (two 32-bit longs), 24 on 64-bit hosts.
        let b = encode(&[(EV_KEY, BTN_TOUCH, 1)]);
        let t = 2 * size_of::<usize>();
        assert_eq!(b.len(), t + 8);
        assert_eq!(&b[t..t + 2], &EV_KEY.to_ne_bytes());
        assert_eq!(&b[t + 2..t + 4], &BTN_TOUCH.to_ne_bytes());
        assert_eq!(&b[t + 4..], &1i32.to_ne_bytes());
    }

    #[test]
    fn reads_the_pages_report_and_summarises() {
        let body =
            r#"{"id":"f","touch":{"last_ms":21,"painted_at_ms":1700000000123,"samples_ms":[21]}}"#;
        assert_eq!(touch_of(body), Some((1700000000123, 21)));
        assert_eq!(touch_of(r#"{"id":"f"}"#), None);
        assert_eq!(summary(&[30, 10, 20]), Some((20, 30)));
        assert_eq!(summary(&[]), None);
    }
}
