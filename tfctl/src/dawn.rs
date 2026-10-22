//! The "dawn" splash (SPLASH_STYLE=dawn, the default): a sunrise of peach, rose and lilac behind
//! one frosted glass sheet, the title in a thin sans (sdf.rs, Poppins ExtraLight), the boot status
//! with a pulsing dot, and the frame's address in the corner. Picked from five browser mockups
//! on 2026-10-21.
//!
//! No real blur: the scene behind the glass is all soft gradients, so frosting is the white tint
//! alone. Built once into layers (background without the sun, shadow and glass alphas, ink
//! alpha). A frame only moves the sun a little, so it is recomposed inside the sun's box and
//! copied from a still everywhere else.
use crate::sdf;
use std::f32::consts::PI;

const INK: [u8; 3] = [0x2b, 0x21, 0x40];
const WHITE: [u8; 3] = [255, 255, 255];
const SHADOW: [u8; 3] = [0x50, 0x32, 0x78];
/// 160deg sunrise: peach, rose, lilac, sky.
const SKY: [(f32, [f32; 3]); 4] = [
    (0.0, [255.0, 214.0, 189.0]),
    (0.38, [246.0, 169.0, 196.0]),
    (0.72, [185.0, 162.0, 241.0]),
    (1.0, [134.0, 180.0, 246.0]),
];
/// Milliseconds for one drift of the sun.
const DRIFT_MS: f32 = 12_000.0;
const TABLE: usize = 256;

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn sky_at(s: f32) -> [f32; 3] {
    let s = s.clamp(0.0, 1.0);
    for w in SKY.windows(2) {
        if s <= w[1].0 {
            let t = (s - w[0].0) / (w[1].0 - w[0].0);
            return [0, 1, 2].map(|c| lerp(w[0].1[c], w[1].1[c], t));
        }
    }
    SKY[3].1
}

/// Position along a CSS-style gradient line of `deg` degrees over a w x h box, 0..1.
fn along(x: f32, y: f32, w: f32, h: f32, deg: f32) -> f32 {
    let (dx, dy) = (deg.to_radians().sin(), -deg.to_radians().cos());
    let len = (w * dx).abs() + (h * dy).abs();
    ((x - w / 2.0) * dx + (y - h / 2.0) * dy) / len + 0.5
}

