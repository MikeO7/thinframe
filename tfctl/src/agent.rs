//! Thin-client loop: poll the server for config, post heartbeats, act on one-shot actions.
//
// TASK 10 (frame side of the thin client). Control plane = one outbound WebSocket, see
// docs/decisions.md "Two planes".
//  - connect ws://{server_url}/api/frames/{frame_id}/ws with Authorization: Bearer <token>
//    (hand-rolled RFC 6455 client over std::net::TcpStream: HTTP upgrade, masked text frames,
//    ping/pong, close; ~250 lines, no crates). Reconnect with backoff 1..60 s.
//  - send hello on connect, heartbeat every 60 s and on change, ack for every action id
//  - receive config (apply whole object; restart cog only if a cog-affecting field changed) and
//    action (reboot|restart-kiosk|reload-page|update-software|update-system|rollback|screenshot)
//  - while the socket is down: GET /api/frames/{id}/config every 60 s (plain HTTP/1.1 client)
//  - if url/rotation changed: rewrite /etc/thinframe.conf and restart cog (signal to kiosk loop)
//  - action "reboot" -> `reboot`; "update" -> update::run; "restart-kiosk" -> kill cog
//  - POST heartbeat JSON built by status.rs (hand-rolled JSON, no serde)
//  - if the server is unreachable for 3 polls, switch cog to FALLBACK_URL (local file) and keep polling;
//    switch back when it answers. The dashboard page itself should also work offline (cache headers).
//  - TASK 11: software update (tfctl + overlay) when `software_version` changes; keep tfctl.prev
//  - TASK 12: system update when `system_version` changes: standby slot <- rootfs.ext4.zst, zImage.<slot>,
//    uEnv.txt slot/tries, reboot; first heartbeat after boot marks slot good (tries=0)
//  - all requests carry Authorization: Bearer <FRAME_TOKEN> from /etc/thinframe.conf
// Runs as a second thread inside `tfctl kiosk`, not a separate process.
use crate::config::Config;
use std::io;

pub fn run(_cfg: &Config) -> io::Result<()> {
    Err(io::Error::other("agent: not implemented (TASK 10)"))
}
