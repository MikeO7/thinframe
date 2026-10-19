//! tfctl: the one custom binary on the frame. Static musl, no runtime deps, no allocator
//! surprises. Everything that is not the browser lives here so the shell layer stays
//! busybox + OpenRC only.
//!
//! Subcommands (see TASKS.md for which are done):
//!   tfctl kiosk      run cog on DRM, restart on crash, health-check the page (service entry point)
//!   tfctl update     fetch main of the GitHub repo as a tarball, apply overlay + www, restart
//!   tfctl wifi SSID PSK   write iwd profile, reconnect
//!   tfctl status     one screen: uptime, ip, wifi, mem, cog pid, url, temp
//!   tfctl agent      poll the server for config, post heartbeats (also runs inside kiosk)
//!
//! Config: /etc/thinframe.conf (KEY=VALUE, see image/rootfs/etc/thinframe.conf).
//! Size budget: release binary under 1 MB. Check with `ls -l` in CI.

use std::env;
use std::process::ExitCode;

mod agent;
mod config;
mod kiosk;
mod status;
mod update;
mod wifi;

fn usage() -> ExitCode {
    eprintln!("usage: tfctl <kiosk|agent|update|wifi SSID PSK|status>");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let cfg = match config::load(config::DEFAULT_PATH) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("tfctl: config: {e}");
            return ExitCode::from(1);
        }
    };
    let result = match args.first().map(String::as_str) {
        Some("kiosk") => kiosk::run(&cfg),
        Some("update") => update::run(&cfg),
        Some("wifi") if args.len() == 3 => wifi::run(&args[1], &args[2]),
        Some("status") => status::run(&cfg),
        Some("agent") => agent::run(&cfg),
        _ => return usage(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("tfctl: {e}");
            ExitCode::from(1)
        }
    }
}
