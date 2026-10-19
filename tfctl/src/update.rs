//! Pull main as a tarball (no git on the frame) and apply it.
//
// TODO(TASK 8):
//  - download cfg.repo_tarball to /tmp with busybox `wget -O` (spawn; no TLS crate needed)
//  - `tar -xzf` to /tmp/thinframe-main
//  - copy image/rootfs/ over / excluding etc/thinframe.conf, etc/ssh/, etc/iwd/
//  - replace /var/lib/thinframe/www/ with www/
//  - copy prebuilt tfctl from the GitHub release if present, else keep current
//  - `rc-service thinframe restart`
//  - append a line to /var/log/tf-update.log
use crate::config::Config;
use std::io;

pub fn run(_cfg: &Config) -> io::Result<()> {
    Err(io::Error::other("update: not implemented (TASK 8)"))
}
