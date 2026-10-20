#!/usr/bin/env bash
# M4: both containers from compose.yml (frameserver + simframe: real WPE WebKit, real tfctl agent).
# Asserts: the sim frame registers, it writes screenshots, set_frame_config over MCP reaches the
# frame, and the next screenshot changes (rotation 0 -> 3 swaps the viewport). Docker required.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export PORT="${PORT:-18088}" FRAME_TOKEN=e2e-token
TMP="$(mktemp -d)"; export DATA_DIR="$TMP/data" SIM_OUT="$TMP/out"
mkdir -p "$DATA_DIR" "$SIM_OUT"
DC=(docker compose -p thinframe-e2e -f "$ROOT/compose.yml" --profile sim)
BASE="http://127.0.0.1:$PORT"; SHOT="$SIM_OUT/frame.png"
cleanup() { "${DC[@]}" down -v >/dev/null 2>&1 || true; rm -rf "$TMP"; }
trap cleanup EXIT
fail() { echo "FAIL: $*" >&2; "${DC[@]}" logs --tail 30 >&2 || true; exit 1; }
api() { curl -fsS -H "Authorization: Bearer $FRAME_TOKEN" "$@"; }
mcp() {	# tool json-args
	api -X POST -H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' \
		-d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" "$BASE/mcp"
}
wait_for() {	# seconds cmd...
	local t=$1; shift
	for _ in $(seq "$t"); do "$@" >/dev/null 2>&1 && return 0; sleep 1; done
	return 1
}
dims() { local h; h=$(od -An -tx1 -j16 -N8 "$SHOT" | tr -d ' \n'); echo "$((16#${h:0:8}))x$((16#${h:8:8}))"; }
online() { api "$BASE/api/frames" | grep -q '"id":"sim"'; }

echo "== build and start (first build pulls Debian WPE packages)"
"${DC[@]}" up -d --build >"$TMP/up.log" 2>&1 || { cat "$TMP/up.log" >&2; fail "compose up"; }
wait_for 120 curl -fsS "$BASE/healthz" || fail "frameserver never answered on :$PORT"
wait_for 180 online || fail "sim frame never came online"
echo "ok   sim frame online"

mcp set_frame_config '{"id":"sim","rotation":0}' | grep -q '"rotation":0' || fail "MCP set rotation 0"
rm -f "$SHOT"; wait_for 120 test -s "$SHOT" || fail "no screenshot"
sleep 6; before=$(dims); sum_before=$(shasum -a 256 "$SHOT" | cut -c1-64)
echo "ok   screenshot at rotation 0: $before"

out=$(mcp set_frame_config '{"id":"sim","rotation":3}') || fail "MCP call"
echo "$out" | grep -q '"rotation":3' || fail "MCP set_frame_config did not return rotation 3: $out"
echo "ok   MCP set_frame_config rotation=3"
changed() { [ "$(dims)" != "$before" ] && [ "$(shasum -a 256 "$SHOT" | cut -c1-64)" != "$sum_before" ]; }
wait_for 60 changed || fail "screenshot did not change after rotation 3 (still $before)"
echo "ok   screenshot changed: $before -> $(dims)"
echo "e2e: ok"
