#!/usr/bin/env bash
# Fast iteration on a connected frame: build tfctl for armv7, stage it with the rootfs overlay
# as a dev software version in the server's artifacts directory, and ask the frame to update now.
#   scripts/dev-push.sh [ARTIFACTS_DIR]     default $THINFRAME_ARTIFACTS or ~/.thinframe/artifacts
# The server reads versions.json on every config request, so no restart. With SERVER_URL,
# FRAME_ID and FRAME_TOKEN set, the update-software action goes out at once; otherwise the frame
# takes the new version on its next config poll. tfctl swaps its binary and init respawns it; a
# build that crash-loops twice is rolled back by tfctl itself (update.rs). Mac side only.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
out="${1:-${THINFRAME_ARTIFACTS:-$HOME/.thinframe/artifacts}}"
target=armv7-unknown-linux-musleabihf
limit=2097152

# cargo lives in the rustup toolchain, not on PATH (CLAUDE.md).
for d in "$HOME/.cargo/bin" "$HOME"/.rustup/toolchains/stable-*/bin; do
	[ -d "$d" ] && PATH="$d:$PATH"
done
export PATH

echo "== building tfctl ($target)"
(cd "$root/tfctl" && cargo zigbuild --release --target "$target" --quiet)
bin="$root/tfctl/target/$target/release/tfctl"
size=$(stat -f %z "$bin" 2>/dev/null || stat -c %s "$bin")
if [ "$size" -ge "$limit" ]; then
	echo "tfctl is $size bytes, limit $limit: not pushing" >&2
	exit 1
fi
echo "   $size bytes"

ver="dev-$(date -u +%Y%m%d%H%M%S)-$(git -C "$root" rev-parse --short HEAD)"
echo "== staging $ver in $out"
"$root/scripts/make-artifacts.sh" "$ver" "$bin" "" "$out" >/dev/null
echo "   versions.json: $(cat "$out/versions.json")"

if [ -n "${SERVER_URL:-}" ] && [ -n "${FRAME_ID:-}" ]; then
	echo "== update-software -> $FRAME_ID at $SERVER_URL"
	curl -fsS -H "Authorization: Bearer ${FRAME_TOKEN:-}" -H 'Content-Type: application/json' \
		-d '{"name":"update-software"}' "${SERVER_URL%/}/api/frames/$FRAME_ID/actions"
	echo
	echo "   watch: ${SERVER_URL%/}/admin (events: software-updated, then a new heartbeat with tfctl $ver)"
else
	echo "   no SERVER_URL/FRAME_ID: the frame updates on its next config poll"
fi
