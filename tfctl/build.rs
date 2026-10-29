//! TF_DRAW_HASH: a hash of the code and assets that draw the splash and backdrops, for the render
//! cache key (rendercache.rs). Keyed by the binary instead, every software update re-rendered the
//! splash on its first boot, ~4 s of CPU before cog could start.
//! TF_BUILD_EPOCH: when this ran (or SOURCE_DATE_EPOCH), for the clock's upper bound (clock.rs).
//! It reruns only with the drawing files, so the value can be older than the binary: the bound
//! is then earlier, still 20 years out.
use std::fs;

const DRAWING: &[&str] = &[
    "src/splash.rs",
    "src/dawn.rs",
    "src/seventies.rs",
    "src/retro.rs",
    "src/font.rs",
    "src/png.rs",
    "src/sdf.rs",
    "src/rendercache.rs",
    "assets/poppins-extralight.sdf",
    "assets/shrikhand.sdf",
];

fn main() {
    // FNV-1a, 64 bits: no crate, and a collision only means a stale splash.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for f in DRAWING {
        println!("cargo:rerun-if-changed={f}");
        let b = fs::read(f).unwrap_or_else(|e| panic!("{f}: {e}"));
        for &x in f.as_bytes().iter().chain(&b) {
            h = (h ^ u64::from(x)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    println!("cargo:rustc-env=TF_DRAW_HASH={h:016x}");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    let built = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs())
        });
    println!("cargo:rustc-env=TF_BUILD_EPOCH={built}");
}
