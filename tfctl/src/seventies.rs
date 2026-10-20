//! The "seventies" splash (SPLASH_STYLE=seventies, the default; TASK 15): warm stripes curving
//! around a sun, sunburst rays that turn slowly, the title in Shrikhand (sdf.rs), the boot status
//! with three pulsing dots, and the frame's address. Picked from browser mockups on 2026-10-20.
//!
//! Animated cheaply: everything except the rays and the dots is drawn once into a static layer.
//! Each frame only works out which ray a background pixel falls in (angle and radius per pixel
//! are precomputed), blends the static layer over it and applies a fixed film grain.
use crate::sdf;
use std::f32::consts::PI;

const CREAM: u32 = 0xf4dcaa;
const RAY: u32 = 0xeecb8d;
const SUN: u32 = 0xf2c14e;
const BANDS: [u32; 4] = [0xeaa53b, 0xd9662b, 0xb8452a, 0x5b3a1e];
const BROWN: u32 = 0x5b3a1e;
const RUST: u32 = 0xb8452a;
const RAYS: f32 = 24.0;
/// Radians per millisecond: one ray width every ~6.5 s.
const SPIN: f32 = 0.00004;

fn rgb(c: u32) -> [f32; 3] {
    [
        (c >> 16 & 255) as f32,
        (c >> 8 & 255) as f32,
        (c & 255) as f32,
    ]
}

