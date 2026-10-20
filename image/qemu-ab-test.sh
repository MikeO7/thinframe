#!/usr/bin/env bash
# shellcheck disable=SC2329,SC2119  # helpers run via check(); start_qemu takes optional QEMU flags
# A/B system update and rollback in QEMU (TASK 12 gate), driven only by the server:
#   A (the image as built) -> B -> BAD (no init: the kernel panics, panic=10 reboots, U-Boot
#   spends the tries and flips back to B) -> C. No manual step touches the frame.
# Bundles B, BAD, C are the built system tar with /etc/thinframe.system-version rewritten
# (BAD also loses /sbin/init and /bin/sh). Result: image/out/qemu-ab-test.txt.
set -euo pipefail
RES="$(cd "$(dirname "$0")" && pwd)/out/qemu-ab-test.txt"
# shellcheck source=SCRIPTDIR/qemu-lib.sh
. "$(dirname "$0")/qemu-lib.sh"
A="$(cat "$HERE/out/system-version" 2>/dev/null || true)"
[ -n "$A" ] && [ -f "$HERE/out/system-$A.tar" ] || { echo "STOP: no image/out/system-<ver>.tar; run image/build.sh rootfs" >&2; exit 1; }
B="$A-b"; BAD="$A-bad"; C="$A-c"
TFCTL="$HERE/work/tfctl/tfctl"

echo "== bundles $B $BAD $C from system-$A.tar"
rm -rf "$W/bundles" && mkdir -p "$W/bundles"
cp "$HERE/out/system-$A.tar" "$W/bundles/"
docker run --rm -v thinframe-br:/br -v "$W/bundles":/b -e A="$A" -e B="$B" -e BAD="$BAD" -e C="$C" \
	thinframe-br bash -euo pipefail -c '
	DFS=$(ls /br/output/host/sbin/debugfs)
	mk() {	# version [break]
		d=/b/$1; mkdir -p $d && tar -xf /b/system-$A.tar -C $d
		zstd -q -d $d/rootfs.ext4.zst -o $d/rootfs.ext4 && rm $d/rootfs.ext4.zst
		echo "$1" > $d/ver
		$DFS -w -R "rm /etc/thinframe.system-version" $d/rootfs.ext4 >/dev/null 2>&1
		$DFS -w -R "write $d/ver /etc/thinframe.system-version" $d/rootfs.ext4 >/dev/null 2>&1
		[ -z "${2:-}" ] || for f in /sbin/init /bin/sh; do $DFS -w -R "rm $f" $d/rootfs.ext4 >/dev/null 2>&1; done
		zstd -q -19 -T0 $d/rootfs.ext4 -o $d/rootfs.ext4.zst && rm $d/rootfs.ext4 $d/ver
		(cd $d && sha256sum zImage rootfs.ext4.zst > sha256 && printf "{\"version\":\"%s\"}\n" "$1" > version.json)
		tar -C $d -cf /b/system-$1.tar zImage rootfs.ext4.zst sha256 version.json && rm -rf $d
	}
	mk $B; mk $BAD broken; mk $C'
# Software version stays $A (what the image carries), so only the system layer moves.
for v in "$B" "$BAD" "$C"; do
	"$ROOT/scripts/make-artifacts.sh" "$A" "$TFCTL" "$W/bundles/system-$v.tar" "$W/art-$v" >/dev/null
done

frame() { api "/api/frames/$ID"; }
on() {	# version slot: the last heartbeat reports both
	frame | tr -d ' \n' | grep -q "\"last_heartbeat\":{[^}]*\"system_version\":\"$1\"" &&
		frame | tr -d ' \n' | grep -q "\"last_heartbeat\":{[^}]*\"slot\":\"$2\""
}
event() { frame | grep -q "\"frame:$1\""; }	# the server prefixes frame events

prepare_image
start_server
start_qemu
check "boot 1: A on slot a reports to the server" wait_for 1200 on "$A" a

start_server "$W/art-$B"
check "B written to slot b, rebooted, B reports from slot b" wait_for 1800 on "$B" b
check "U-Boot booted b with tries pending" grep -q -a 'thinframe: slot b, tries left 2' "$W/serial.log"
check "slot b confirmed by the agent (event)" wait_for 120 event slot-confirmed

start_server "$W/art-$BAD"
check "BAD written to slot a and the board rebooted into it" wait_for 1800 grep -q -a 'thinframe: slot a, tries left 2' "$W/serial.log"
check "BAD kernel panics (no init)" wait_for 600 grep -q -a 'No working init found' "$W/serial.log"
check "U-Boot spent the last try and flipped back to slot b" wait_for 900 grep -q -a 'thinframe: slot b, tries left 0' "$W/serial.log"
check "rolled back: B reports from slot b again" wait_for 1200 on "$B" b
check "agent refuses to flash BAD again (event)" wait_for 300 event system-update-skipped

start_server "$W/art-$C"
check "C written to slot a, rebooted, C reports from slot a" wait_for 1800 on "$C" a
grep -a -E 'thinframe: slot|No working init|tfctl: (agent|update)' "$W/serial.log" >> "$RES" || true
[ "$fail" = 0 ] && echo "qemu-ab-test: $A -> $B -> $BAD (rolled back) -> $C passed" | tee -a "$RES"
exit "$fail"
