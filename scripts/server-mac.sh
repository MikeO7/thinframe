#!/usr/bin/env bash
# Run frameserver on this Mac as a login service (launchd), so frames always find it.
#   scripts/server-mac.sh install    build from this checkout, write the service, start it
#   scripts/server-mac.sh status     is it running, what it logs, which frames it knows
#   scripts/server-mac.sh restart | logs | uninstall
# State in ~/.thinframe (mode 700): frameserver, env (FRAME_TOKEN, made once, mode 600), data/,
# artifacts/ (what frames update from), server.log. Discovery (UDP 41007) needs no setup: a
# native process sees the LAN's broadcasts. No sudo.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
STATE="${THINFRAME_STATE:-$HOME/.thinframe}"
LABEL=com.thinframe.server
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
PORT="${PORT:-41007}"
B=$'\033[1m'; G=$'\033[32m'; N=$'\033[0m'
ok()   { printf '  %s✓%s %s\n' "$G" "$N" "$*"; }
stop() { printf '  \033[31m✗ %s\033[0m\n' "$*" >&2; exit 1; }
[ "$(uname)" = Darwin ] || stop "macOS only (on Linux run the container: docker compose up -d)"

token() { sed -n 's/^FRAME_TOKEN=//p' "$STATE/env"; }

build() {
	command -v go >/dev/null || stop "Go is needed to build the server (brew install go)"
	(cd "$ROOT/server" && go build -trimpath -o "$STATE/frameserver.new" ./cmd/frameserver)
	mv -f "$STATE/frameserver.new" "$STATE/frameserver"
	ok "built frameserver from $(git -C "$ROOT" rev-parse --short HEAD)"
}

write_plist() {
	mkdir -p "$(dirname "$PLIST")"
	cat > "$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
	<key>Label</key><string>$LABEL</string>
	<key>ProgramArguments</key><array>
		<string>/bin/sh</string><string>-c</string>
		<string>set -a; . "$STATE/env"; exec "$STATE/frameserver"</string>
	</array>
	<key>EnvironmentVariables</key><dict>
		<key>PORT</key><string>$PORT</string>
		<key>DATA_DIR</key><string>$STATE/data</string>
		<key>ARTIFACTS_DIR</key><string>$STATE/artifacts</string>
	</dict>
	<key>RunAtLoad</key><true/>
	<key>KeepAlive</key><true/>
	<key>ThrottleInterval</key><integer>5</integer>
	<key>StandardOutPath</key><string>$STATE/server.log</string>
	<key>StandardErrorPath</key><string>$STATE/server.log</string>
</dict></plist>
EOF
	plutil -lint -s "$PLIST" || stop "bad plist"
}

start() {
	launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true
	# a server started by hand (nohup) would hold the port
	pkill -f "^$STATE/frameserver\$" 2>/dev/null || true
	launchctl bootstrap "gui/$(id -u)" "$PLIST"
	for _ in $(seq 20); do curl -fsS -o /dev/null "http://127.0.0.1:$PORT/healthz" 2>/dev/null && break; sleep 0.5; done
	curl -fsS -o /dev/null "http://127.0.0.1:$PORT/healthz" || stop "the server did not answer; see $STATE/server.log"
	ok "running on port $PORT (starts at login, restarts if it stops)"
}

install() {
	printf '\n%sthinframe server on this Mac%s\n' "$B" "$N"
	mkdir -p "$STATE/data" "$STATE/artifacts"; chmod 700 "$STATE"
	if [ ! -f "$STATE/env" ]; then
		(umask 077; printf 'FRAME_TOKEN=%s\n' "$(openssl rand -hex 32)" > "$STATE/env")
		ok "made an admin token in $STATE/env"
	fi
	build; write_plist; start
	local ip; ip=$(ipconfig getifaddr en0 2>/dev/null || ipconfig getifaddr en1 2>/dev/null || echo localhost)
	printf '\n  Admin page  http://%s:%s/admin  (any user name, password = the token)\n' "$ip" "$PORT"
	printf '  Token       %s\n' "$(token)"
	printf '  Keep this Mac awake while frames use it: System Settings > Energy > prevent sleep.\n\n'
}

status() {
	if launchctl print "gui/$(id -u)/$LABEL" >/dev/null 2>&1; then ok "service loaded"; else stop "not installed (scripts/server-mac.sh install)"; fi
	curl -fsS -o /dev/null "http://127.0.0.1:$PORT/healthz" || stop "not answering on port $PORT"
	ok "answering on port $PORT"
	curl -fsS -H "Authorization: Bearer $(token)" "http://127.0.0.1:$PORT/api/frames" |
		python3 -c 'import json,sys
for f in json.load(sys.stdin):
    hb = f.get("last_heartbeat") or {}
    print("  frame %-12s %-7s ip %-15s system %s" % (f["id"], "online" if f.get("online") else "offline", hb.get("ip") or "-", hb.get("system_version") or "-"))'
	tail -3 "$STATE/server.log"
}

case "${1:-}" in
	install) install ;;
	status) status ;;
	restart) build; start ;;
	logs) tail -f "$STATE/server.log" ;;
	uninstall) launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true; rm -f "$PLIST"; ok "service removed ($STATE kept)" ;;
	*) sed -n '2,7p' "$0"; exit 1 ;;
esac
