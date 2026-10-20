//! Boot splash (TASK 15, minimal): background colour + one line of text, drawn to a DRM dumb
//! buffer (drm.rs) with the embedded 8x16 font scaled 4x. `COG_ROTATION` quarter turns are
//! applied so the text reads upright on the mounted frame ([U] same sign as cog's rotation;
//! if it comes out upside down on hardware, flip `rotate`). The PNG logo from TASK 15 is a
//! follow-up: this file has no decoder yet.
//!
//! `tfctl splash [TEXT]` draws and then holds the buffer until SIGTERM; the kiosk draws it in
//! process and drops it the moment cog is spawned.
use crate::config::Config;
use crate::{drm, font, png, sys};
use std::io;
use std::time::Duration;

pub const DEVICE: &str = "/dev/dri/card0";
pub const SCALE: usize = 4;
pub const TEXT_COLOR: u32 = 0xdfe3e8;

/// Packed XRGB8888 pixels, row-major, physical panel orientation.
pub struct Canvas {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u32>,
}

impl Canvas {
    pub fn new(w: usize, h: usize, bg: u32) -> Canvas {
        Canvas {
            w,
            h,
            px: vec![bg & 0xff_ffff; w * h],
        }
    }

    /// Logical (upright) size after `rotation` quarter turns.
    pub fn logical(&self, rotation: u8) -> (usize, usize) {
        if rotation % 2 == 1 {
            (self.h, self.w)
        } else {
            (self.w, self.h)
        }
    }

    /// Logical point -> physical pixel for `rotation` quarter turns clockwise.
    pub fn rotate(&self, rotation: u8, lx: usize, ly: usize) -> (usize, usize) {
        match rotation % 4 {
            0 => (lx, ly),
            1 => (self.w - 1 - ly, lx),
            2 => (self.w - 1 - lx, self.h - 1 - ly),
            _ => (ly, self.h - 1 - lx),
        }
    }

    pub fn set(&mut self, x: usize, y: usize, color: u32) {
        if x < self.w && y < self.h {
            self.px[y * self.w + x] = color & 0xff_ffff;
        }
    }

    /// One line, centred in the logical (rotated) frame. Longer than the width: clipped.
    pub fn text_centered(&mut self, text: &str, scale: usize, color: u32, rotation: u8) {
        self.text_centered_at(text, scale, color, rotation, 0);
    }

    /// Same, `dy` pixels below the centre line.
    pub fn text_centered_at(
        &mut self,
        text: &str,
        scale: usize,
        color: u32,
        rotation: u8,
        dy: isize,
    ) {
        let (lw, lh) = self.logical(rotation);
        let chars: Vec<char> = text.chars().collect();
        let (gw, gh) = (font::WIDTH * scale, font::HEIGHT * scale);
        let text_w = chars.len() * gw;
        let x0 = lw.saturating_sub(text_w) / 2;
        let y0 = ((lh.saturating_sub(gh) / 2) as isize + dy).max(0) as usize;
        for (i, c) in chars.iter().enumerate() {
            let g = font::glyph(*c);
            for (row, bits) in g.iter().enumerate() {
                for col in 0..font::WIDTH {
                    if bits & (0x80 >> col) == 0 {
                        continue;
                    }
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let lx = x0 + i * gw + col * scale + dx;
                            let ly = y0 + row * scale + dy;
                            if lx < lw && ly < lh {
                                let (x, y) = self.rotate(rotation, lx, ly);
                                self.set(x, y, color);
                            }
                        }
                    }
                }
            }
        }
    }
}

impl Canvas {
    /// Alpha-blend `img` centred in the logical frame, `dy` pixels below centre.
    pub fn blit_centered(&mut self, img: &png::Image, rotation: u8, dy: isize) {
        let (lw, lh) = self.logical(rotation);
        let x0 = (lw as isize - img.w as isize) / 2;
        let y0 = (lh as isize - img.h as isize) / 2 + dy;
        for iy in 0..img.h {
            for ix in 0..img.w {
                let (lx, ly) = (x0 + ix as isize, y0 + iy as isize);
                if lx < 0 || ly < 0 || lx as usize >= lw || ly as usize >= lh {
                    continue;
                }
                let p = img.px[iy * img.w + ix];
                let a = p >> 24;
                if a == 0 {
                    continue;
                }
                let (x, y) = self.rotate(rotation, lx as usize, ly as usize);
                let bg = self.px[y * self.w + x];
                let mix = |sh: u32| {
                    let (f, b) = ((p >> sh) & 0xff, (bg >> sh) & 0xff);
                    (f * a + b * (255 - a)) / 255
                };
                self.set(x, y, mix(16) << 16 | mix(8) << 8 | mix(0));
            }
        }
    }
}

pub const LOGO_PATH: &str = "/etc/thinframe/splash.png";

/// The logo, if one is installed and decodes; a bad file is logged and ignored.
fn load_logo() -> Option<png::Image> {
    let path = std::env::var("TFCTL_SPLASH_PNG").unwrap_or_else(|_| LOGO_PATH.into());
    let bytes = std::fs::read(&path).ok()?;
    match png::decode(&bytes) {
        Ok(i) => Some(i),
        Err(e) => {
            eprintln!("tfctl: splash: {path}: {e}");
            None
        }
    }
}

