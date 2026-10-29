//! Boot splash (TASK 15), drawn in software into a DRM dumb
//! buffer (drm.rs). `SPLASH_STYLE=dawn` (default) is the frosted-glass sunrise in dawn.rs,
//! `seventies` the animated scene in seventies.rs, `retro` the synthwave scene in retro.rs; `plain` is a flat
//! colour, the logo PNG and one text line in the embedded 8x16 font. `COG_ROTATION` is applied
//! exactly as cog applies it, so splash, status page and dashboard share one orientation.
//!
//! `tfctl splash [--png FILE] [STATUS]` draws and holds until SIGTERM (or writes a preview);
//! the kiosk draws it in process and keeps it up while cog starts (see `Splash::hand_over`).
use crate::config::Config;
use crate::{dawn, drm, font, png, rendercache, retro, seventies, sys};
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub const DEVICE: &str = "/dev/dri/card0";

/// Open the first DRM card that can drive a panel. On the A33 card0 is lima (3D only, no mode
/// setting: GETRESOURCES fails) and the display engine is card1 [V 2026-10-22]; the order is
/// whichever driver probed first, so try them all.
fn open_display() -> io::Result<drm::Display> {
    let mut why = Vec::new();
    for n in 0..8 {
        let path = format!("/dev/dri/card{n}");
        if !std::path::Path::new(&path).exists() {
            continue;
        }
        match drm::Display::open(&path) {
            Ok(d) => {
                eprintln!("tfctl: splash: using {path}");
                return Ok(d);
            }
            Err(e) => why.push(format!("{path}: {e}")),
        }
    }
    if why.is_empty() {
        return drm::Display::open(DEVICE);
    }
    Err(io::Error::other(why.join("; ")))
}
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

    /// Logical point -> physical pixel, turned the way cog turns its output for the same
    /// `rotation`: at 3 the top of the picture is at the panel's right edge (x = w-1), at 1 at
    /// its left edge. Seen side by side with cog under QEMU, 2026-10-20 [V].
    pub fn rotate(&self, rotation: u8, lx: usize, ly: usize) -> (usize, usize) {
        match rotation % 4 {
            0 => (lx, ly),
            1 => (ly, self.h - 1 - lx),
            2 => (self.w - 1 - lx, self.h - 1 - ly),
            _ => (self.w - 1 - ly, lx),
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
/// Pushed by the server (`splash_png`); wins over the image's logo.
pub const DATA_LOGO_PATH: &str = "/data/thinframe/splash.png";

/// The logo, if one is installed and decodes; a bad file is logged and ignored.
fn logo_path() -> String {
    std::env::var("TFCTL_SPLASH_PNG").unwrap_or_else(|_| {
        let data = std::path::Path::new(DATA_LOGO_PATH).exists();
        (if data { DATA_LOGO_PATH } else { LOGO_PATH }).into()
    })
}

fn load_logo() -> Option<png::Image> {
    let path = logo_path();
    let bytes = std::fs::read(&path).ok()?;
    match png::decode(&bytes) {
        Ok(i) => Some(i),
        Err(e) => {
            eprintln!("tfctl: splash: {path}: {e}");
            None
        }
    }
}

/// A live splash: dropping it stops the animation and frees the buffer (and DRM master if it
/// still holds it).
pub struct Splash {
    display: Arc<Mutex<drm::Display>>,
    handed_over: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    anim: Option<thread::JoinHandle<()>>,
}

impl Splash {
    /// Hold the animation (the backlight is off): the picture stays, no frames are drawn. A
    /// cog that paints while paused still ends the splash.
    pub fn pause(&self, on: bool) {
        self.paused.store(on, Ordering::SeqCst);
    }

    /// Let cog take the display: drop DRM master, keep the picture up. False if that failed
    /// (then the caller must drop the splash, or cog cannot become master).
    pub fn hand_over(&mut self) -> bool {
        let mut d = self.display.lock().unwrap_or_else(|e| e.into_inner());
        match d.drop_master() {
            Ok(()) => {
                self.handed_over.store(true, Ordering::SeqCst);
                true
            }
            Err(e) => {
                eprintln!("tfctl: splash: {e}");
                false
            }
        }
    }

    /// Handed over and no longer on screen: cog has shown its first frame.
    pub fn replaced(&self) -> bool {
        self.handed_over.load(Ordering::SeqCst)
            && !self
                .display
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .shown()
    }
}

impl Drop for Splash {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.anim.take() {
            let _ = t.join();
        }
    }
}

/// The animated styles. Built for one size, title, status and address; `frame` draws upright.
enum Scene {
    Dawn(Box<dawn::Scene>),
    Seventies(seventies::Scene),
}

impl Scene {
    fn new(
        style: &str,
        w: usize,
        h: usize,
        title: &str,
        status: &str,
        ip: Option<&str>,
    ) -> Option<Scene> {
        match style {
            "dawn" => Some(Scene::Dawn(Box::new(dawn::Scene::new(
                w, h, title, status, ip,
            )))),
            "seventies" => Some(Scene::Seventies(seventies::Scene::new(
                w, h, title, status, ip,
            ))),
            _ => None,
        }
    }

    fn frame(&self, t_ms: u64, out: &mut [u32]) {
        match self {
            Scene::Dawn(s) => s.frame(t_ms, out),
            Scene::Seventies(s) => s.frame(t_ms, out),
        }
    }

    /// Time between frames: what the style needs and the A33 can draw [U].
    fn period(&self) -> Duration {
        match self {
            Scene::Dawn(_) => Duration::from_millis(150),
            Scene::Seventies(_) => FRAME,
        }
    }
}

/// What the splash shows: `SPLASH_STYLE` (dawn, seventies, retro or plain), colours, the title
/// (`SPLASH_TEXT`), the boot status line under it and the frame's address once it has one.
pub struct Look<'a> {
    pub style: &'a str,
    pub bg: u32,
    pub title: &'a str,
    pub status: &'a str,
    pub rotation: u8,
    pub ip: Option<String>,
}

