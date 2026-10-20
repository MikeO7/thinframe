//! The default boot screen (TASK 15): a synthwave sunset drawn in software. Gradient sky with
//! stars, a striped sun on a glowing horizon, a perspective grid floor, the title in the
//! embedded 8x16 font with a chrome gradient, the boot status in amber under a block cursor,
//! then CRT scanlines and a vignette. Drawn upright (logical size); splash.rs rotates it onto the
//! panel. About 1 M pixels of float maths, once per boot. `SPLASH_STYLE=plain` gives the old
//! flat look.
use crate::font;
use crate::png::Image;
use crate::splash::Canvas;

const SKY_TOP: u32 = 0x07051a;
const SKY_MID: u32 = 0x1d0b3d;
const SKY_LOW: u32 = 0x5c1460;
const SUN_TOP: u32 = 0xffe14d;
const SUN_MID: u32 = 0xff8a3d;
const SUN_BOT: u32 = 0xff2a7a;
const HALO: u32 = 0xff3d9a;
const GROUND: u32 = 0x0b0518;
const GRID_FAR: u32 = 0x5a1d6e;
const GRID_NEAR: u32 = 0xff4fd8;
const HORIZON: u32 = 0xffb3ec;
const AMBER: u32 = 0xffb000;
const SHADOW: u32 = 0x2a0a40;

fn ch(c: u32, s: u32) -> f32 {
    ((c >> s) & 0xff) as f32
}

fn rgb(r: f32, g: f32, b: f32) -> u32 {
    let q = |v: f32| v.round().clamp(0.0, 255.0) as u32;
    q(r) << 16 | q(g) << 8 | q(b)
}

/// a..b at t (clamped to 0..1).
fn mix(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let m = |s| ch(a, s) + (ch(b, s) - ch(a, s)) * t;
    rgb(m(16), m(8), m(0))
}

/// a + b*k, per channel, saturating (light adds up: glows, stars).
fn add(a: u32, b: u32, k: f32) -> u32 {
    let m = |s| ch(a, s) + ch(b, s) * k;
    rgb(m(16), m(8), m(0))
}

fn dim(a: u32, k: f32) -> u32 {
    mix(0, a, k)
}

fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

impl Canvas {
    fn get(&self, x: usize, y: usize) -> u32 {
        self.px[y * self.w + x]
    }

    fn put(&mut self, x: isize, y: isize, c: u32) {
        if x >= 0 && y >= 0 {
            self.set(x as usize, y as usize, c);
        }
    }

    fn blend(&mut self, x: isize, y: isize, c: u32, k: f32) {
        if x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h {
            let p = self.get(x as usize, y as usize);
            self.set(x as usize, y as usize, add(p, c, k));
        }
    }
}

/// Pixel text at (x0, y0): every set font bit becomes a `scale` square, coloured by
/// `color(fraction of the glyph height)` so a gradient runs through each line.
fn text(c: &mut Canvas, s: &str, x0: isize, y0: isize, scale: usize, color: &dyn Fn(f32) -> u32) {
    let gh = (font::HEIGHT * scale) as f32;
    for (i, ch) in s.chars().enumerate() {
        let gx = x0 + (i * font::WIDTH * scale) as isize;
        for (row, bits) in font::glyph(ch).iter().enumerate() {
            for col in 0..font::WIDTH {
                if bits & (0x80 >> col) == 0 {
                    continue;
                }
                for dy in 0..scale {
                    let y = y0 + (row * scale + dy) as isize;
                    let k = color((row * scale + dy) as f32 / gh);
                    for dx in 0..scale {
                        c.put(gx + (col * scale + dx) as isize, y, k);
                    }
                }
            }
        }
    }
}

fn text_width(s: &str, scale: usize) -> isize {
    (s.chars().count() * font::WIDTH * scale) as isize
}