/// A live splash: dropping it releases the DRM device for cog.
pub struct Splash {
    _display: drm::Display,
}

pub fn show(bg: u32, text: &str, rotation: u8) -> io::Result<Splash> {
    let mut d = drm::Display::open(DEVICE)?;
    let (w, h) = (d.mode_size.width as usize, d.mode_size.height as usize);
    let mut c = Canvas::new(w, h, bg);
    match load_logo() {
        Some(logo) => {
            c.blit_centered(&logo, rotation, -((font::HEIGHT * SCALE) as isize));
            c.text_centered_at(
                text,
                SCALE,
                TEXT_COLOR,
                rotation,
                (logo.h / 2 + font::HEIGHT * SCALE) as isize,
            );
        }
        None => c.text_centered(text, SCALE, TEXT_COLOR, rotation),
    }
    d.write(&c.px);
    d.commit()?;
    eprintln!("tfctl: splash: {w}x{h} rotation {rotation}: {text:?}");
    Ok(Splash { _display: d })
}

/// `tfctl splash [TEXT]`: draw, then hold until SIGTERM/SIGINT.
pub fn run(cfg: &Config, text: Option<&str>) -> io::Result<()> {
    sys::trap_term();
    let _s = show(
        cfg.splash_bg,
        text.unwrap_or(&cfg.splash_text),
        cfg.rotation,
    )?;
    while !sys::term_requested() {
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(c: &Canvas, bg: u32) -> usize {
        c.px.iter().filter(|p| **p != bg).count()
    }

    fn glyph_pixels(text: &str) -> usize {
        text.chars()
            .map(|ch| {
                font::glyph(ch)
                    .iter()
                    .map(|r| r.count_ones() as usize)
                    .sum::<usize>()
            })
            .sum()
    }

    #[test]
    fn centred_text_every_rotation() {
        let bg = 0x0b0d10;
        for rot in 0..4u8 {
            let mut c = Canvas::new(800, 1280, bg);
            c.text_centered("thinframe", 4, TEXT_COLOR, rot);
            assert_eq!(
                lit(&c, bg),
                glyph_pixels("thinframe") * 16,
                "rotation {rot}"
            );
            // Bounding box is centred in the logical frame.
            let (lw, lh) = c.logical(rot);
            let (mut minx, mut maxx, mut miny, mut maxy) = (usize::MAX, 0, usize::MAX, 0);
            for ly in 0..lh {
                for lx in 0..lw {
                    let (x, y) = c.rotate(rot, lx, ly);
                    if c.px[y * c.w + x] != bg {
                        minx = minx.min(lx);
                        maxx = maxx.max(lx);
                        miny = miny.min(ly);
                        maxy = maxy.max(ly);
                    }
                }
            }
            assert!(
                (minx as i64 - (lw - 1 - maxx) as i64).abs() <= 8,
                "rotation {rot} x"
            );
            assert!(
                (miny as i64 - (lh - 1 - maxy) as i64).abs() <= 8,
                "rotation {rot} y"
            );
            assert!(maxx - minx < 9 * 32 && maxy - miny < 64);
        }
    }

    #[test]
    fn logo_blends_centred_with_alpha_in_every_rotation() {
        let img = png::Image {
            w: 2,
            h: 2,
            px: vec![0xff_ff0000, 0x00_00ff00, 0x80_0000ff, 0xff_ffffff],
        };
        for rot in 0..4u8 {
            let mut c = Canvas::new(10, 10, 0);
            c.blit_centered(&img, rot, 0);
            assert_eq!(
                c.px.iter().filter(|p| **p != 0).count(),
                3,
                "rotation {rot}"
            );
            let (lw, lh) = c.logical(rot);
            let (x, y) = c.rotate(rot, (lw - 2) / 2, (lh - 2) / 2);
            assert_eq!(
                c.px[y * c.w + x],
                0xff0000,
                "opaque red at top-left, rotation {rot}"
            );
        }
        let mut c = Canvas::new(10, 10, 0);
        c.blit_centered(&img, 0, 0);
        assert_eq!(c.px[5 * 10 + 4], 0x00_00_80, "half-alpha blue over black");
    }

    #[test]
    fn rotation_maps_corners() {
        let c = Canvas::new(10, 20, 0);
        assert_eq!(c.logical(1), (20, 10));
        assert_eq!(c.rotate(0, 0, 0), (0, 0));
        assert_eq!(c.rotate(1, 0, 0), (9, 0));
        assert_eq!(c.rotate(2, 0, 0), (9, 19));
        assert_eq!(c.rotate(3, 0, 0), (0, 19));
        assert_eq!(c.rotate(1, 19, 9), (0, 19));
        let mut c = Canvas::new(4, 4, 0);
        c.set(9, 9, 1);
        assert!(c.px.iter().all(|p| *p == 0));
    }

    #[test]
    fn long_text_is_clipped_not_panicking() {
        let mut c = Canvas::new(64, 16, 0);
        c.text_centered("this is far too long for the panel", 4, 0xffffff, 0);
        assert!(lit(&c, 0) > 0);
    }

    #[test]
    fn show_fails_cleanly_without_drm() {
        if std::path::Path::new(DEVICE).exists() {
            return;
        }
        assert!(show(0, "x", 0).is_err());
    }
}
