//! frameserver: everything smart lives here, in a container on real hardware.
//
// The frame is a Wi-Fi touch screen. It does two things:
//   1. cog loads  GET /frame/<id>           (the dashboard page for that frame)
//   2. afctl agent polls  GET /api/frames/<id>/config  and posts  /api/frames/<id>/heartbeat
//
// Endpoints (TASK S1..S3):
//   GET  /frame/<id>                 dashboard HTML (static/ + per-frame data injected)
//   GET  /api/frames/<id>/config     {"url":..,"rotation":..,"brightness":..,"action":null|"reboot"|"update"|"restart-kiosk"}
//   POST /api/frames/<id>/heartbeat  {"uptime_s","mem_used_mb","ip","wifi_rssi","temp_c","afctl_version","page_url"}
//   GET  /api/frames                 list of known frames with last heartbeat
//   PUT  /api/frames/<id>/config     admin sets fields; "action" is one-shot, cleared after the frame acks
//   GET  /admin                      static page listing frames + edit form (plain HTML, no framework)
//   GET  /api/data/*                 whatever the dashboard needs (weather, calendar, photos). Lives here, not on the frame.
//
// State: /data/frames/<id>.json. No DB. One container, one volume.
// Auth: none on the LAN by default; FRAME_TOKEN env enables a shared bearer token (TASK S4).

fn main() {
    eprintln!("frameserver: not implemented (TASK S1). See server/README.md");
    std::process::exit(1);
}
