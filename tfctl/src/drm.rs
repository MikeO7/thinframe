//! Raw DRM/KMS through ioctl, enough to put one dumb buffer on the first connected output
//! (TASK 15). No libdrm, no crates: the ioctl numbers and structs are from
//! `include/uapi/drm/drm.h` + `drm_mode.h` (pointers are `__u64` there, so the layouts are the
//! same on armv7 and x86_64). Linux only; on other hosts `Display::open` returns an error.
//! Whoever opens the device while no one is master becomes master. The splash drops master
//! (`drop_master`) before cog starts but keeps its buffer scanned out, so the picture stays
//! until cog sets its own (`shown` turns false); then the kiosk drops the `Display`.
#[cfg(not(target_os = "linux"))]
use std::io;

pub struct Mode {
    pub width: u32,
    pub height: u32,
}

#[cfg(target_os = "linux")]
mod linux {
    use super::Mode;
    use std::ffi::{c_int, c_ulong, c_void};
    use std::fs::OpenOptions;
    use std::io;
    use std::os::fd::AsRawFd;

    extern "C" {
        fn ioctl(fd: c_int, req: c_ulong, ...) -> c_int;
        fn mmap(
            addr: *mut c_void,
            len: usize,
            prot: c_int,
            flags: c_int,
            fd: c_int,
            off: i64,
        ) -> *mut c_void;
        fn munmap(addr: *mut c_void, len: usize) -> c_int;
    }

    const fn iowr(nr: u32, size: usize) -> c_ulong {
        ((3u32 << 30) | ((size as u32) << 16) | (0x64 << 8) | nr) as c_ulong
    }