/// Draw the whole scene into `c` (upright). `title` is drawn big (upper-cased), `status` small.
pub fn draw(c: &mut Canvas, title: &str, status: &str, logo: Option<&Image>) {
    let (w, h) = (c.w as f32, c.h as f32);
    let u = w.min(h);
    let hz = (h * 0.64) as usize; // horizon row
    let (cx, r) = (w / 2.0, u * 0.27);
    let cy = hz as f32 - r * 0.38;

    // Sky, with the sun's halo added in.
    for y in 0..hz {
        let t = y as f32 / hz as f32;
        let base = if t < 0.55 {
            mix(SKY_TOP, SKY_MID, t / 0.55)
        } else {
            mix(SKY_MID, SKY_LOW, (t - 0.55) / 0.45)
        };
        for x in 0..c.w {
            let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt() / r;
            let halo = if d > 1.0 {
                (1.0 - (d - 1.0) / 1.1).max(0.0).powi(2) * 0.55
            } else {
                0.0
            };
            c.px[y * c.w + x] = add(base, HALO, halo);
        }
    }

    // Stars, thinning towards the horizon.
    let n = (w * h / 2400.0) as u32;
    for i in 0..n {
        let x = (hash(i * 3 + 1) % c.w as u32) as isize;
        let yf = (hash(i * 3 + 2) % 10_000) as f32 / 10_000.0;
        let y = (yf * yf * hz as f32 * 0.8) as isize;
        let b = (hash(i * 3 + 3) % 1000) as f32 / 1000.0;
        let star = mix(0x8a7fd0, 0xffffff, b);
        c.blend(x, y, star, 0.35 + 0.65 * b);
        if b > 0.93 {
            for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                c.blend(x + dx, y + dy, star, 0.45);
            }
        }
    }

    // The sun: yellow to pink, cut by bands that thicken towards the horizon.
    let band0 = cy - r * 0.45;
    let period = r * 0.135;
    let top = (cy - r).max(0.0) as usize;
    for y in top..hz {
        let dy = y as f32 + 0.5 - cy;
        if dy.abs() > r {
            continue;
        }
        let yf = y as f32;
        if yf > band0 {
            let k = yf - band0;
            let gap = period * (0.06 + 0.62 * (k / (hz as f32 - band0)).powf(1.3));
            if k % period < gap {
                continue;
            }
        }
        let half = (r * r - dy * dy).sqrt();
        let st = (yf - (cy - r)) / (hz as f32 - (cy - r));
        let col = if st < 0.5 {
            mix(SUN_TOP, SUN_MID, st / 0.5)
        } else {
            mix(SUN_MID, SUN_BOT, (st - 0.5) / 0.5)
        };
        let (x0, x1) = ((cx - half).floor() as isize, (cx + half).ceil() as isize);
        for x in x0..x1 {
            // Soft edge: one pixel of coverage blend.
            let e = (half - (x as f32 + 0.5 - cx).abs()).clamp(0.0, 1.0);
            if (x as usize) < c.w {
                let p = c.get(x as usize, y);
                c.set(x as usize, y, mix(p, col, e));
            }
        }
    }

    // Mountains on both sides of the horizon: dark, with a neon rim.
    let noise = |x: f32| {
        let mut v = 0.0;
        for (f, a) in [(0.006, 0.6), (0.017, 0.28), (0.05, 0.12)] {
            let p = x * f;
            let (i, t) = (p.floor() as u32, p.fract());
            let s = |k: u32| {
                (hash(k.wrapping_mul(7919).wrapping_add((f * 1e4) as u32)) % 1000) as f32 / 1000.0
            };
            v += a * (s(i) + (s(i + 1) - s(i)) * (t * t * (3.0 - 2.0 * t)));
        }
        v
    };
    let mut ridge = vec![hz as isize; c.w];
    for (x, top) in ridge.iter_mut().enumerate() {
        let side = ((x as f32 - cx).abs() / (w / 2.0) - 0.28) / 0.72;
        if side <= 0.0 {
            continue;
        }
        let height = h * 0.17 * side.min(1.0).powf(0.7) * (0.35 + 0.65 * noise(x as f32));
        *top = hz as isize - height as isize;
        for y in (*top).max(0)..hz as isize {
            let t = (y - *top) as f32 / height.max(1.0);
            c.put(x as isize, y, mix(0x241040, 0x0d0620, t));
        }
    }
    for x in 0..c.w {
        if ridge[x] < hz as isize {
            let slope = (ridge[x] - ridge[x.saturating_sub(1)]).abs().max(1);
            for d in 0..slope.min(6) + 1 {
                c.blend(x as isize, ridge[x] + d, 0xff5fe0, 0.9 - 0.12 * d as f32);
            }
        }
    }

    // Ground and its grid.
    for y in hz..c.h {
        let t = (y - hz) as f32 / (h - hz as f32);
        let base = mix(0x1a0830, GROUND, t.sqrt());
        for x in 0..c.w {
            c.px[y * c.w + x] = base;
        }
    }
    let depth = h - hz as f32;
    for k in 1..=12 {
        let t = (k as f32 / 12.0).powf(2.2);
        let y = hz as f32 + depth * t;
        let col = mix(GRID_FAR, GRID_NEAR, t.sqrt());
        let thick = 1.0 + 2.0 * t;
        for dy in 0..thick.ceil() as isize {
            let yy = y as isize + dy;
            for x in 0..c.w as isize {
                c.blend(x, yy, col, 0.25 + 0.65 * t.sqrt());
            }
        }
    }
    let spacing = w / 9.0;
    for i in -14..=14 {
        let i = i as f32;
        for y in hz..c.h {
            let t = (y - hz) as f32 / depth;
            let x = cx + i * spacing * 0.08 + i * spacing * 1.9 * t;
            let col = mix(GRID_FAR, GRID_NEAR, t.sqrt());
            let thick = 1.0 + 1.6 * t;
            let x0 = (x - thick / 2.0).round() as isize;
            for dx in 0..thick.ceil() as isize {
                c.blend(x0 + dx, y as isize, col, 0.8 * t.powf(0.6));
            }
        }
    }

    // Horizon: a hot line with a glow above and below.
    for d in -18..=18isize {
        let k = (1.0 - d.unsigned_abs() as f32 / 19.0).powi(3);
        for x in 0..c.w as isize {
            c.blend(x, hz as isize + d, HORIZON, 0.5 * k);
        }
    }
    for x in 0..c.w as isize {
        c.put(x, hz as isize, HORIZON);
    }

    // An optional logo sits on the sun.
    if let Some(img) = logo {
        let (x0, y0) = (
            (cx - img.w as f32 / 2.0) as isize,
            (cy - img.h as f32 / 2.0) as isize,
        );
        for iy in 0..img.h {
            for ix in 0..img.w {
                let p = img.px[iy * img.w + ix];
                let a = (p >> 24) as f32 / 255.0;
                let (x, y) = (x0 + ix as isize, y0 + iy as isize);
                if a > 0.0 && x >= 0 && y >= 0 && (x as usize) < c.w && (y as usize) < c.h {
                    let bg = c.get(x as usize, y as usize);
                    c.set(x as usize, y as usize, mix(bg, p & 0xff_ffff, a));
                }
            }
        }
    }

    // Title: chrome pixel letters with a deep shadow and a thin outline.
    let title = title.to_uppercase();
    let n = title.chars().count().max(1);
    let ts = ((w * 0.78) / (n * font::WIDTH) as f32)
        .min(h * 0.17 / font::HEIGHT as f32)
        .floor()
        .max(2.0) as usize;
    let tx = (c.w as isize - text_width(&title, ts)) / 2;
    let ty = (h * 0.1) as isize;
    let o = (ts as isize / 2).max(1);
    // Soft neon glow behind the title block.
    let (gx0, gy0) = (tx, ty);
    let (gx1, gy1) = (
        tx + text_width(&title, ts),
        ty + (font::HEIGHT * ts) as isize,
    );
    let reach = (ts * 9) as f32;
    for y in (gy0 - reach as isize).max(0)..(gy1 + reach as isize).min(c.h as isize) {
        for x in (gx0 - reach as isize).max(0)..(gx1 + reach as isize).min(c.w as isize) {
            let dx = (gx0 - x).max(x - gx1).max(0) as f32;
            let dy = (gy0 - y).max(y - gy1).max(0) as f32;
            let k = (1.0 - (dx * dx + dy * dy).sqrt() / reach).max(0.0);
            c.blend(x, y, 0x8a2be2, 0.16 * k * k * k);
        }
    }
    text(c, &title, tx + o * 2, ty + o * 2, ts, &|_| SHADOW);
    for (dx, dy) in [(-o, 0), (o, 0), (0, -o), (0, o)] {
        text(c, &title, tx + dx, ty + dy, ts, &|_| 0x3b1670);
    }
    text(c, &title, tx, ty, ts, &|t| {
        if t < 0.5 {
            mix(0x7ef0ff, 0xffffff, t / 0.5)
        } else if t < 0.56 {
            0xffffff
        } else {
            mix(0xff5ccf, 0x7a2fe0, (t - 0.56) / 0.44)
        }
    });

    // Status: amber terminal line on a dark plate, block cursor after it. None for the
    // status page backdrop, where the page draws the live line itself.
    if !status.is_empty() {
        let ss = (u / 220.0).floor().max(2.0) as usize;
        let line = format!("{status} ");
        let sw = text_width(&line, ss) + (font::WIDTH * ss) as isize;
        let sx = (c.w as isize - sw) / 2;
        let sy = (h * 0.86) as isize - (font::HEIGHT * ss / 2) as isize;
        let pad = (ss * 6) as isize;
        for y in sy - pad / 2..sy + (font::HEIGHT * ss) as isize + pad / 2 {
            for x in sx - pad..sx + sw + pad {
                if x >= 0 && y >= 0 && (x as usize) < c.w && (y as usize) < c.h {
                    let p = c.get(x as usize, y as usize);
                    c.set(x as usize, y as usize, dim(p, 0.28));
                }
            }
        }
        for x in sx - pad..sx + sw + pad {
            c.put(x, sy - pad / 2, 0x3fd8ff);
            c.put(x, sy + (font::HEIGHT * ss) as isize + pad / 2, 0x3fd8ff);
        }
        text(c, &line, sx + ss as isize, sy + ss as isize, ss, &|_| {
            0x3a1d00
        });
        text(c, &line, sx, sy, ss, &|_| AMBER);
        let cur_x = sx + text_width(&line, ss);
        for y in sy + (3 * ss) as isize..sy + (14 * ss) as isize {
            for x in cur_x..cur_x + (font::WIDTH * ss) as isize - ss as isize {
                c.put(x, y, AMBER);
            }
        }
    }
    // CRT: every third row darker, corners darker.
    for y in 0..c.h {
        let scan = if y % 3 == 2 { 0.72 } else { 1.0 };
        let ny = (y as f32 / h - 0.5) * 2.0;
        for x in 0..c.w {
            let nx = (x as f32 / w - 0.5) * 2.0;
            let v = 1.0 - 0.32 * ((nx * nx + ny * ny) / 2.0).powf(1.5);
            let i = y * c.w + x;
            c.px[i] = dim(c.px[i], scan * v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(c: &Canvas, f: impl Fn(u32) -> bool) -> usize {
        c.px.iter().filter(|p| f(**p)).count()
    }

    #[test]
    fn colour_helpers() {
        assert_eq!(mix(0x000000, 0xff8040, 0.5), 0x804020);
        assert_eq!(mix(0x102030, 0xffffff, -1.0), 0x102030);
        assert_eq!(add(0xf0f0f0, 0x404040, 1.0), 0xffffff);
        assert_eq!(dim(0x804020, 0.5), 0x402010);
    }

    #[test]
    fn scene_has_sun_grid_title_and_status() {
        let mut c = Canvas::new(1280, 800, 0);
        draw(&mut c, "thinframe", "starting", None);
        let hz = (800.0 * 0.64) as usize;
        // Warm sun pixels above the horizon, centred.
        let sunny = |p: u32| ch(p, 16) > 180.0 && ch(p, 8) > 60.0 && ch(p, 0) < 200.0;
        assert!(sunny(c.get(640, (hz as f32 - 0.27 * 800.0) as usize + 30)));
        // Bright magenta grid lines below the horizon.
        let grid = (hz + 20..800)
            .flat_map(|y| (0..1280).map(move |x| (x, y)))
            .filter(|&(x, y)| ch(c.get(x, y), 16) > 150.0 && ch(c.get(x, y), 0) > 120.0)
            .count();
        assert!(grid > 5_000, "grid pixels {grid}");
        // Near-white chrome in the title band.
        let title = (80..240)
            .flat_map(|y| (0..1280).map(move |x| (x, y)))
            .filter(|&(x, y)| c.get(x, y) & 0xe0e0e0 == 0xe0e0e0)
            .count();
        assert!(title > 2_000, "title pixels {title}");
        // Amber status text near the bottom.
        let amber =
            |p: u32| ch(p, 16) > 150.0 && (90.0..200.0).contains(&ch(p, 8)) && ch(p, 0) < 40.0;
        assert!(count(&c, amber) > 300);
    }

    #[test]
    fn portrait_and_tiny_canvases_do_not_panic() {
        for (w, h) in [(800, 1280), (64, 40), (1, 1)] {
            let mut c = Canvas::new(w, h, 0);
            draw(&mut c, "a very long title for a small panel", "", None);
        }
    }

    #[test]
    fn logo_lands_on_the_sun() {
        let img = Image {
            w: 4,
            h: 4,
            px: vec![0xff_00ff00; 16],
        };
        let mut c = Canvas::new(1280, 800, 0);
        draw(&mut c, "x", "y", Some(&img));
        let hz = (800.0 * 0.64) as usize;
        let cy = (hz as f32 - 0.27 * 800.0 * 0.38) as usize;
        assert!(ch(c.get(640, cy), 8) > 150.0 && ch(c.get(640, cy), 16) < 60.0);
    }
}
