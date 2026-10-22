//! Smooth display lettering for the "seventies" splash (TASK 15): Shrikhand (SIL OFL 1.1,
//! image/rootfs/usr/share/fonts/thinframe/OFL.txt) as a signed distance field atlas made on the
//! Mac by tools/fontgen. One 84 KB atlas draws every size with anti-aliased edges; it is inflated
//! on first use (about 170 KB of RAM). Format: see tools/fontgen/src/main.rs.
use crate::png;
use std::sync::OnceLock;

static ATLAS: &[u8] = include_bytes!("../assets/shrikhand.sdf");
/// Poppins ExtraLight (SIL OFL 1.1) for the "dawn" splash: `fontgen Poppins-ExtraLight.ttf
/// poppins-extralight.sdf 64 5`. Small text is drawn with `draw_bold` so the thin strokes hold up.
static THIN_ATLAS: &[u8] = include_bytes!("../assets/poppins-extralight.sdf");

#[derive(Clone, Copy, Default)]
struct Glyph {
    w: usize,
    h: usize,
    xoff: f32,
    yoff: f32,
    adv: f32,
    off: usize,
}

pub struct Font {
    base: f32,
    spread: f32,
    glyphs: Vec<Glyph>,
    kerns: Vec<(u8, u8, f32)>,
    data: Vec<u8>,
}

fn parse(b: &[u8]) -> Option<Font> {
    let rd16 = |i: usize| -> Option<i16> { Some(i16::from_le_bytes([*b.get(i)?, *b.get(i + 1)?])) };
    if b.get(..6)? != b"TFSDF1" {
        return None;
    }
    let (base, spread) = (*b.get(6)? as f32, *b.get(7)? as f32);
    let n = rd16(12)? as u16 as usize;
    let mut p = 14;
    let mut glyphs = Vec::with_capacity(n);
    let mut off = 0;
    for _ in 0..n {
        let g = b.get(p..p + 7)?;
        let (w, h) = (g[1] as usize, g[2] as usize);
        glyphs.push(Glyph {
            w,
            h,
            xoff: g[3] as i8 as f32,
            yoff: g[4] as i8 as f32,
            adv: u16::from_le_bytes([g[5], g[6]]) as f32 / 16.0,
            off,
        });
        off += w * h;
        p += 7;
    }
    let nk = rd16(p)? as u16 as usize;
    p += 2;
    let mut kerns = Vec::with_capacity(nk);
    for _ in 0..nk {
        kerns.push((*b.get(p)?, *b.get(p + 1)?, rd16(p + 2)? as f32 / 16.0));
        p += 4;
    }
    let raw = u32::from_le_bytes(b.get(p..p + 4)?.try_into().ok()?) as usize;
    let data = png::inflate(b.get(p + 4..)?, raw).ok()?;
    if data.len() != raw || raw != off {
        return None;
    }
    Some(Font {
        base,
        spread,
        glyphs,
        kerns,
        data,
    })
}

/// The embedded font; None only if the atlas is corrupt (a test guards that).
pub fn font() -> Option<&'static Font> {
    static F: OnceLock<Option<Font>> = OnceLock::new();
    F.get_or_init(|| parse(ATLAS)).as_ref()
}

/// The thin sans of the "dawn" splash; None only if its atlas is corrupt (a test guards that).
pub fn thin() -> Option<&'static Font> {
    static F: OnceLock<Option<Font>> = OnceLock::new();
    F.get_or_init(|| parse(THIN_ATLAS)).as_ref()
}

impl Font {
    fn glyph(&self, c: char) -> Glyph {
        let i = (c as u32).wrapping_sub(32) as usize;
        self.glyphs
            .get(i)
            .or_else(|| self.glyphs.get((b'?' - 32) as usize))
            .copied()
            .unwrap_or_default()
    }

    fn kern(&self, l: char, r: char) -> f32 {
        self.kerns
            .iter()
            .find(|(a, b, _)| *a as char == l && *b as char == r)
            .map_or(0.0, |k| k.2)
    }

    /// Advance width of `text` at `px` pixels per em.
    pub fn measure(&self, text: &str, px: f32) -> f32 {
        let s = px / self.base;
        let mut w = 0.0;
        let mut prev = None;
        for c in text.chars() {
            if let Some(p) = prev {
                w += self.kern(p, c) * s;
            }
            w += self.glyph(c).adv * s;
            prev = Some(c);
        }
        w
    }

