#!/usr/bin/env bash
# shellcheck disable=SC2329,SC2119  # helpers run via check(); start_qemu takes optional QEMU flags
# The owner's whole journey on the real image (QEMU=1 build) under -M virt, driven like a person:
# a fresh card (server URL only, as flash.sh writes it with no Wi-Fi typed), the Wi-Fi page, the
# on-screen keyboard, PAIR, the dashboard; then MCP (rotation, layout, reload), a system update,
# a rollback by MCP that sticks, and a reboot that comes back alone. Taps are QMP multitouch at
# panel pixels; the pages are landscape (rotation 3), so page x,y is panel 799-y,x.
# Proof is never "the screen changed": cog's own "Loaded successfully" lines, the dashboard's
# /alive pings (its JS runs), cog's command line, the server's records. Screenshots are the record.
# Result: image/out/qemu/qemu-journey-test.txt, screenshots in image/out/qemu/journey/.
set -euo pipefail
export BOARD=virt SCREEN=1 FRESH=1
# shellcheck source=SCRIPTDIR/qemu-lib.sh
. "$(dirname "$0")/qemu-lib.sh"
RES="$OUT/qemu-journey-test.txt"; SHOTS="$OUT/journey"
ROW0="${ROW0:-231}"; ROWH="${ROWH:-62}"	# first network row, row pitch (page px)
A="$(built_version)"; B="$A-b"

