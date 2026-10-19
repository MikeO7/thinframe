// frameserver: everything smart lives here, in a container on real hardware.
//
// The frame is a Wi-Fi touch screen. It does two things:
//  1. cog loads  GET /frame/{id}            (the dashboard page for that frame)
//  2. tfctl agent polls GET /api/frames/{id}/config and POSTs /api/frames/{id}/heartbeat
//
// Routes (stdlib ServeMux, Go 1.22+ patterns):
//
//	GET  /frame/{id}                  dashboard HTML from static/ with per-frame data injected
//	GET  /api/frames                  known frames + last heartbeat
//	GET  /api/frames/{id}/config      {url, rotation, brightness, page_version, software_version, system_version, action}
//	PUT  /api/frames/{id}/config      admin/MCP sets fields; action is one-shot, cleared on ack
//	POST /api/frames/{id}/heartbeat   {uptime_s, mem_used_mb, ip, wifi_rssi, temp_c, tfctl_version, system_version, slot, page_url, latency_ms}
//	GET  /api/frames/{id}/ws          control WebSocket: server pushes config/action, frame sends hello/heartbeat/ack
//	GET  /api/stream/{id}             SSE: live data for the dashboard page (data plane, separate from control)
//	GET  /api/artifacts/{name}        tfctl-armv7, overlay.tar.gz, system-<ver>.tar (+ .sha256)
//	GET  /api/data/...                adapters the dashboard needs (photos, weather, calendar)
//	GET  /admin                       plain HTML table + edit form, no framework
//	POST /mcp                         MCP streamable HTTP (internal/mcp), same handlers as REST
//
// State: JSON files under /data/frames/{id}.json (internal/store). No database.
// Auth: FRAME_TOKEN env; when set, every /api and /mcp request needs "Authorization: Bearer".
package main

import (
	"fmt"
	"os"
)

func main() {
	fmt.Fprintln(os.Stderr, "frameserver: not implemented (TASK S1). See server/README.md")
	os.Exit(1)
}
