//! tfctl: the one custom binary on the frame. Static musl, no runtime deps, no allocator
//! surprises. Everything that is not the browser lives here so the shell layer stays
//! busybox + OpenRC only.
//!
//! Subcommands (see TASKS.md for which are done):
//!   tfctl kiosk      run cog on DRM, restart on crash, run the agent thread (inittab respawn)
//!   tfctl boot       per-boot setup from S90thinframe: provision, grow /data, hostname, USB DHCP
//!   tfctl boot fsck  from inittab before `mount -a`: e2fsck the data partition
//!   tfctl agent      control socket to the server alone (debugging; kiosk commands are logged)
//!   tfctl update [--force]  software update from the server's config (the agent does this itself)
//!   tfctl provision [--boot-dir D]  first boot: apply /boot/provision.txt, delete it (TASK 14)
//!   tfctl splash [--png FILE] [STATUS]  draw the boot splash on the DRM card that has a panel, hold it until SIGTERM (TASK 15)
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
mod b64;
mod bench;
mod boot;
mod config;
mod dawn;
mod dawnpage;
mod dbg;
mod drm;
mod font;
mod http;
mod json;
mod kiosk;
mod logmirror;
mod pair;
mod png;
mod provision;
mod report;
mod retro;
mod screen;
mod sdf;
mod seventies;
mod sha1;
mod sha256;
mod slot;
mod splash;
mod status;
mod statuspage;
mod sys;
mod sysupdate;
mod touchfw;
mod update;
mod watchdog;
mod wifi;
mod wifisetup;
mod ws;

#[cfg(test)]
mod fuzz;

fn usage() -> ExitCode {
    eprintln!(
        "usage: tfctl <kiosk|boot [fsck]|agent|update [--force]|provision [--boot-dir D]|splash [--png FILE] [STATUS]|\
         wifi SSID PSK|status|bench [TAPS]|set KEY VALUE>"
    );
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
    // `boot fsck` runs from inittab before /data (where /etc/thinframe.conf points) is mounted;
    // it needs no config, so a config error must not stop the check or the device links.
    if args.first().is_some_and(|a| a == "boot") && args.get(1).is_some_and(|a| a == "fsck") {
        if let Err(e) = boot::fsck_data() {
            eprintln!("tfctl: boot: fsck: {e}");
            return ExitCode::from(1);
        }
        return ExitCode::SUCCESS;
    }
    let cfg = match config::load(&conf) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("tfctl: config: {e}");
            return ExitCode::from(1);
        }
    };
    // The zone the server last sent (agent on_hello), for this process and everything it
    // starts (busybox date for the clock, cog). Still one thread here, so set_var is sound.
    if agent::valid_tz(&cfg.tz) && env::var_os("TZ").is_none() {
        env::set_var("TZ", &cfg.tz);
    }
    dbg::set(cfg.debug);
    let result = match args.first().map(String::as_str) {
        Some("kiosk") => kiosk::run(&cfg),
        Some("boot") => boot::run(&conf),
        Some("agent") => agent_alone(&cfg),
        Some("update") => update::run(&cfg, args.get(1).is_some_and(|a| a == "--force")),
        Some("provision") => provision::run(&args[1..]),
        Some("splash") => splash::run(&cfg, args.get(1..).unwrap_or_default()),
        Some("wifi") if args.len() == 3 => wifi::run(&args[1], &args[2]),
        Some("status") => status::run(&cfg),
        Some("bench") => bench::run(&cfg, &args[1..]),
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
