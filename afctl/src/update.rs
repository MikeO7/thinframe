//! Pull main as a tarball (no git on the frame) and apply it.
//
// TODO(TASK 8):
//  - download cfg.repo_tarball to /tmp with busybox `wget -O` (spawn; no TLS crate needed)
//  - `tar -xzf` to /tmp/alpineframe-main
//  - copy image/rootfs/ over / excluding etc/alpineframe.conf, etc/ssh/, etc/iwd/
//  - replace /var/lib/alpineframe/www/ with www/
//  - copy prebuilt afctl from the GitHub release if present, else keep current
//  - `rc-service alpineframe restart`
//  - append a line to /var/log/af-update.log
use crate::config::Config;
use std::io;

pub fn run(_cfg: &Config) -> io::Result<()> {
    Err(io::Error::other("update: not implemented (TASK 8)"))
}
