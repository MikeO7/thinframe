//! Software and system updates. Stubs for now; the agent calls these for the
//! `update-software`, `update-system` and `rollback` actions and acks the error.
//
// TODO(TASK 8, `tfctl update`): download cfg.repo_tarball with busybox `wget -O`, `tar -xzf`,
//   copy image/rootfs/ over / excluding etc/thinframe.conf, etc/ssh/, etc/iwd/, replace
//   /var/lib/thinframe/www/, keep tfctl.prev, `rc-service thinframe restart`, log to
//   /var/log/tf-update.log.
// TODO(TASK 11, software): compare config.software_version with /etc/thinframe.version, fetch
//   tfctl-armv7 + overlay.tar.gz from /api/artifacts/, verify sha256, install, restart.
// TODO(TASK 12, system): stream rootfs.ext4.zst to the standby slot, copy zImage.<slot>, verify,
//   set slot/tries in uEnv.txt, reboot. First heartbeat after boot marks the slot good.
use crate::config::Config;
use std::io;

fn todo(what: &str) -> io::Error {
    eprintln!("tfctl: update: {what} requested but not implemented yet");
    io::Error::other(format!("{what}: not implemented"))
}

pub fn run(_cfg: &Config) -> io::Result<()> {
    Err(todo("update (TASK 8)"))
}

/// TASK 11: tfctl + overlay from the server's /api/artifacts/.
pub fn software(_cfg: &Config, _version: Option<&str>) -> io::Result<()> {
    Err(todo("update-software (TASK 11)"))
}

/// TASK 12: A/B root slot swap.
pub fn system(_cfg: &Config, _version: Option<&str>) -> io::Result<()> {
    Err(todo("update-system (TASK 12)"))
}

/// TASK 12: boot the other slot.
pub fn rollback(_cfg: &Config) -> io::Result<()> {
    Err(todo("rollback (TASK 12)"))
}