/// Signed distance to a rounded rectangle (negative inside).
fn rrect(x: f32, y: f32, (x0, y0, x1, y1): (f32, f32, f32, f32), r: f32) -> f32 {
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let (hx, hy) = ((x1 - x0) / 2.0 - r, (y1 - y0) / 2.0 - r);
    let (qx, qy) = ((x - cx).abs() - hx, (y - cy).abs() - hy);
    (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r
}

fn blend(a: [u8; 3], b: [u8; 3], k: u32) -> [u8; 3] {
    // k 0..255
    [0, 1, 2].map(|c| ((a[c] as u32 * (255 - k) + b[c] as u32 * k + 127) / 255) as u8)
}

fn px([r, g, b]: [u8; 3]) -> u32 {
    (r as u32) << 16 | (g as u32) << 8 | b as u32
}

fn unpx(p: u32) -> [u8; 3] {
    [(p >> 16) as u8, (p >> 8) as u8, p as u8]
}

fn a8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

pub struct Scene {
    pub w: usize,
    pub h: usize,
    /// Sky and blobs, no sun.
    bg: Vec<u32>,
    /// Drop shadow outside the sheet, then the glass (tint and rim), both 0..255.
    shadow: Vec<u8>,
    glass: Vec<u8>,
    ink: Vec<u8>,
    /// The scene with the sun at rest.
    still: Vec<u32>,
    /// The sun's glow by squared distance: colour, alpha.
    glow: [([u8; 3], u8); TABLE],
    sun: (f32, f32),
    radius: f32,
    /// Pixel box the sun can reach: x0, y0, x1, y1.
    reach: (usize, usize, usize, usize),
    dot: Option<(f32, f32, f32)>,
}

impl Scene {
    pub fn new(w: usize, h: usize, title: &str, status: &str, ip: Option<&str>) -> Scene {
        let u = (w as f32 / 1280.0).min(h as f32 / 800.0);
        let (wf, hf) = (w as f32, h as f32);
        let sheet = (56.0 * u, 56.0 * u, wf - 56.0 * u, hf - 56.0 * u);
        let radius = 48.0 * u;
        let n = w * h;
        // Blobs: centre, radius where they fade out, colour, strength.
        let blobs = [
            (
                (370.0 * u, hf - 160.0 * u),
                298.0 * u,
                [125.0, 143.0, 242.0],
                0.8,
            ),
            (
                (510.0 * u, 190.0 * u),
                125.0 * u,
                [255.0, 143.0, 174.0],
                0.8,
            ),
        ];
        let (mut bg, mut shadow, mut glass) = (vec![0u32; n], vec![0u8; n], vec![0u8; n]);
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                let mut c = sky_at(along(fx, fy, wf, hf, 160.0));
                for &((bx, by), br, col, k) in &blobs {
                    let a =
                        (1.0 - ((fx - bx).powi(2) + (fy - by).powi(2)).sqrt() / br).max(0.0) * k;
                    if a > 0.0 {
                        c = [0, 1, 2].map(|i| lerp(c[i], col[i], a));
                    }
                }
                let i = y * w + x;
                bg[i] = px(c.map(|v| v.round() as u8));
                // Glass: tint along 155deg, a rim one pixel wide, brighter on the top edge.
                let sd = rrect(fx, fy, sheet, radius);
                let body = (0.5 - sd).clamp(0.0, 1.0);
                if body > 0.0 {
                    let s = along(fx, fy, wf, hf, 155.0);
                    let tint = if s < 0.6 {
                        lerp(0.46, 0.18, s / 0.6)
                    } else {
                        lerp(0.18, 0.30, (s - 0.6) / 0.4)
                    };
                    let rim = (body - (0.5 - (sd + u.max(1.0))).clamp(0.0, 1.0)).max(0.0);
                    let top = 1.0 - ((fy - sheet.1) / (sheet.3 - sheet.1)).clamp(0.0, 1.0);
                    let rim_a = rim * (0.55 + 0.4 * top);
                    glass[i] = a8(1.0 - (1.0 - body * tint) * (1.0 - rim_a));
                } else {
                    // 0 40px 100px rgba(80,50,120,.28): a soft fall-off, lower than the sheet.
                    let d = rrect(fx, fy - 40.0 * u, sheet, radius) / (50.0 * u);
                    shadow[i] = a8(0.28 / (1.0 + (d * 1.6).exp()));
                }
            }
        }

        // Sun: centre and reach as in the mockup, drifting a few pixels.
        let sun = (wf - 426.0 * u, 224.0 * u);
        let reach_r = 273.0 * u;
        let mut glow = [([0u8; 3], 0u8); TABLE];
        for (k, g) in glow.iter_mut().enumerate() {
            let t = (k as f32 / (TABLE - 1) as f32).sqrt(); // table index is distance squared
            let (col, a) = if t < 0.542 {
                (
                    lerp_c([255.0, 247.0, 223.0], [255.0, 217.0, 160.0], t / 0.542),
                    1.0,
                )
            } else {
                let q = (t - 0.542) / 0.458;
                (
                    lerp_c([255.0, 217.0, 160.0], [255.0, 190.0, 150.0], q),
                    1.0 - q,
                )
            };
            *g = (col, a8(a));
        }
        let (bx, by) = (14.0 * u, 18.0 * u);
        let reach = (
            ((sun.0 - reach_r - bx).floor().max(0.0)) as usize,
            ((sun.1 - reach_r - by).floor().max(0.0)) as usize,
            (((sun.0 + reach_r + bx).ceil()) as usize + 1).min(w),
            (((sun.1 + reach_r + by).ceil()) as usize + 1).min(h),
        );

        // Ink: the mark and name, the address, the title, the status.
        let mut ink = vec![0u8; n];
        let mut put = |x: i32, y: i32, a: f32| {
            if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && a > 0.0 {
                let c = &mut ink[y as usize * w + x as usize];
                *c = (*c).max(a8(a));
            }
        };
        let (left, top) = (sheet.0 + 64.0 * u, sheet.1 + 48.0 * u);
        // An empty title is the page backdrop (Wi-Fi, PAIR): the sheet alone, no mark, no lettering.
        let bare = title.is_empty();
        // Mark: an outlined rounded square around a filled one.
        let m = 34.0 * u;
        let (mx, my) = (left, top);
        let (y0, y1) = (my.floor() as i32 - 2, (my + m).ceil() as i32 + 2);
        for y in y0..if bare { y0 } else { y1 } {
            for x in (mx.floor() as i32 - 2)..(mx + m).ceil() as i32 + 2 {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                let outer = rrect(fx, fy, (mx, my, mx + m, my + m), m * 0.25);
                let ring = (0.5 - outer).clamp(0.0, 1.0)
                    - (0.5 - (outer + 2.6 * m / 48.0 * 1.0)).clamp(0.0, 1.0);
                let (q0, q1) = (
                    m * 10.0 / 48.0 + m * 2.0 / 48.0,
                    m - m * 10.0 / 48.0 - m * 2.0 / 48.0,
                );
                let inner = rrect(fx, fy, (mx + q0, my + q0, mx + q1, my + q1), m * 0.125);
                put(x, y, ring.max((0.5 - inner).clamp(0.0, 1.0)));
            }
        }
        let mut dot = None;
        if let Some(f) = sdf::thin().filter(|_| !bare) {
            f.draw_bold(
                "thinframe",
                mx + m + 14.0 * u,
                my + m * 0.5 + 7.0 * u,
                20.0 * u,
                0.5 * u,
                &mut put,
            );
            if let Some(ip) = ip {
                let size = 16.0 * u;
                let wd = f.measure(ip, size);
                f.draw_bold(
                    ip,
                    sheet.2 - 64.0 * u - wd,
                    my + m * 0.5 + 5.0 * u,
                    size,
                    0.45 * u,
                    |x, y, a| put(x, y, a * 0.85),
                );
            }
            let room = sheet.2 - sheet.0 - 128.0 * u;
            let mut size = 176.0 * u;
            let tw = f.measure(title, size);
            if tw > room && tw > 0.0 {
                size *= room / tw;
            }
            let base = (sheet.1 + sheet.3) / 2.0 + 40.0 * u;
            f.draw_bold(title, left - 4.0 * u, base, size, 0.0, &mut put);
            if !status.is_empty() {
                let sy = base + 84.0 * u;
                let sz = 26.0 * u;
                let avail = room - 30.0 * u;
                let wd = f.measure(status, sz);
                let sz = if wd > avail && wd > 0.0 {
                    sz * avail / wd
                } else {
                    sz
                };
                dot = Some((left + 6.0 * u, sy - sz * 0.32, 6.0 * u));
                f.draw_bold(status, left + 30.0 * u, sy, sz, 0.5 * u, &mut put);
            }
        }
        let mut s = Scene {
            w,
            h,
            bg,
            shadow,
            glass,
            ink,
            still: vec![0; n],
            glow,
            sun,
            radius: reach_r,
            reach,
            dot,
        };
        for i in 0..n {
            s.still[i] = s.compose(i, None);
        }
        let (x0, y0, x1, y1) = s.reach;
        for y in y0..y1 {
            for x in x0..x1 {
                let i = y * w + x;
                s.still[i] = s.compose(i, s.sun_at(x, y, s.sun));
            }
        }
        s
    }

    fn sun_at(&self, x: usize, y: usize, c: (f32, f32)) -> Option<([u8; 3], u8)> {
        let d2 = (x as f32 + 0.5 - c.0).powi(2) + (y as f32 + 0.5 - c.1).powi(2);
        let r2 = self.radius * self.radius;
        (d2 < r2).then(|| self.glow[((d2 / r2) * (TABLE - 1) as f32) as usize])
    }

    /// Pixel `i`: sky, sun, shadow, glass, ink.
    fn compose(&self, i: usize, sun: Option<([u8; 3], u8)>) -> u32 {
        let mut c = unpx(self.bg[i]);
        if let Some((col, a)) = sun {
            c = blend(c, col, a as u32);
        }
        c = blend(c, SHADOW, self.shadow[i] as u32);
        c = blend(c, WHITE, self.glass[i] as u32);
        px(blend(c, INK, self.ink[i] as u32))
    }

    /// One frame at `t_ms` into `out` (w*h, upright).
    pub fn frame(&self, t_ms: u64, out: &mut [u32]) {
        out[..self.w * self.h].copy_from_slice(&self.still);
        let t = t_ms as f32 / DRIFT_MS * 2.0 * PI;
        let u = (self.w as f32 / 1280.0).min(self.h as f32 / 800.0);
        let c = (
            self.sun.0 + t.sin() * 14.0 * u,
            self.sun.1 + (t * 0.5).cos() * 18.0 * u - 18.0 * u,
        );
        let (x0, y0, x1, y1) = self.reach;
        for y in y0..y1 {
            for x in x0..x1 {
                let i = y * self.w + x;
                out[i] = self.compose(i, self.sun_at(x, y, c));
            }
        }
        // The status dot breathes.
        if let Some((dx, dy, r)) = self.dot {
            let a = 0.55 + 0.45 * ((t_ms as f32 / 380.0).sin() + 1.0) / 2.0;
            for y in ((dy - r - 1.0) as i32).max(0)..((dy + r + 2.0) as i32).min(self.h as i32) {
                for x in ((dx - r - 1.0) as i32).max(0)..((dx + r + 2.0) as i32).min(self.w as i32)
                {
                    let d = ((x as f32 + 0.5 - dx).powi(2) + (y as f32 + 0.5 - dy).powi(2)).sqrt();
                    let cov = (r - d + 0.5).clamp(0.0, 1.0) * a;
                    if cov > 0.0 {
                        let i = y as usize * self.w + x as usize;
                        out[i] = px(blend(unpx(out[i]), INK, a8(cov) as u32));
                    }
                }
            }
        }
    }
}