impl<'a> Look<'a> {
    pub fn new(cfg: &'a Config, status: &'a str) -> Look<'a> {
        Look {
            style: &cfg.splash_style,
            bg: cfg.splash_bg,
            title: &cfg.splash_text,
            status,
            rotation: cfg.rotation,
            ip: lan_ip(),
        }
    }
}

/// The frame's first LAN address (not the USB-C link's), if it has one yet.
/// From the kernel's tables only: `status::collect` asks iwctl (up to 5 s while iwd starts),
/// which held the first splash frame and froze the animation.
pub fn lan_ip() -> Option<String> {
    crate::status::ips()
        .into_iter()
        .find(|ip| ip != crate::statuspage::USB_IP)
}

/// Upright picture `l` (logical) turned into the panel canvas `c`.
fn turn_into(c: &mut Canvas, rotation: u8, l: &[u32], lw: usize, lh: usize) {
    for ly in 0..lh {
        for lx in 0..lw {
            let (x, y) = c.rotate(rotation, lx, ly);
            c.px[y * c.w + x] = l[ly * lw + lx];
        }
    }
}

/// The splash as panel pixels (physical `w` x `h`), at the start of its animation.
pub fn render(look: &Look, w: usize, h: usize) -> Canvas {
    let mut c = Canvas::new(w, h, look.bg);
    let (lw, lh) = c.logical(look.rotation);
    if let Some(scene) = Scene::new(
        look.style,
        lw,
        lh,
        look.title,
        look.status,
        look.ip.as_deref(),
    ) {
        let mut l = vec![0; lw * lh];
        scene.frame(0, &mut l);
        turn_into(&mut c, look.rotation, &l, lw, lh);
        return c;
    }
    let logo = load_logo();
    if look.style == "plain" {
        let text = if look.status.is_empty() {
            look.title
        } else {
            look.status
        };
        match logo {
            Some(logo) => {
                c.blit_centered(&logo, look.rotation, -((font::HEIGHT * SCALE) as isize));
                let dy = (logo.h / 2 + font::HEIGHT * SCALE) as isize;
                c.text_centered_at(text, SCALE, TEXT_COLOR, look.rotation, dy);
            }
            None => c.text_centered(text, SCALE, TEXT_COLOR, look.rotation),
        }
        return c;
    }
    let mut l = Canvas::new(lw, lh, 0);
    retro::draw(&mut l, look.title, look.status, logo.as_ref());
    turn_into(&mut c, look.rotation, &l.px, lw, lh);
    c
}

