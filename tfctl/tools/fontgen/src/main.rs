//! `fontgen FONT.ttf OUT.sdf [BASE_PX] [SPREAD]`: ASCII 32..126 of a TrueType font as a signed
//! distance field atlas for tfctl/src/sdf.rs (one atlas draws every size; TASK 15 "seventies").
//! Runs on the Mac only; its crates never reach the frame binary.
//!
//! Format (little endian): b"TFSDF1", u8 base_px, u8 spread, i16 ascent, i16 descent,
//! u16 glyphs, per glyph {u8 char, u8 w, u8 h, i8 xoff, i8 yoff, u16 advance*16},
//! u16 kerning pairs, per pair {u8 left, u8 right, i16 kern*16}, u32 raw length, raw deflate of
//! all glyph fields in order (w*h bytes each, 128 = edge, 255 = `spread` px inside).
use std::{env, fs};

const SUPER: usize = 8;

/// Squared Euclidean distance transform (Felzenszwalb & Huttenlocher), in place on a grid.
fn edt(grid: &mut [f32], w: usize, h: usize) {
    let n = w.max(h);
    let (mut f, mut d, mut v, mut z) = (vec![0f32; n], vec![0f32; n], vec![0usize; n], vec![0f32; n + 1]);
    let mut pass = |get: &mut dyn FnMut(usize) -> f32, len: usize, f: &mut Vec<f32>, out: &mut dyn FnMut(usize, f32)| {
        for (q, fq) in f.iter_mut().enumerate().take(len) {
            *fq = get(q);
        }
        let mut k = 0;
        v[0] = 0;
        z[0] = f32::NEG_INFINITY;
        z[1] = f32::INFINITY;
        for q in 1..len {
            let mut s;
            loop {
                let p = v[k];
                s = ((f[q] + (q * q) as f32) - (f[p] + (p * p) as f32)) / (2.0 * q as f32 - 2.0 * p as f32);
                if s <= z[k] && k > 0 {
                    k -= 1;
                } else {
                    break;
                }
            }
            if s <= z[k] {
                v[k] = q;
                z[k] = f32::NEG_INFINITY;
                z[k + 1] = f32::INFINITY;
                continue;
            }
            k += 1;
            v[k] = q;
            z[k] = s;
            z[k + 1] = f32::INFINITY;
        }
        k = 0;
        for q in 0..len {
            while z[k + 1] < q as f32 {
                k += 1;
            }
            let p = v[k];
            d[q] = (q as f32 - p as f32).powi(2) + f[p];
            out(q, d[q]);
        }
    };
    for x in 0..w {
        let col: Vec<f32> = (0..h).map(|y| grid[y * w + x]).collect();
        let mut res = vec![0f32; h];
        pass(&mut |q| col[q], h, &mut f, &mut |q, val| res[q] = val);
        for y in 0..h {
            grid[y * w + x] = res[y];
        }
    }
    for y in 0..h {
        let row: Vec<f32> = grid[y * w..(y + 1) * w].to_vec();
        let mut res = vec![0f32; w];
        pass(&mut |q| row[q], w, &mut f, &mut |q, val| res[q] = val);
        grid[y * w..(y + 1) * w].copy_from_slice(&res);
    }
}

fn main() {
    let a: Vec<String> = env::args().collect();
    if a.len() < 3 {
        eprintln!("usage: fontgen FONT.ttf OUT.sdf [BASE_PX=48] [SPREAD=5]");
        std::process::exit(2);
    }
    let base: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(48);
    let spread: usize = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(5);
    let font = fontdue::Font::from_bytes(fs::read(&a[1]).expect("read font"), fontdue::FontSettings::default()).expect("parse font");
    let hi = (base * SUPER) as f32;
    let lm = font.horizontal_line_metrics(base as f32).expect("line metrics");
    let mut out = b"TFSDF1".to_vec();
    out.extend([base as u8, spread as u8]);
    out.extend((lm.ascent.round() as i16).to_le_bytes());
    out.extend((lm.descent.round() as i16).to_le_bytes());
    out.extend(95u16.to_le_bytes());
    let mut data = Vec::new();
    for ch in (32u8..127).map(char::from) {
        let (m, cov) = font.rasterize(ch, hi);
        let pad = spread * SUPER;
        // Hi-res grid with padding, then sample the distance at each low-res pixel centre.
        let (gw, gh) = (m.width + 2 * pad, m.height + 2 * pad);
        let (lw, lh) = (gw.div_ceil(SUPER), gh.div_ceil(SUPER));
        let (gw, gh) = (lw * SUPER, lh * SUPER);
        let inside = |x: usize, y: usize| x >= pad && y >= pad && x - pad < m.width && y - pad < m.height && cov[(y - pad) * m.width + (x - pad)] >= 128;
        let big = 1e20f32;
        let mut to_in: Vec<f32> = (0..gw * gh).map(|i| if inside(i % gw, i / gw) { 0.0 } else { big }).collect();
        let mut to_out: Vec<f32> = (0..gw * gh).map(|i| if inside(i % gw, i / gw) { big } else { 0.0 }).collect();
        edt(&mut to_in, gw, gh);
        edt(&mut to_out, gw, gh);
        for ly in 0..lh {
            for lx in 0..lw {
                let i = (ly * SUPER + SUPER / 2) * gw + lx * SUPER + SUPER / 2;
                let d = (to_out[i].sqrt() - to_in[i].sqrt()) / SUPER as f32; // + inside
                let v = 128.0 + d / spread as f32 * 127.0;
                data.push(v.round().clamp(0.0, 255.0) as u8);
            }
        }
        let xoff = (m.xmin as f32 / SUPER as f32).floor() as i32 - spread as i32;
        let yoff = -((m.ymin + m.height as i32) as f32 / SUPER as f32).ceil() as i32 - spread as i32;
        let adv = (m.advance_width / SUPER as f32 * 16.0).round() as u16;
        out.extend([ch as u8, lw as u8, lh as u8, xoff as i8 as u8, yoff as i8 as u8]);
        out.extend(adv.to_le_bytes());
    }
    let mut kerns = Vec::new();
    for l in 32u8..127 {
        for r in 32u8..127 {
            if let Some(k) = font.horizontal_kern(l as char, r as char, base as f32) {
                let k16 = (k * 16.0).round() as i16;
                if k16 != 0 {
                    kerns.push((l, r, k16));
                }
            }
        }
    }
    out.extend((kerns.len() as u16).to_le_bytes());
    for (l, r, k) in &kerns {
        out.extend([*l, *r]);
        out.extend(k.to_le_bytes());
    }
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(miniz_oxide::deflate::compress_to_vec(&data, 10));
    fs::write(&a[2], &out).expect("write");
    eprintln!("fontgen: {} glyphs, {} kerning pairs, {} raw -> {} bytes", 95, kerns.len(), data.len(), out.len());
}
