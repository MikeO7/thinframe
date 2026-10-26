//! Screenshot: exactly what the panel shows, for the server (`screenshot` action, so a page
//! designer over MCP sees the result) and for debugging (`tfctl screenshot FILE`, a P6 PPM).
//! Read from the scanned-out buffer through DRM (drm::capture, no GPU readback), then turned
//! back into the page orientation with COG_ROTATION, the inverse of cog's own mapping
//! (image/buildroot/patches/cog/0002-drm-rotate-touch-coordinates.patch).
use crate::config::Config;
use crate::drm;
use crate::http::{self, Url};
use std::io;
use std::time::{Duration, Instant};

/// Packed RGB, rows top to bottom, page orientation (what a person sees).
pub struct Shot {
    pub w: usize,
    pub h: usize,
    pub rgb: Vec<u8>,
}

/// The first DRM card with a CRTC that scans out a framebuffer (card1 on the A33; card0 is
/// lima, which has no CRTCs). Returns the shot and the card used.
pub fn capture(rotation: u8) -> io::Result<(Shot, String)> {
    let mut why = Vec::new();
    for n in 0..8 {
        let path = format!("/dev/dri/card{n}");
        if !std::path::Path::new(&path).exists() {
            continue;
        }
        match drm::capture(&path) {
            Ok((m, px)) => {
                let shot = to_page(&px, m.width as usize, m.height as usize, rotation);
                return Ok((shot, path));
            }
            Err(e) => why.push(format!("{path}: {e}")),
        }
    }
    if why.is_empty() {
        return Err(io::Error::other("no /dev/dri/card*"));
    }
    Err(io::Error::other(why.join("; ")))
}

/// Panel XRGB8888 (w x h) to page RGB. cog maps panel point (px, py) to page point
/// 1: (h-1-py, px), 2: (w-1-px, h-1-py), 3: (py, w-1-px), 0: unchanged; so page (x, y) reads
/// the panel at the inverse.
fn to_page(px: &[u32], w: usize, h: usize, rotation: u8) -> Shot {
    let (pw, ph) = if rotation % 2 == 1 { (h, w) } else { (w, h) };
    let mut rgb = Vec::with_capacity(pw * ph * 3);
    for y in 0..ph {
        for x in 0..pw {
            let (sx, sy) = match rotation {
                1 => (y, h - 1 - x),
                2 => (w - 1 - x, h - 1 - y),
                3 => (w - 1 - y, x),
                _ => (x, y),
            };
            let p = px[sy * w + sx];
            rgb.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
        }
    }
    Shot { w: pw, h: ph, rgb }
}

/// Binary PPM (P6).
fn ppm(s: &Shot) -> Vec<u8> {
    let mut out = format!("P6\n{} {}\n255\n", s.w, s.h).into_bytes();
    out.extend_from_slice(&s.rgb);
    out
}

/// POST {SERVER_URL}/api/frames/{FRAME_ID}/screenshot?w=W&h=H, body W*H*3 bytes of RGB.
pub fn upload(base: &Url, cfg: &Config, s: &Shot) -> io::Result<()> {
    let path = format!(
        "/api/frames/{}/screenshot?w={}&h={}",
        cfg.frame_id, s.w, s.h
    );
    let r = http::post_bytes(
        &base.with_path(&path),
        &cfg.token,
        "application/octet-stream",
        &s.rgb,
        Duration::from_secs(30),
    )?;
    if r.status != 200 && r.status != 201 && r.status != 204 {
        return Err(io::Error::other(format!(
            "POST screenshot: HTTP {}",
            r.status
        )));
    }
    Ok(())
}

/// The `screenshot` action, in its own thread: capture, upload, and a one-line outcome.
pub fn capture_and_upload(base: &Url, cfg: &Config) -> io::Result<String> {
    let t = Instant::now();
    let (s, _) = capture(cfg.rotation)?;
    let took = t.elapsed().as_millis();
    let t = Instant::now();
    upload(base, cfg, &s)?;
    Ok(format!(
        "{}x{}, {} bytes, capture {took} ms, upload {} ms",
        s.w,
        s.h,
        s.rgb.len(),
        t.elapsed().as_millis()
    ))
}

/// `tfctl screenshot FILE` (root).
pub fn run(cfg: &Config, file: &str) -> io::Result<()> {
    let t = Instant::now();
    let (s, card) = capture(cfg.rotation)?;
    let took = t.elapsed().as_millis();
    std::fs::write(file, ppm(&s))?;
    eprintln!(
        "tfctl: screenshot: {}x{} from {card} (rotation {}) in {took} ms, wrote {file}",
        s.w, s.h, cfg.rotation
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// cog's touch mapping, copied from the patch: panel (px, py) -> page point.
    fn cog(rotation: u8, w: usize, h: usize, px: usize, py: usize) -> (usize, usize) {
        match rotation {
            1 => (h - 1 - py, px),
            2 => (w - 1 - px, h - 1 - py),
            3 => (py, w - 1 - px),
            _ => (px, py),
        }
    }

    #[test]
    fn page_matches_cog_for_every_rotation() {
        let (w, h) = (3, 5);
        // Distinct colour per panel pixel: R = x, G = y, B = 0x5a.
        let panel: Vec<u32> = (0..w * h)
            .map(|i| (((i % w) as u32) << 16) | (((i / w) as u32) << 8) | 0x5a)
            .collect();
        for rotation in 0..4 {
            let s = to_page(&panel, w, h, rotation);
            let want = if rotation % 2 == 1 { (h, w) } else { (w, h) };
            assert_eq!((s.w, s.h), want, "rotation {rotation}");
            assert_eq!(s.rgb.len(), w * h * 3);
            for py in 0..h {
                for px in 0..w {
                    let (x, y) = cog(rotation, w, h, px, py);
                    let i = (y * s.w + x) * 3;
                    assert_eq!(
                        &s.rgb[i..i + 3],
                        &[px as u8, py as u8, 0x5a],
                        "rotation {rotation} panel ({px},{py})"
                    );
                }
            }
        }
    }

    #[test]
    fn rotation_3_reads_the_panel_column_by_column() {
        // Panel 2x3 at rotation 3 is a 3x2 page; page (x, y) = panel (w-1-y, x).
        let panel = [1, 2, 3, 4, 5, 6];
        let s = to_page(&panel, 2, 3, 3);
        assert_eq!((s.w, s.h), (3, 2));
        let blue: Vec<u8> = s.rgb.chunks(3).map(|p| p[2]).collect();
        assert_eq!(blue, [2, 4, 6, 1, 3, 5]);
        assert!(ppm(&s).starts_with(b"P6\n3 2\n255\n"));
    }
}