/// The render cache key of the splash `look` on a `w` x `h` panel.
fn splash_key(look: &Look, w: usize, h: usize) -> String {
    let (wh, bg, rot) = (
        format!("{w}x{h}"),
        look.bg.to_string(),
        look.rotation.to_string(),
    );
    let ip = look.ip.as_deref().unwrap_or("");
    rendercache::key(
        &[
            "splash",
            look.style,
            look.title,
            look.status,
            ip,
            &bg,
            &rot,
            &wh,
        ],
        &logo_path(),
    )
}

/// `render`, from the render cache when this exact picture was drawn on an earlier boot.
fn render_cached(look: &Look, w: usize, h: usize) -> Canvas {
    let key = splash_key(look, w, h);
    let b = rendercache::get_or_make(Path::new(rendercache::DIR), &key, Some(w * h * 4), || {
        rendercache::to_bytes(&render(look, w, h).px)
    });
    Canvas {
        w,
        h,
        px: rendercache::from_bytes(&b),
    }
}

/// The scene without a status line, upright, as a PNG for the status page's background (TASK 15:
/// the same picture from splash to status page). Panel 800x1280 turned by COG_ROTATION. None for
/// `SPLASH_STYLE=plain`. Written once per boot (tmpfs).
pub fn write_backdrop(cfg: &Config, path: &str) -> io::Result<Option<String>> {
    backdrop(cfg, path, true)
}

/// The same for the pages that wait for a person (Wi-Fi, PAIR). Under `dawn` it is the frosted
/// sheet alone, because the page's own text sits on it; the other styles keep their picture.
pub fn write_page_backdrop(cfg: &Config, path: &str) -> io::Result<Option<String>> {
    backdrop(cfg, path, false)
}

/// One backdrop drawn in the background at a time (see `backdrop`).
static BACKDROP_BUSY: AtomicBool = AtomicBool::new(false);

/// The backdrop file at `path`, from the render cache. A miss (first boot, a new look) draws for
/// ~4 s; that happens in the background and this returns None, so the status page goes up at once
/// without the picture and carries it from its next write. Drawn in line, the miss held the
/// splash and the agent's first connect back 4.5 s [V 2026-10-27].
fn backdrop(cfg: &Config, path: &str, titled: bool) -> io::Result<Option<String>> {
    if cfg.splash_style == "plain" {
        return Ok(None);
    }
    if Path::new(path).exists() {
        return Ok(Some(path.to_string()));
    }
    let title = if titled || cfg.splash_style != "dawn" {
        cfg.splash_text.clone()
    } else {
        String::new()
    };
    let rot = cfg.rotation.to_string();
    let key = rendercache::key(
        &[
            "backdrop",
            &cfg.splash_style,
            &title,
            &cfg.splash_text,
            &rot,
        ],
        &logo_path(),
    );
    if let Some(png) = rendercache::get(Path::new(rendercache::DIR), &key, None) {
        write_atomic(path, &png)?;
        return Ok(Some(path.to_string()));
    }
    if !BACKDROP_BUSY.swap(true, Ordering::SeqCst) {
        let (style, text, rotation, path) = (
            cfg.splash_style.clone(),
            cfg.splash_text.clone(),
            cfg.rotation,
            path.to_string(),
        );
        thread::spawn(move || {
            sys::nice_this_thread(10);
            let (lw, lh) = Canvas::new(800, 1280, 0).logical(rotation);
            let px = if let Some(scene) = Scene::new(&style, lw, lh, &title, "", None) {
                let mut l = vec![0; lw * lh];
                scene.frame(0, &mut l);
                l
            } else {
                let mut l = Canvas::new(lw, lh, 0);
                retro::draw(&mut l, &text, "", load_logo().as_ref());
                l.px
            };
            let png = png::encode_rgb(lw, lh, &px);
            rendercache::put(Path::new(rendercache::DIR), &key, &png);
            if let Err(e) = write_atomic(&path, &png) {
                eprintln!("tfctl: splash: backdrop {path}: {e}");
            }
            BACKDROP_BUSY.store(false, Ordering::SeqCst);
        });
    }
    Ok(None)
}