fn pack(c: [f32; 3]) -> u32 {
    let b = |v: f32| v.round().clamp(0.0, 255.0) as u32;
    b(c[0]) << 16 | b(c[1]) << 8 | b(c[2])
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Straight-alpha layer: what is drawn over the turning rays.
struct Layer {
    w: usize,
    h: usize,
    c: Vec<[f32; 3]>,
    a: Vec<f32>,
}

impl Layer {
    fn over(&mut self, x: i32, y: i32, color: u32, alpha: f32) {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h || alpha <= 0.0 {
            return;
        }
        let i = y as usize * self.w + x as usize;
        let (ca, aa) = (self.c[i], self.a[i]);
        let na = alpha + aa * (1.0 - alpha);
        let k = alpha / na;
        self.c[i] = mix(ca, rgb(color), k);
        self.a[i] = na;
    }

    fn text(&mut self, f: &sdf::Font, s: &str, x: f32, y: f32, px: f32, color: u32) {
        f.draw(s, x, y, px, |xx, yy, a| self.over(xx, yy, color, a));
    }
}

/// Everything the scene needs per pixel, built once for a size, title, status and address.
pub struct Scene {
    pub w: usize,
    pub h: usize,
    #[cfg_attr(not(test), allow(dead_code))]
    cx: f32,
    #[cfg_attr(not(test), allow(dead_code))]
    cy: f32,
    /// Final colour where the static layer is opaque (grain applied), else unused.
    solid: Vec<u32>,
    alpha: Vec<u8>,
    color: Vec<[u8; 3]>,
    /// Angle around the sun (0..65536 = 0..2π) and distance from it, for the rays.
    ang: Vec<u16>,
    rad: Vec<u16>,
    grain: Vec<u8>,
    dots: Vec<(f32, f32, f32)>,
}

fn hash(x: u32) -> u32 {
    let mut h = x.wrapping_mul(0x9e37_79b9);
    h ^= h >> 15;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^ h >> 13
}

impl Scene {
    pub fn new(w: usize, h: usize, title: &str, status: &str, ip: Option<&str>) -> Scene {
        let u = (w as f32 / 1280.0).min(h as f32 / 800.0);
        let (cx, cy) = (
            w as f32 - 380.0 * u,
            330.0 * u + (h as f32 - 800.0 * u) * 0.3,
        );
        let mut l = Layer {
            w,
            h,
            c: vec![[0.0; 3]; w * h],
            a: vec![0.0; w * h],
        };
        // Sun, then four bands: left of the sun straight, a quarter turn below-right, then up.
        let bw = 46.0 * u;
        for y in 0..h {
            for x in 0..w {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
                l.over(
                    x as i32,
                    y as i32,
                    SUN,
                    (104.0 * u - d + 0.5).clamp(0.0, 1.0),
                );
                for (i, &col) in BANDS.iter().enumerate() {
                    let r = (126.0 + i as f32 * 46.0 + 23.0) * u;
                    let off = if px <= cx {
                        (py - (cy + r)).abs()
                    } else if py >= cy {
                        (d - r).abs()
                    } else {
                        (px - (cx + r)).abs()
                    };
                    l.over(
                        x as i32,
                        y as i32,
                        col,
                        (bw / 2.0 + 0.5 - off).clamp(0.0, 1.0),
                    );
                }
            }
        }
        let mut dots = Vec::new();
        if let Some(f) = sdf::font() {
            let mut size = 156.0 * u;
            let room = cx - 104.0 * u - 90.0 * u;
            let tw = f.measure(title, size);
            if tw > room && tw > 0.0 {
                size *= room / tw;
            }
            let (tx, ty) = (70.0 * u, 300.0 * u);
            for (o, col) in [(12.0, BANDS[0]), (6.0, BANDS[1]), (0.0, BROWN)] {
                l.text(f, title, tx + o * u, ty + o * u, size, col);
            }
            if !status.is_empty() {
                let (sx, sy, sz) = (74.0 * u, 690.0 * u, 42.0 * u);
                l.text(f, status, sx, sy, sz, RUST);
                let dx = sx + f.measure(status, sz) + 16.0 * u;
                dots = (0..3)
                    .map(|i| (dx + i as f32 * 26.0 * u, sy - 12.0 * u, 7.0 * u))
                    .collect();
            }
            if let Some(ip) = ip {
                l.text(f, &format!("at {ip}"), 76.0 * u, 744.0 * u, 34.0 * u, BROWN);
            }
        }
        let n = w * h;
        let (mut solid, mut alpha, mut color) = (vec![0; n], vec![0; n], vec![[0u8; 3]; n]);
        let (mut ang, mut rad, mut grain) = (vec![0; n], vec![0; n], vec![0; n]);
        for i in 0..n {
            let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
            let th = (y - cy).atan2(x - cx).rem_euclid(2.0 * PI);
            ang[i] = (th / (2.0 * PI) * 65536.0) as u32 as u16;
            rad[i] = ((x - cx).hypot(y - cy)).min(65535.0) as u16;
            // 7% film grain, multiplied in (as in the mockup).
            grain[i] = 255 - (hash(i as u32) % 19) as u8;
            let a = l.a[i];
            alpha[i] = (a * 255.0).round() as u8;
            let c = l.c[i];
            color[i] = [c[0] as u8, c[1] as u8, c[2] as u8];
            if alpha[i] == 255 {
                solid[i] = apply_grain(pack(c), grain[i]);
            }
        }
        Scene {
            w,
            h,
            cx,
            cy,
            solid,
            alpha,
            color,
            ang,
            rad,
            grain,
            dots,
        }
    }

    /// One frame at `t_ms` into `out` (w*h, upright).
    pub fn frame(&self, t_ms: u64, out: &mut [u32]) {
        let rot = (t_ms as f32 * SPIN).rem_euclid(2.0 * PI);
        let rot16 = (rot / (2.0 * PI) * 65536.0) as u32;
        let (cream, ray) = (rgb(CREAM), rgb(RAY));
        let wedge = 2.0 * PI / RAYS;
        for (i, o) in out.iter_mut().enumerate().take(self.w * self.h) {
            let a = self.alpha[i];
            if a == 255 {
                *o = self.solid[i];
                continue;
            }
            let pos = ((self.ang[i] as u32 + 65536 - rot16) & 0xffff) * RAYS as u32;
            let frac = (pos & 0xffff) as f32 / 65536.0;
            let edge = frac.min(1.0 - frac) * wedge * self.rad[i] as f32;
            let k = (0.5 + edge).min(1.0);
            let k = if (pos >> 16).is_multiple_of(2) {
                k
            } else {
                1.0 - k
            };
            let mut c = mix(cream, ray, k);
            if a > 0 {
                let s = self.color[i];
                c = mix(c, [s[0] as f32, s[1] as f32, s[2] as f32], a as f32 / 255.0);
            }
            *o = apply_grain(pack(c), self.grain[i]);
        }
        // Three dots after the status that pulse one after another.
        for (n, &(dx, dy, r)) in self.dots.iter().enumerate() {
            let k = ((t_ms as f32 / 300.0 - n as f32 * 0.9).sin() + 1.0) / 2.0;
            let a = 0.25 + 0.75 * k;
            let (x0, x1) = ((dx - r - 1.0) as i32, (dx + r + 2.0) as i32);
            let (y0, y1) = ((dy - r - 1.0) as i32, (dy + r + 2.0) as i32);
            for y in y0.max(0)..y1.min(self.h as i32) {
                for x in x0.max(0)..x1.min(self.w as i32) {
                    let d = ((x as f32 + 0.5 - dx).powi(2) + (y as f32 + 0.5 - dy).powi(2)).sqrt();
                    let cov = (r - d + 0.5).clamp(0.0, 1.0) * a;
                    if cov > 0.0 {
                        let i = y as usize * self.w + x as usize;
                        out[i] = pack(mix(rgb(out[i]), rgb(BROWN), cov));
                    }
                }
            }
        }
    }

    /// The sun's centre, for tests.
    #[cfg(test)]
    pub fn sun(&self) -> (f32, f32) {
        (self.cx, self.cy)
    }
}

fn apply_grain(c: u32, g: u8) -> u32 {
    let g = g as u32;
    ((c >> 16 & 255) * g / 255) << 16 | ((c >> 8 & 255) * g / 255) << 8 | ((c & 255) * g / 255)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: u32, b: u32, tol: i32) -> bool {
        (0..3).all(|s| ((a >> (s * 8) & 255) as i32 - (b >> (s * 8) & 255) as i32).abs() <= tol)
    }

    #[test]
    fn layout_and_colours() {
        let s = Scene::new(1280, 800, "thinframe", "warming up", Some("192.168.1.20"));
        let mut px = vec![0; 1280 * 800];
        s.frame(0, &mut px);
        let at = |x: usize, y: usize| px[y * 1280 + x];
        let (cx, cy) = s.sun();
        assert!(
            near(at(cx as usize, cy as usize), SUN, 20),
            "sun in the middle of the arc"
        );
        // The outer band runs straight along the left at cy + 274.
        assert!(near(
            at(40, (cy + 126.0 + 3.0 * 46.0 + 23.0) as usize),
            BANDS[3],
            20
        ));
        assert!(near(at(40, (cy + 126.0 + 23.0) as usize), BANDS[0], 20));
        // Top-left corner: cream or ray, never dark.
        let c = at(5, 5);
        assert!(near(c, CREAM, 25) || near(c, RAY, 25), "{c:06x}");
        // Title, status and address put dark ink where they should.
        let dark = |x0: usize, y0: usize, x1: usize, y1: usize| {
            (y0..y1)
                .flat_map(|y| (x0..x1).map(move |x| (x, y)))
                .filter(|&(x, y)| near(at(x, y), BROWN, 30) || near(at(x, y), RUST, 30))
                .count()
        };
        assert!(dark(70, 180, 600, 310) > 4000, "title");
        assert!(dark(70, 650, 600, 700) > 800, "status");
        assert!(dark(70, 715, 600, 752) > 400, "address");
    }

    #[test]
    fn rays_turn_and_dots_pulse() {
        let s = Scene::new(640, 400, "t", "loading", None);
        let (mut a, mut b) = (vec![0; 640 * 400], vec![0; 640 * 400]);
        s.frame(0, &mut a);
        s.frame(3000, &mut b);
        let changed = a.iter().zip(&b).filter(|(x, y)| x != y).count();
        assert!(changed > 10_000, "rays moved: {changed}");
        // The static parts (sun centre) stay.
        let (cx, cy) = s.sun();
        let i = cy as usize * 640 + cx as usize;
        assert_eq!(a[i], b[i]);
    }

    #[test]
    fn small_and_portrait_sizes_do_not_panic() {
        for (w, h) in [(1, 1), (16, 9), (800, 1280), (100, 30)] {
            let s = Scene::new(
                w,
                h,
                "a very long title that will not fit",
                "x",
                Some("10.0.0.1"),
            );
            let mut px = vec![0; w * h];
            s.frame(12345, &mut px);
        }
    }
}
