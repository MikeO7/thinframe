#!/usr/bin/env bash
# End-to-end without Docker: real frameserver + real `tfctl agent` on this machine.
# Asserts: the frame registers, a rotation PUT reaches the agent and is persisted to its conf,
# an action is acked, and the hello reply carried the clock/TZ. The screenshot assertion
# (M4) needs the sim container and lives in tests/e2e.sh.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PORT="${PORT:-18080}"; TOKEN=e2e-token; ID=e2e
TMP="$(mktemp -d)"; PIDS=()
# Wait for each process: a frameserver writes frame files while it shuts down, racing rm -rf.
cleanup() { for p in "${PIDS[@]:-}"; do kill "$p" 2>/dev/null || true; done; for p in "${PIDS[@]:-}"; do wait "$p" 2>/dev/null || true; done; rm -rf "$TMP"; }
trap cleanup EXIT
fail() { echo "FAIL: $*" >&2; tail -n 20 "$TMP"/*.log >&2 || true; exit 1; }
api() { curl -fsS -H "Authorization: Bearer $TOKEN" "$@"; }
# serve PORT DATA ARTIFACTS LOG: start a frameserver on the first free port from PORT (10 tries),
# set $SP to it, and prove that this process (not some other listener) answers /healthz.
serve() {
	local p pid rc
	for p in $(seq "$1" $(($1 + 9))); do
		rc=0; curl -s -o /dev/null --max-time 1 "http://127.0.0.1:$p/" || rc=$?
		[ "$rc" -eq 7 ] || continue	# anything but "connection refused": something listens there
		PORT=$p DATA_DIR="$2" ARTIFACTS_DIR="$3" FRAME_TOKEN=$TOKEN "$TMP/frameserver" >"$4" 2>&1 &
		pid=$!; PIDS+=("$pid")
		for _ in $(seq 50); do curl -fsS "http://127.0.0.1:$p/healthz" >/dev/null 2>&1 && break; sleep 0.1; done
		sleep 0.2
		kill -0 "$pid" 2>/dev/null || fail "frameserver on :$p exited (port taken?)"
		curl -fsS "http://127.0.0.1:$p/healthz" >/dev/null || fail "frameserver on :$p never answered /healthz"
		SP=$p; return
	done
	fail "no free port in $1..$(($1 + 9))"
}

(cd "$ROOT/server" && go build -o "$TMP/frameserver" ./cmd/frameserver)
(cd "$ROOT/tfctl" && cargo build -q --release --target-dir "$TMP/target")

serve "$PORT" "$TMP/data" "$TMP/artifacts" "$TMP/server.log"; PORT=$SP

cat >"$TMP/frame.conf" <<CONF
SERVER_URL="http://127.0.0.1:$PORT"
FRAME_ID="$ID"
FRAME_TOKEN="$TOKEN"
COG_ROTATION="3"
CONF
TFCTL_CONF="$TMP/frame.conf" "$TMP/target/release/tfctl" agent >"$TMP/agent.log" 2>&1 & PIDS+=($!)

online=""
for _ in $(seq 100); do
	if api "http://127.0.0.1:$PORT/api/frames" | grep -q "\"$ID\""; then online=1; break; fi
	sleep 0.1
done
[ -n "$online" ] || fail "frame never registered"

api -X PUT -H 'Content-Type: application/json' -d '{"rotation":1}' \
	"http://127.0.0.1:$PORT/api/frames/$ID/config" >/dev/null
for _ in $(seq 100); do grep -q 'COG_ROTATION="\?1' "$TMP/frame.conf" && break; sleep 0.1; done
grep -q 'COG_ROTATION="\?1' "$TMP/frame.conf" || fail "rotation not persisted by the agent"

api -X POST -H 'Content-Type: application/json' -d '{"name":"screen","args":{"state":"off"}}' \
	"http://127.0.0.1:$PORT/api/frames/$ID/actions" >/dev/null
for _ in $(seq 100); do
	api "http://127.0.0.1:$PORT/api/frames/$ID" | grep -q '"acked_at"' && break; sleep 0.1
done
api "http://127.0.0.1:$PORT/api/frames/$ID" | grep -q '"acked_at"' || fail "action never acked"

# Pairing: a frame without a token shows a code on its page; nothing pairs until the owner types
# that code on /admin, then the frame gets its own token. (Own dir: the token file sits next to
# the conf.)
PD="$TMP/pairframe"; mkdir -p "$PD"
printf 'SERVER_URL="http://127.0.0.1:%s"\nFRAME_ID="e2e-pair"\n' "$PORT" >"$PD/thinframe.conf"
TFCTL_CONF="$PD/thinframe.conf" TFCTL_STATUS_PAGE="$PD/status.html" "$TMP/target/release/tfctl" agent \
	>"$TMP/agent-pair.log" 2>&1 & PIDS+=($!)
code=""
for _ in $(seq 100); do
	code="$(sed -n 's/.*<div class="code">\([0-9]\{3\}\) \([0-9]\{3\}\)<.*/\1\2/p' "$PD/pair.html" 2>/dev/null || true)"
	[ -n "$code" ] && break; sleep 0.1
done
[ -n "$code" ] || fail "the unpaired frame shows no pairing code"
sleep 3 # a few polls while nobody has typed the code
api "http://127.0.0.1:$PORT/api/frames" | grep -q '"e2e-pair"' && fail "a frame record exists before the code was typed"
[ -s "$PD/thinframe.token" ] && fail "the frame got a token before the code was typed"
wrong=$(( (10#$code + 1) % 1000000 )); wrong=$(printf '%06d' "$wrong")
curl -fsS -o /dev/null -w '%{redirect_url}' -u "admin:$TOKEN" -d "code=$wrong" \
	"http://127.0.0.1:$PORT/admin/e2e-pair/pair" | grep -q 'err=' || fail "a wrong code was not refused"
curl -fsS -o /dev/null -u "admin:$TOKEN" -d "code=$code" "http://127.0.0.1:$PORT/admin/e2e-pair/pair" || fail "typing the code"
for _ in $(seq 100); do [ -s "$PD/thinframe.token" ] && break; sleep 0.1; done
[ -s "$PD/thinframe.token" ] || fail "the frame never picked its token up"
for _ in $(seq 100); do api "http://127.0.0.1:$PORT/api/frames/e2e-pair" 2>/dev/null | grep -q '"ip"' && break; sleep 0.1; done
api "http://127.0.0.1:$PORT/api/frames/e2e-pair" | grep -q '"ip"' || fail "the paired frame never sent a heartbeat"
# Artifacts: a second server (no agent, so nothing tries to self-update) carrying staged files.
"$ROOT/scripts/make-artifacts.sh" v-e2e "$TMP/target/release/tfctl" "" "$TMP/staged" >/dev/null
serve $((PORT + 1)) "$TMP/data2" "$TMP/staged" "$TMP/server2.log"; P2=$SP
api -X PUT -H 'Content-Type: application/json' -d '{"rotation":0}' "http://127.0.0.1:$P2/api/frames/a/config" \
	| grep -q '"software_version":"v-e2e"' || fail "config lacks software_version"
want="$({ sha256sum "$TMP/target/release/tfctl" 2>/dev/null || shasum -a 256 "$TMP/target/release/tfctl"; } | cut -d' ' -f1)"
got="$(api "http://127.0.0.1:$P2/api/artifacts/tfctl-armv7.sha256" | cut -d' ' -f1)"
[ "$want" = "$got" ] || fail "artifact sha256 mismatch"
echo "e2e-native: ok"