fn write_atomic(path: &str, b: &[u8]) -> io::Result<()> {
    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, b)?;
    std::fs::rename(&tmp, path)
}

/// Time between seventies frames; the A33 draws one in well under this budget [U].
const FRAME: Duration = Duration::from_millis(100);

pub fn show(look: &Look) -> io::Result<Splash> {
    let mut d = open_display()?;
    let (w, h) = (d.mode_size.width as usize, d.mode_size.height as usize);
    let animated = matches!(look.style, "dawn" | "seventies");
    let key = splash_key(look, w, h);
    // A miss draws for ~4 s (first boot, a new look). Animated, the background goes up at once
    // and the animation's first frame brings the picture and fills the cache; a still splash is
    // drawn here.
    let (c, fill) = match rendercache::get(Path::new(rendercache::DIR), &key, Some(w * h * 4)) {
        Some(b) => (
            Canvas {
                w,
                h,
                px: rendercache::from_bytes(&b),
            },
            None,
        ),
        None if animated => (Canvas::new(w, h, look.bg), Some(key)),
        None => (render_cached(look, w, h), None),
    };
    d.write(&c.px);
    d.commit()?;
    eprintln!(
        "tfctl: splash: {w}x{h} rotation {} {}: {:?} / {:?} / {:?}",
        look.rotation, look.style, look.title, look.status, look.ip
    );
    let display = Arc::new(Mutex::new(d));
    let (handed_over, stop, paused) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    );
    let anim = animated.then(|| {
        let (disp, handed, stop, pause) = (
            display.clone(),
            handed_over.clone(),
            stop.clone(),
            paused.clone(),
        );
        let (style, title, status, ip, rot) = (
            look.style.to_string(),
            look.title.to_string(),
            look.status.to_string(),
            look.ip.clone(),
            look.rotation,
        );
        thread::spawn(move || {
            // It runs while cog starts; cog's first frame matters more than a smooth dawn.
            sys::nice_this_thread(10);
            animate(
                &disp,
                (&handed, &stop, &pause),
                (w, h),
                rot,
                (&style, &title, &status),
                ip,
                fill,
            )
        })
    });
    Ok(Splash {
        display,
        handed_over,
        stop,
        paused,
        anim,
    })
}

