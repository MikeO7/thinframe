//! Run the browser and keep it alive. This is the OpenRC service's `command`.
//
// TODO(TASK 5):
//  - spawn `cog --platform=drm --platform-params=rotation=N <url>` (or `cage -- cog --platform=wl`)
//    with XDG_RUNTIME_DIR=/run/user/<uid>, LIBSEAT_BACKEND=seatd
//  - if cog exits, wait 3 s and restart; back off to 30 s after 5 fast failures
//  - forward SIGTERM to the child and exit cleanly
// TODO(TASK 9):
//  - health check every `health_interval_s`: cog has a WebDriver/automation port option;
//    simplest is `COG_PLATFORM_DRM`-independent: write a tiny file from the page via
//    fetch() to a local endpoint? No server on the frame. Alternative: cog's `--enable-automation`
//    and ping it; or just check the process is alive and the DRM device is still held.
use crate::config::Config;
use std::io;

pub fn run(_cfg: &Config) -> io::Result<()> {
    Err(io::Error::other("kiosk: not implemented (TASK 5)"))
}