    #[repr(C)]
    #[derive(Default)]
    struct CardRes {
        fb_id_ptr: u64,
        crtc_id_ptr: u64,
        connector_id_ptr: u64,
        encoder_id_ptr: u64,
        count_fbs: u32,
        count_crtcs: u32,
        count_connectors: u32,
        count_encoders: u32,
        min_width: u32,
        max_width: u32,
        min_height: u32,
        max_height: u32,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct ModeInfo {
        clock: u32,
        hdisplay: u16,
        hsync_start: u16,
        hsync_end: u16,
        htotal: u16,
        hskew: u16,
        vdisplay: u16,
        vsync_start: u16,
        vsync_end: u16,
        vtotal: u16,
        vscan: u16,
        vrefresh: u32,
        flags: u32,
        typ: u32,
        name: [u8; 32],
    }
    #[repr(C)]
    #[derive(Default)]
    struct GetConnector {
        encoders_ptr: u64,
        modes_ptr: u64,
        props_ptr: u64,
        prop_values_ptr: u64,
        count_modes: u32,
        count_props: u32,
        count_encoders: u32,
        encoder_id: u32,
        connector_id: u32,
        connector_type: u32,
        connector_type_id: u32,
        connection: u32,
        mm_width: u32,
        mm_height: u32,
        subpixel: u32,
        pad: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct GetEncoder {
        encoder_id: u32,
        encoder_type: u32,
        crtc_id: u32,
        possible_crtcs: u32,
        possible_clones: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Crtc {
        set_connectors_ptr: u64,
        count_connectors: u32,
        crtc_id: u32,
        fb_id: u32,
        x: u32,
        y: u32,
        gamma_size: u32,
        mode_valid: u32,
        mode: ModeInfo,
    }
    #[repr(C)]
    #[derive(Default)]
    struct CreateDumb {
        height: u32,
        width: u32,
        bpp: u32,
        flags: u32,
        handle: u32,
        pitch: u32,
        size: u64,
    }
    #[repr(C)]
    #[derive(Default)]
    struct MapDumb {
        handle: u32,
        pad: u32,
        offset: u64,
    }
    #[repr(C)]
    #[derive(Default)]
    struct FbCmd {
        fb_id: u32,
        width: u32,
        height: u32,
        pitch: u32,
        bpp: u32,
        depth: u32,
        handle: u32,
    }

    const GETRESOURCES: c_ulong = iowr(0xA0, size_of::<CardRes>());
    const SETCRTC: c_ulong = iowr(0xA2, size_of::<Crtc>());
    const GETCRTC: c_ulong = iowr(0xA1, size_of::<Crtc>());
    /// `_IO('d', 0x1f)`: no payload.
    const DROP_MASTER: c_ulong = (0x64 << 8) | 0x1f;
    const GETENCODER: c_ulong = iowr(0xA6, size_of::<GetEncoder>());
    const GETCONNECTOR: c_ulong = iowr(0xA7, size_of::<GetConnector>());
    const ADDFB: c_ulong = iowr(0xAE, size_of::<FbCmd>());
    const RMFB: c_ulong = iowr(0xAF, size_of::<u32>());
    const CREATE_DUMB: c_ulong = iowr(0xB2, size_of::<CreateDumb>());
    const MAP_DUMB: c_ulong = iowr(0xB3, size_of::<MapDumb>());
    const DESTROY_DUMB: c_ulong = iowr(0xB4, size_of::<u32>());
    const CONNECTED: u32 = 1;
    const TYPE_PREFERRED: u32 = 1 << 3;

    fn call<T>(fd: c_int, req: c_ulong, arg: &mut T, what: &str) -> io::Result<()> {
        // SAFETY: `arg` is a live repr(C) struct matching the request's size; the kernel reads
        // and writes only within it and within the user arrays it points at (sized by us).
        let rc = unsafe { ioctl(fd, req, arg as *mut T) };
        if rc == 0 {
            Ok(())
        } else {
            let e = io::Error::last_os_error();
            Err(io::Error::new(e.kind(), format!("{what}: {e}")))
        }
    }

    pub struct Display {
        file: std::fs::File,
        connector_id: u32,
        crtc_id: u32,
        mode: ModeInfo,
        handle: u32,
        fb_id: u32,
        map: *mut u8,
        size: usize,
        pub pitch: u32,
        pub mode_size: Mode,
    }

    // SAFETY: the mapping and the fd belong to the process; the splash moves the Display
    // behind a Mutex into its animation thread and never shares it unguarded.
    unsafe impl Send for Display {}

    impl Display {
        pub fn open(path: &str) -> io::Result<Display> {
            let file = OpenOptions::new().read(true).write(true).open(path)?;
            let fd = file.as_raw_fd();
            let mut res = CardRes::default();
            call(fd, GETRESOURCES, &mut res, "GETRESOURCES")?;
            let mut connectors = vec![0u32; res.count_connectors as usize];
            let mut crtcs = vec![0u32; res.count_crtcs as usize];
            res.connector_id_ptr = connectors.as_mut_ptr() as u64;
            res.crtc_id_ptr = crtcs.as_mut_ptr() as u64;
            res.count_fbs = 0;
            res.count_encoders = 0;
            call(fd, GETRESOURCES, &mut res, "GETRESOURCES")?;
            let mut found = None;
            for &id in &connectors {
                let mut c = GetConnector {
                    connector_id: id,
                    ..GetConnector::default()
                };
                call(fd, GETCONNECTOR, &mut c, "GETCONNECTOR")?;
                if c.connection != CONNECTED || c.count_modes == 0 {
                    continue;
                }
                let mut modes = vec![ModeInfo::default(); c.count_modes as usize];
                c.modes_ptr = modes.as_mut_ptr() as u64;
                c.count_props = 0;
                c.count_encoders = 0;
                call(fd, GETCONNECTOR, &mut c, "GETCONNECTOR")?;
                let mode = *modes
                    .iter()
                    .find(|m| m.typ & TYPE_PREFERRED != 0)
                    .unwrap_or(&modes[0]);
                let mut crtc_id = 0;
                if c.encoder_id != 0 {
                    let mut e = GetEncoder {
                        encoder_id: c.encoder_id,
                        ..GetEncoder::default()
                    };
                    if call(fd, GETENCODER, &mut e, "GETENCODER").is_ok() {
                        crtc_id = e.crtc_id;
                    }
                }
                if crtc_id == 0 {
                    crtc_id = *crtcs.first().ok_or_else(|| io::Error::other("no CRTC"))?;
                }
                found = Some((id, crtc_id, mode));
                break;
            }
            let (connector_id, crtc_id, mode) =
                found.ok_or_else(|| io::Error::other("no connected DRM connector"))?;
            let (w, h) = (mode.hdisplay as u32, mode.vdisplay as u32);
            let mut dumb = CreateDumb {
                height: h,
                width: w,
                bpp: 32,
                ..CreateDumb::default()
            };
            call(fd, CREATE_DUMB, &mut dumb, "CREATE_DUMB")?;
            let mut fb = FbCmd {
                width: w,
                height: h,
                pitch: dumb.pitch,
                bpp: 32,
                depth: 24,
                handle: dumb.handle,
                ..FbCmd::default()
            };
            call(fd, ADDFB, &mut fb, "ADDFB")?;
            let mut mapreq = MapDumb {
                handle: dumb.handle,
                ..MapDumb::default()
            };
            call(fd, MAP_DUMB, &mut mapreq, "MAP_DUMB")?;
            let size = dumb.size as usize;
            // SAFETY: mapping `size` bytes of the dumb buffer the kernel just handed us,
            // PROT_READ|PROT_WRITE (3), MAP_SHARED (1).
            let map = unsafe { mmap(std::ptr::null_mut(), size, 3, 1, fd, mapreq.offset as i64) };
            if map as isize == -1 {
                return Err(io::Error::other("mmap dumb buffer failed"));
            }
            Ok(Display {
                file,
                connector_id,
                crtc_id,
                mode,
                handle: dumb.handle,
                fb_id: fb.fb_id,
                map: map as *mut u8,
                size,
                pitch: dumb.pitch,
                mode_size: Mode {
                    width: w,
                    height: h,
                },
            })
        }

        /// Copy a packed XRGB8888 canvas (width*height) into the pitched buffer.
        pub fn write(&mut self, px: &[u32]) {
            let w = self.mode_size.width as usize;
            // chunks_exact: a short last row is skipped rather than read past the slice.
            for (y, row) in px.chunks_exact(w).enumerate() {
                let off = y * self.pitch as usize;
                if off + w * 4 > self.size {
                    break;
                }
                // SAFETY: off + row bytes is within the mapping; u32 -> bytes copy.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        row.as_ptr() as *const u8,
                        self.map.add(off),
                        w * 4,
                    );
                }
            }
        }

