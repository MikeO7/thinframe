#!/usr/bin/env bash
# Stage the frame software the server carries (TASK S6): tfctl-armv7, overlay.tar.gz, optional
# system-<ver>.tar, and versions.json, into DIR (default server/artifacts, baked into the image).
#   scripts/make-artifacts.sh VERSION TFCTL_BINARY [SYSTEM_TAR] [DIR]
set -euo pipefail

ver="${1:?usage: make-artifacts.sh VERSION TFCTL_BINARY [SYSTEM_TAR] [DIR]}"
bin="${2:?missing tfctl binary}"
sys="${3:-}"
root="$(cd "$(dirname "$0")/.." && pwd)"
out="${4:-$root/server/artifacts}"

[ -f "$bin" ] || { echo "no such binary: $bin" >&2; exit 1; }
mkdir -p "$out"
install -m 0755 "$bin" "$out/tfctl-armv7"
tar -C "$root/image/rootfs" --owner=0 --group=0 -czf "$out/overlay.tar.gz" .

files='"tfctl":"tfctl-armv7","overlay":"overlay.tar.gz"'
system=""
if [ -n "$sys" ]; then
	[ -f "$sys" ] || { echo "no such system bundle: $sys" >&2; exit 1; }
	cp "$sys" "$out/$(basename "$sys")"
	files="$files,\"system\":\"$(basename "$sys")\""
	system="$(basename "$sys" .tar)"; system="${system#system-}"
fi
printf '{"software":"%s","system":"%s","files":{%s}}\n' "$ver" "$system" "$files" > "$out/versions.json"
echo "artifacts in $out:"; ls -l "$out"
