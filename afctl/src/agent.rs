//! Thin-client loop: poll the server for config, post heartbeats, act on one-shot actions.
//
// TASK 10 (frame side of the thin client):
//  - every cfg.poll_interval_s: GET {server_url}/api/frames/{frame_id}/config (busybox wget -qO-
//    spawned, or a 40-line HTTP/1.1 client over std::net::TcpStream; no TLS on the LAN)
//  - if url/rotation changed: rewrite /etc/alpineframe.conf and restart cog (signal to kiosk loop)
//  - action "reboot" -> `reboot`; "update" -> update::run; "restart-kiosk" -> kill cog
//  - POST heartbeat JSON built by status.rs (hand-rolled JSON, no serde)
//  - if the server is unreachable for 3 polls, switch cog to FALLBACK_URL (local file) and keep polling;
//    switch back when it answers. The dashboard page itself should also work offline (cache headers).
// Runs as a second thread inside `afctl kiosk`, not a separate process.
use crate::config::Config;
use std::io;

pub fn run(_cfg: &Config) -> io::Result<()> {
    Err(io::Error::other("agent: not implemented (TASK 10)"))
}
