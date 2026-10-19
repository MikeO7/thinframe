//! tfctl: the one custom binary on the frame. Static musl, no runtime deps, no allocator
//! surprises. Everything that is not the browser lives here so the shell layer stays
//! busybox + OpenRC only.
//!
//! Subcommands (see TASKS.md for which are done):
//!   tfctl kiosk      run cog on DRM, restart on crash, run the agent thread (service entry point)
//!   tfctl agent      control socket to the server alone (debugging; kiosk commands are logged)
//!   tfctl update     fetch main of the GitHub repo as a tarball, apply overlay + www, restart
//!   tfctl wifi SSID PSK   write iwd profile, reconnect
//!   tfctl status     one screen: uptime, ip, wifi, mem, cog pid, url, temp
//!   tfctl set KEY VALUE   rewrite one key in /etc/thinframe.conf (first pairing)
//!
//! Config: /etc/thinframe.conf (KEY=VALUE, see image/rootfs/etc/thinframe.conf), or $TFCTL_CONF.
//! Size budget: release binary under 1 MB. Check with `ls -l` in CI.

use std::env;
use std::process::ExitCode;
use std::sync::mpsc;
use std::thread;

mod agent;
mod agent_cfg;
mod config;
mod http;
mod json;
mod kiosk;
mod screen;
mod sha1;
mod status;
mod sys;
mod update;
mod wifi;
mod ws;

fn usage() -> ExitCode {
    eprintln!("usage: tfctl <kiosk|agent|update|wifi SSID PSK|status|set KEY VALUE>");
    ExitCode::from(2)
}

/// `tfctl agent` without a kiosk: print what the kiosk would have been told.
fn agent_alone(cfg: &config::Config) -> std::io::Result<()> {
    sys::trap_term();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for cmd in rx {
            eprintln!("tfctl: (no kiosk) {cmd:?}");
        }
    });
    agent::run(cfg, tx)
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let conf = config::path();
    let cfg = match config::load(&conf) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("tfctl: config: {e}");
            return ExitCode::from(1);
        }
    };
    let result = match args.first().map(String::as_str) {
        Some("kiosk") => kiosk::run(&cfg),
        Some("agent") => agent_alone(&cfg),
        Some("update") => update::run(&cfg),
        Some("wifi") if args.len() == 3 => wifi::run(&args[1], &args[2]),
        Some("status") => status::run(&cfg),
        Some("set") if args.len() == 3 => config::save_overrides(&conf, &[(&args[1], &args[2])]),
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