# shellcheck source=SCRIPTDIR/qemu-screen.sh
. "$(dirname "$0")/qemu-screen.sh"
log_has() { r "grep -q -F '$1' /var/log/thinframe.log"; }
loaded() { r "grep -F 'Loaded successfully' /var/log/thinframe.log | grep -q -F '$1'"; }	# cog's own line, URL part
cog_has() { r "ps | grep -v grep | grep -q -e '^ *[0-9]* root *cog .*$1'"; }
srv_has() { grep -q -F "$1" "$W/server.log"; }
count() { grep -c -a -F "$1" "$2" || true; }
more() { [ "$(count "$1" "$2")" -gt "$3" ]; }	# needle file n: seen more than n times
alive() { count "/api/frames/$ID/alive" "$W/server.log"; }
page_up() { wait_for "${2:-900}" more "/api/frames/$ID/alive" "$W/server.log" "$1"; }	# a new ping since $1
mcp() {	# tool json-args: one stateless MCP 2026-07-28 call, as Claude Code or Hermes would
	local m='"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientCapabilities":{},"io.modelcontextprotocol/clientInfo":{"name":"journey","version":"1"}}'
	curl -fsS -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
		-H 'Accept: application/json, text/event-stream' -H 'MCP-Protocol-Version: 2026-07-28' \
		-H 'Mcp-Method: tools/call' -H "Mcp-Name: $1" "http://127.0.0.1:$PORT/mcp" \
		-d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2,$m}}"
}
mcp_ok() { local o; o="$(mcp "$@")" && grep -q '"resultType":"complete"' <<<"$o" && ! grep -q '"isError":true' <<<"$o"; }
mcp_online() { mcp list_frames '{}' | grep -q '"online":true'; }
listed() { r 'wget -qO- http://127.0.0.1:8091/wifi' | grep -q 'Groovy Wifi'; }
acked() { frame | tr -d ' \n' | grep -q "{[^{}]*\"name\":\"$1\"[^{}]*\"ok\":true"; }
no_event() { ! event "$1"; }
rendered() { curl -fsS "http://127.0.0.1:$PORT/frame/$ID" | grep -q "$1"; }
stays() { sleep "$1"; on "$2" "$3"; }
benched() { event bench && frame | tr -d '\n' | grep -o '"frame:bench"[^}]*' | tee -a "$RES"; }
admin_touch() { curl -fsS "http://127.0.0.1:$PORT/admin" | grep -q 'p50 [0-9]* · max'; }
# README budget: nothing runs but these (kernel threads aside; sh/ps are this ssh session;
# hostapd is the QEMU image's simulated access point).
OK_PROCS='hostapd|init|getty|dropbear|iwd|dbus-daemon|dhcpcd|udevd|udhcpd|tfctl|cog|WPEWebProcess|WPENetworkProcess|sh|ps|grep'
procs() {	# kernel threads are [name] in args; the rest by program name
	r 'ps -o args' | sed 1d | grep -v '^\[' | awk '{ sub(".*/", "", $1); sub(":$", "", $1); print $1 }' |
		grep -v -E "^($OK_PROCS)$" | sort -u > "$W/extra-procs.txt" || true
	cat "$W/extra-procs.txt" >&2; [ ! -s "$W/extra-procs.txt" ]
}
mem() { r 'free -m' | tee -a "$RES" | awk '/^Mem:/ { exit !($3 < 300) }'; }
mkdir -p "$SHOTS"; rm -f "$SHOTS"/*.png
echo "== bundle $B from system-$A.tar"
make_bundles "$A" "$B"
prepare_image
: > "$W/server.log"
start_server
start_qemu

check "fresh card: Wi-Fi page first (cog loaded it)" wait_for 1500 loaded "127.0.0.1:8091/wifi"
check "the sim's networks are listed" wait_for 300 listed
check "Wi-Fi list painted" snap 1-wifi-list 300
idx="$(r 'wget -qO- http://127.0.0.1:8091/wifi' | grep -o 'class="name">[^<]*' | grep -n 'Groovy Wifi' | cut -d: -f1)"
tap 300 $(( ROW0 + (${idx:-1} - 1) * ROWH ))
check "row tapped: the password page loads" wait_for 300 loaded "/wifi/net?ssid=Groovy"
check "password page painted" snap 2-password 300
type_text "far out man"
snap 3-typed 60 || true
tap 1110 139
check "joined Groovy Wifi from the screen" wait_for 300 log_has "wifi setup: joined Groovy Wifi"
check "then the frame asks the server to pair" wait_for 600 srv_has "POST /api/pair"
check "PAIR page loads" wait_for 600 loaded "pair.html"
check "PAIR page painted" snap 4-pair 300
a=$(alive); tap 252 543
check "PAIR pressed on the frame: server accepted it" wait_for 300 srv_has "POST /pair/$ID/accept"
check "the dashboard runs in cog (its JS pings /alive)" page_up "$a"
check "heartbeat: A on slot a" wait_for 600 on "$A" a
snap 5-dashboard 300 || true
check "MCP frame_action bench" mcp_ok frame_action "{\"id\":\"$ID\",\"name\":\"bench\"}"
check "bench: uinput taps paint on the page (event)" wait_for 180 benched
check "/admin shows touch p50 and max" admin_touch
check "only the README's processes run" procs
check "under 300 MB used with the page up" mem

check "MCP list_frames sees the frame online" mcp_online
a=$(alive)
check "MCP set_frame_config rotation 1" mcp_ok set_frame_config "{\"id\":\"$ID\",\"rotation\":1}"
check "cog restarted with rotation 1" wait_for 600 cog_has "rotation=1"
check "dashboard back after the turn" page_up "$a"
snap 6-rotation-1 300 || true
a=$(alive); mcp_ok set_frame_config "{\"id\":\"$ID\",\"rotation\":3}" || true
check "MCP rotation 3: cog back in landscape" wait_for 600 cog_has "rotation=3"
check "dashboard back in landscape" page_up "$a"
a=$(alive)
check "MCP set_layout: one text widget" mcp_ok set_layout "{\"id\":\"$ID\",\"layout\":{\"cols\":4,\"rows\":3,\"widgets\":[{\"id\":\"t\",\"type\":\"text\",\"x\":0,\"y\":0,\"w\":4,\"h\":3,\"props\":{\"text\":\"Hello from MCP\"}}]}}"
check "server renders the new layout" rendered "Hello from MCP"
check "the frame reloads it" page_up "$a"
snap 7-layout 300 || true
check "MCP frame_action reload-page" mcp_ok frame_action "{\"id\":\"$ID\",\"name\":\"reload-page\"}"
check "reload-page acked by the frame" wait_for 300 acked reload-page

a=$(alive)
start_server "$W/art-$B"
check "system update offered by the server: B on slot b" wait_for 2400 on "$B" b
check "B confirmed its slot" wait_for 300 event slot-confirmed
check "no false 'skipped' event while B runs" no_event system-update-skipped
check "Wi-Fi and pairing kept: dashboard runs on B" page_up "$a"
snap 8-on-b 300 || true
a=$(alive)
check "MCP frame_action rollback" mcp_ok frame_action "{\"id\":\"$ID\",\"name\":\"rollback\"}"
check "rolled back: A on slot a" wait_for 1800 on "$A" a
check "rollback sticks: B not flashed again (event)" wait_for 600 event system-update-skipped
check "dashboard runs on A again" page_up "$a"
check "still A on slot a three minutes later" stays 180 "$A" a
before=$(count "thinframe: slot" "$W/serial.log"); a=$(alive)
check "MCP frame_action reboot" mcp_ok frame_action "{\"id\":\"$ID\",\"name\":\"reboot\"}"
check "reboot: U-Boot ran again" wait_for 900 more "thinframe: slot" "$W/serial.log" "$before"
check "back online by itself: A on slot a" wait_for 1500 on "$A" a
check "dashboard runs after the reboot" page_up "$a"
snap 9-after-reboot 300 || true
[ "$fail" = 0 ] && echo "qemu-journey-test: fresh card -> Wi-Fi -> PAIR -> dashboard -> MCP -> $B -> rollback -> reboot passed" | tee -a "$RES"
exit "$fail"