fn lerp_c(a: [f32; 3], b: [f32; 3], t: f32) -> [u8; 3] {
    [0, 1, 2].map(|c| lerp(a[c], b[c], t).round() as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luma(p: u32) -> u32 {
        let [r, g, b] = unpx(p);
        (r as u32 * 3 + g as u32 * 6 + b as u32) / 10
    }

    fn scene() -> (Scene, Vec<u32>) {
        let s = Scene::new(
            1280,
            800,
            "thinframe",
            "reaching http://nas:8080",
            Some("192.168.1.42"),
        );
        let mut px = vec![0; 1280 * 800];
        s.frame(0, &mut px);
        (s, px)
    }

    #[test]
    fn sunrise_sky_glass_and_ink_land_where_they_should() {
        let (_, px) = scene();
        let at = |x: usize, y: usize| px[y * 1280 + x];
        // Outside the sheet the corner is the peach end of the sky.
        let [r, g, b] = unpx(at(4, 4));
        assert!(r > 230 && g > 190 && b > 160, "{r} {g} {b}");
        // Inside the sheet the same row is frosted: lighter than the bare sky next to it.
        let (s, _) = scene();
        assert!(
            luma(at(200, 700)) > luma(s.bg[700 * 1280 + 200]),
            "glass tints the sky"
        );
        // The sky stays visible through the glass: top-left differs from bottom-right.
        assert_ne!(unpx(at(120, 120)), unpx(at(1160, 690)));
        // Ink: dark pixels for the title band, the status and the address.
        let dark = |x0: usize, y0: usize, x1: usize, y1: usize| {
            (y0..y1)
                .flat_map(|y| (x0..x1).map(move |x| (x, y)))
                .filter(|&(x, y)| luma(at(x, y)) < 110)
                .count()
        };
        assert!(dark(110, 280, 1150, 450) > 3000, "title");
        assert!(dark(140, 480, 700, 540) > 300, "status");
        assert!(dark(1000, 90, 1170, 130) > 100, "address");
        assert!(dark(120, 96, 160, 140) > 150, "mark");
    }

    #[test]
    fn the_sun_drifts_and_the_dot_breathes() {
        let (s, a) = scene();
        let mut b = vec![0; 1280 * 800];
        s.frame(3000, &mut b);
        let changed = a.iter().zip(&b).filter(|(x, y)| x != y).count();
        assert!(changed > 20_000, "sun moved: {changed}");
        // Far from the sun and the dot nothing moves (bottom-left corner of the sky).
        for y in 700..790 {
            for x in 5..60 {
                assert_eq!(a[y * 1280 + x], b[y * 1280 + x]);
            }
        }
    }

    #[test]
    fn an_empty_title_is_the_bare_sheet_for_pages() {
        let page = Scene::new(640, 400, "", "", None);
        let titled = Scene::new(640, 400, "thinframe", "", None);
        assert!(page.ink.iter().all(|&a| a == 0), "no lettering, no mark");
        assert!(titled.ink.iter().any(|&a| a > 200));
        assert_eq!(page.bg, titled.bg, "same sky");
        assert_eq!(page.glass, titled.glass, "same sheet");
    }

    #[test]
    fn backdrop_has_no_status_or_address() {
        let full = Scene::new(640, 400, "t", "loading", Some("10.0.0.1"));
        let bare = Scene::new(640, 400, "t", "", None);
        assert!(full.dot.is_some() && bare.dot.is_none());
        assert!(
            full.ink.iter().filter(|&&a| a > 200).count()
                > bare.ink.iter().filter(|&&a| a > 200).count()
        );
    }

    #[test]
    fn small_and_portrait_sizes_do_not_panic() {
        for (w, h) in [(1, 1), (16, 9), (800, 1280), (100, 30)] {
            let s = Scene::new(
                w,
                h,
                "a very long title that will not fit",
                "a long status line",
                Some("10.0.0.1"),
            );
            let mut px = vec![0; w * h];
            s.frame(12345, &mut px);
        }
    }
}

#[cfg(test)]
mod bench {
    #[test]
    #[ignore]
    fn timings() {
        let t = std::time::Instant::now();
        let s = super::Scene::new(1280, 800, "thinframe", "reaching", Some("192.168.1.20"));
        let built = t.elapsed();
        let mut px = vec![0; 1280 * 800];
        let t = std::time::Instant::now();
        for i in 0..10 {
            s.frame(i * 100, &mut px);
        }
        eprintln!("new {:?}, frame {:?}", built, t.elapsed() / 10);
    }
}