        /// Point the CRTC at our buffer (one SETCRTC; the buffer stays scanned out).
        pub fn commit(&mut self) -> io::Result<()> {
            let mut conn = [self.connector_id];
            let mut crtc = Crtc {
                set_connectors_ptr: conn.as_mut_ptr() as u64,
                count_connectors: 1,
                crtc_id: self.crtc_id,
                fb_id: self.fb_id,
                mode_valid: 1,
                mode: self.mode,
                ..Crtc::default()
            };
            call(self.file.as_raw_fd(), SETCRTC, &mut crtc, "SETCRTC")
        }

        /// Give up DRM master but keep the buffer scanned out, so the next client (cog) gets
        /// master on open and replaces the picture with its first frame: no dark gap.
        pub fn drop_master(&mut self) -> io::Result<()> {
            let mut none = 0u32;
            call(self.file.as_raw_fd(), DROP_MASTER, &mut none, "DROP_MASTER")
        }

        /// Is the CRTC still showing our buffer? False once another client set its own.
        pub fn shown(&self) -> bool {
            let mut crtc = Crtc {
                crtc_id: self.crtc_id,
                ..Crtc::default()
            };
            call(self.file.as_raw_fd(), GETCRTC, &mut crtc, "GETCRTC").is_ok()
                && crtc.fb_id == self.fb_id
        }
    }

    impl Drop for Display {
        fn drop(&mut self) {
            let fd = self.file.as_raw_fd();
            // SAFETY: tearing down exactly what `open` created.
            unsafe {
                munmap(self.map as *mut c_void, self.size);
            }
            let mut fb = self.fb_id;
            let _ = call(fd, RMFB, &mut fb, "RMFB");
            let mut h = self.handle;
            let _ = call(fd, DESTROY_DUMB, &mut h, "DESTROY_DUMB");
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn uapi_layouts() {
            assert_eq!(size_of::<CardRes>(), 64);
            assert_eq!(size_of::<ModeInfo>(), 68);
            assert_eq!(size_of::<GetConnector>(), 80);
            assert_eq!(size_of::<GetEncoder>(), 20);
            assert_eq!(size_of::<Crtc>(), 104);
            assert_eq!(size_of::<CreateDumb>(), 32);
            assert_eq!(size_of::<MapDumb>(), 16);
            assert_eq!(size_of::<FbCmd>(), 28);
            // Values from a 64-bit libdrm build; identical on armv7 since the structs are.
            assert_eq!(GETRESOURCES, 0xC04064A0);
            assert_eq!(CREATE_DUMB, 0xC02064B2);
            assert_eq!(SETCRTC, 0xC06864A2);
            assert_eq!(GETCRTC, 0xC06864A1);
            assert_eq!(DROP_MASTER, 0x641F);
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::Display;

#[cfg(not(target_os = "linux"))]
pub struct Display {
    pub mode_size: Mode,
}

#[cfg(not(target_os = "linux"))]
impl Display {
    pub fn open(_path: &str) -> io::Result<Display> {
        Err(io::Error::other("no DRM on this platform"))
    }
    pub fn write(&mut self, _px: &[u32]) {}
    pub fn commit(&mut self) -> io::Result<()> {
        Ok(())
    }
    pub fn drop_master(&mut self) -> io::Result<()> {
        Ok(())
    }
    pub fn shown(&self) -> bool {
        false
    }
}