/// Redraw the animated scene every `period` until stopped or until cog's picture replaced ours,
/// drawing nothing while paused. The address is looked up every 2 s and the scene rebuilt when
/// it appears or changes.
#[allow(clippy::too_many_arguments)]
fn animate(
    disp: &Mutex<drm::Display>,
    (handed, stop, paused): (&AtomicBool, &AtomicBool, &AtomicBool),
    (w, h): (usize, usize),
    rot: u8,
    (style, title, status): (&str, &str, &str),
    mut ip: Option<String>,
    mut fill: Option<String>,
) {
    let mut c = Canvas::new(w, h, 0);
    let (lw, lh) = c.logical(rot);
    let Some(mut scene) = Scene::new(style, lw, lh, title, status, ip.as_deref()) else {
        return;
    };
    let mut l = vec![0; lw * lh];
    let map: Vec<u32> = (0..lw * lh)
        .map(|i| {
            let (x, y) = c.rotate(rot, i % lw, i / lw);
            (y * w + x) as u32
        })
        .collect();
    let start = Instant::now();
    let (mut frames, mut busy, mut last_ip) = (0u32, Duration::ZERO, Instant::now());
    while !stop.load(Ordering::SeqCst) {
        if paused.load(Ordering::SeqCst) {
            let d = disp.lock().unwrap_or_else(|e| e.into_inner());
            if handed.load(Ordering::SeqCst) && !d.shown() {
                break;
            }
            drop(d);
            thread::sleep(Duration::from_millis(250));
            continue;
        }
        let t0 = Instant::now();
        if last_ip.elapsed() >= Duration::from_secs(2) {
            last_ip = Instant::now();
            let now = lan_ip();
            if now != ip {
                ip = now;
                if let Some(s) = Scene::new(style, lw, lh, title, status, ip.as_deref()) {
                    scene = s;
                }
            }
        }
        scene.frame(start.elapsed().as_millis() as u64, &mut l);
        for (i, &p) in l.iter().enumerate() {
            c.px[map[i] as usize] = p;
        }
        {
            let mut d = disp.lock().unwrap_or_else(|e| e.into_inner());
            if handed.load(Ordering::SeqCst) && !d.shown() {
                break;
            }
            d.write(&c.px);
        }
        // The first frame is the picture the cache missed (`show`): keep it for the next start.
        if let Some(key) = fill.take() {
            rendercache::put(
                Path::new(rendercache::DIR),
                &key,
                &rendercache::to_bytes(&c.px),
            );
        }
        busy += t0.elapsed();
        frames += 1;
        if frames == 20 {
            eprintln!(
                "tfctl: splash: 20 frames, {} ms each on average",
                busy.as_millis() / 20
            );
        }
        thread::sleep(scene.period().saturating_sub(t0.elapsed()));
    }
}

/// `tfctl splash [--png FILE] [STATUS]`: draw on the panel and hold until SIGTERM/SIGINT, or
/// write the upright picture (800x1280 panel turned by COG_ROTATION) to FILE to preview it.
pub fn run(cfg: &Config, args: &[String]) -> io::Result<()> {
    let (png_out, status) = match args {
        [f, path, rest @ ..] if f == "--png" => (Some(path.as_str()), rest.first()),
        rest => (None, rest.first()),
    };
    let look = Look::new(cfg, status.map_or("starting", String::as_str));
    if let Some(path) = png_out {
        let (w, h) = (800, 1280);
        let phys = render(&look, w, h);
        let (lw, lh) = phys.logical(look.rotation);
        let mut up = vec![0; lw * lh];
        for ly in 0..lh {
            for lx in 0..lw {
                let (x, y) = phys.rotate(look.rotation, lx, ly);
                up[ly * lw + lx] = phys.px[y * w + x];
            }
        }
        std::fs::write(path, png::encode_rgb(lw, lh, &up))?;
        eprintln!("tfctl: splash: {lw}x{lh} preview in {path}");
        return Ok(());
    }
    sys::trap_term();
    let _s = show(&look)?;
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
        assert_eq!(c.rotate(1, 0, 0), (0, 19));
        assert_eq!(c.rotate(2, 0, 0), (9, 19));
        assert_eq!(
            c.rotate(3, 0, 0),
            (9, 0),
            "cog rotation 3: top-left lands top-right"
        );
        assert_eq!(c.rotate(3, 19, 9), (0, 19));
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
        let cfg = Config::default();
        assert!(show(&Look::new(&cfg, "x")).is_err());
    }
}