    /// Rasterize `text` with its baseline at (`x`, `y`); `plot(x, y, coverage 0..1)` per pixel.
    pub fn draw(&self, text: &str, x: f32, y: f32, px: f32, plot: impl FnMut(i32, i32, f32)) {
        self.draw_bold(text, x, y, px, 0.0, plot)
    }

    /// Like `draw`, with every stroke `bold` pixels fatter on each side (0.4 turns ExtraLight
    /// into something readable at 26 px).
    pub fn draw_bold(
        &self,
        text: &str,
        x: f32,
        y: f32,
        px: f32,
        bold: f32,
        mut plot: impl FnMut(i32, i32, f32),
    ) {
        let s = px / self.base;
        let mut pen = x;
        let mut prev = None;
        for c in text.chars() {
            if let Some(p) = prev {
                pen += self.kern(p, c) * s;
            }
            prev = Some(c);
            let g = self.glyph(c);
            let (x0, y0) = (pen + g.xoff * s, y + g.yoff * s);
            pen += g.adv * s;
            if g.w < 2 || g.h < 2 {
                continue;
            }
            let field = &self.data[g.off..g.off + g.w * g.h];
            let (dx0, dy0) = (x0.floor() as i32, y0.floor() as i32);
            let (dx1, dy1) = (
                (x0 + g.w as f32 * s).ceil() as i32,
                (y0 + g.h as f32 * s).ceil() as i32,
            );
            for dy in dy0..dy1 {
                let v = ((dy as f32 + 0.5 - y0) / s - 0.5).clamp(0.0, (g.h - 1) as f32);
                let (vi, vf) = (v.floor() as usize, v.fract());
                let vi1 = (vi + 1).min(g.h - 1);
                for dx in dx0..dx1 {
                    let u = ((dx as f32 + 0.5 - x0) / s - 0.5).clamp(0.0, (g.w - 1) as f32);
                    let (ui, uf) = (u.floor() as usize, u.fract());
                    let ui1 = (ui + 1).min(g.w - 1);
                    let at = |yy: usize, xx: usize| field[yy * g.w + xx] as f32;
                    let top = at(vi, ui) + (at(vi, ui1) - at(vi, ui)) * uf;
                    let bot = at(vi1, ui) + (at(vi1, ui1) - at(vi1, ui)) * uf;
                    let d = (top + (bot - top) * vf - 128.0) / 127.0 * self.spread * s;
                    let a = (d + 0.5 + bold).clamp(0.0, 1.0);
                    if a > 0.0 {
                        plot(dx, dy, a);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atlas_loads_and_measures() {
        let f = font().expect("atlas parses");
        assert_eq!(f.glyphs.len(), 95);
        let w = f.measure("thinframe", 156.0);
        assert!(w > 500.0 && w < 1100.0, "width {w}");
        assert!((f.measure("ab", 100.0) - 2.0 * f.measure("ab", 50.0)).abs() < 0.5);
        assert_eq!(f.measure("", 40.0), 0.0);
    }

    #[test]
    fn thin_atlas_loads_and_bold_fattens() {
        let f = thin().expect("poppins atlas parses");
        assert_eq!(f.glyphs.len(), 95);
        let ink = |bold: f32| {
            let mut sum = 0.0;
            f.draw_bold("thinframe", 4.0, 40.0, 26.0, bold, |_, _, a| sum += a);
            sum
        };
        assert!(ink(0.45) > ink(0.0) * 1.25, "{} vs {}", ink(0.45), ink(0.0));
        let w = f.measure("thinframe", 176.0);
        assert!(w > 700.0 && w < 1100.0, "width {w}");
    }

    #[test]
    fn draws_solid_inside_and_nothing_far_away() {
        let f = font().unwrap();
        let (w, h) = (400usize, 120usize);
        let mut cov = vec![0f32; w * h];
        f.draw("Ho", 10.0, 100.0, 100.0, |x, y, a| {
            if (0..w as i32).contains(&x) && (0..h as i32).contains(&y) {
                let c = &mut cov[y as usize * w + x as usize];
                *c = c.max(a);
            }
        });
        let solid = cov.iter().filter(|&&a| a >= 0.99).count();
        let edge = cov.iter().filter(|&&a| a > 0.01 && a < 0.99).count();
        assert!(solid > 1500, "solid {solid}");
        assert!(edge > 100, "anti-aliased edge {edge}");
        assert!(
            cov[..w * 5].iter().all(|&a| a == 0.0),
            "nothing above the cap height"
        );
    }
}
