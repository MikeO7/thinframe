#!/usr/bin/env bash
# M2: the real Buildroot image (QEMU=1 variant: same rootfs, tfctl and cog, plus virt drivers and
# Mesa swrast) under `qemu-system-arm -M virt`. cog draws on virtio-gpu at the panel's native
# 800x1280; screendumps land in sim/out/frame.png every SHOT_INTERVAL_S seconds.
#   sim/qemu/run.sh            boot and keep shooting until Ctrl-C (or RUN_FOR_S seconds)
#   sim/qemu/run.sh tap X Y    touch the running guest at panel pixel X,Y (0..799, 0..1279):
#                              one finger on a virtio multitouch screen, like the frame's panel
# Env: SERVER_URL (as the guest sees it; default http://10.0.2.2:8080 = this Mac's :8080),
#      FRAME_ID (sim-qemu), FRAME_TOKEN (set it empty to pair with the PAIR button),
#      SHOT_INTERVAL_S (10), SSHP (2222), RUN_FOR_S,
#      KEEP_SHOTS=1 (also keep each shot as sim/out/seq/<seconds since start>.png),
#      SHOW=1 (watch it live in Screen Sharing over VNC on localhost; a click is a mouse click).
# Build first: image/build.sh inputs && QEMU=1 image/build.sh rootfs. Slow (TCG), for correctness.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
Q="$ROOT/image/out/qemu"; W="$ROOT/sim/qemu/work"; OUT="$ROOT/sim/out"
SERVER_URL="${SERVER_URL:-http://10.0.2.2:8080}"; FRAME_ID="${FRAME_ID:-sim-qemu}"
FRAME_TOKEN="${FRAME_TOKEN-dev-token-change-me}"; SHOT_INTERVAL_S="${SHOT_INTERVAL_S:-10}"
SSHP="${SSHP:-2222}"; KEY="$ROOT/image/secrets/frame_id_ed25519"
PW=800; PH=1280

qmp() {	# one QMP command (JSON) on the running guest
	printf '{"execute":"qmp_capabilities"}\n%s\n' "$1" | nc -U -w 2 "$W/qmp.sock" >/dev/null
}

if [ "${1:-}" = tap ]; then	# one finger on the virtio multitouch screen, like the frame's panel
	x=$(( ${2:?x} * 32767 / (PW - 1) )); y=$(( ${3:?y} * 32767 / (PH - 1) ))
	mt() { printf '{"type":"mtt","data":{"type":"%s","slot":0,"tracking-id":%s,"axis":"%s","value":%s}}' "$@"; }
	qmp "{\"execute\":\"input-send-event\",\"arguments\":{\"events\":[$(mt begin 1 x "$x"),$(mt data 1 x "$x"),$(mt data 1 y "$y")]}}"
	sleep 0.15	# lift the finger: tracking id -1
	qmp "{\"execute\":\"input-send-event\",\"arguments\":{\"events\":[$(mt end -1 x "$x")]}}"
	exit 0
fi

for f in thinframe-sd.img zImage; do
	[ -f "$Q/$f" ] || { echo "STOP: no image/out/qemu/$f; run QEMU=1 image/build.sh rootfs" >&2; exit 1; }
done
mkdir -p "$W" "$OUT"; rm -f "$W"/*.sock
[ -z "${KEEP_SHOTS:-}" ] || { rm -rf "$OUT/seq"; mkdir -p "$OUT/seq"; }
echo "== copy of the image (4 GiB) with provision.txt for $FRAME_ID -> $SERVER_URL"
cp -c "$Q/thinframe-sd.img" "$W/sim.img" 2>/dev/null || cp "$Q/thinframe-sd.img" "$W/sim.img"
truncate -s 4G "$W/sim.img"
{
	printf 'SERVER_URL=%s\nFRAME_ID=%s\nFRAME_TOKEN=%s\n' "$SERVER_URL" "$FRAME_ID" "$FRAME_TOKEN"
	[ ! -f "$KEY.pub" ] || printf 'SSH_PUBKEY=%s\n' "$(cat "$KEY.pub")"
} > "$W/provision.txt"
docker run --rm -v "$W":/w thinframe-br mcopy -o -i /w/sim.img@@4M /w/provision.txt ::/

# The real slot-A command line (quiet, no cursor, fbcon off the panel), serial on the PL011,
# and the panel's mode pinned (as on the frame, where the panel has only one).
APPEND="$(grep -m1 APPEND "$Q/slot-a.conf" | sed -e 's/^[[:space:]]*APPEND //' \
	-e 's/console=ttyGS0,115200/console=ttyAMA0/' -e 's/ g_cdc[^ ]*//g') video=Virtual-1:${PW}x${PH}"
# SHOW=1 watches over VNC: a cocoa window would resize the guest's screen to its own size.
VIEW=(); [ -z "${SHOW:-}" ] || VIEW=(-vnc "127.0.0.1:1,password=on")
# Modern virtio-mmio: virtio-gpu and virtio-input refuse the legacy transport (QEMU default).
qemu-system-arm -M virt -cpu cortex-a15 -m 1G -smp 4 -display none ${VIEW[@]+"${VIEW[@]}"} \
	-global virtio-mmio.force-legacy=false \
	-kernel "$Q/zImage" -append "$APPEND" \
	-drive "file=$W/sim.img,format=raw,if=none,id=disk" -device virtio-blk-device,drive=disk \
	-netdev "user,id=net,hostfwd=tcp:127.0.0.1:$SSHP-:22" -device virtio-net-device,netdev=net \
	-device "virtio-gpu-device,xres=$PW,yres=$PH" -device virtio-multitouch-device \
	-device virtio-tablet-device -device virtio-keyboard-device \
	-monitor "unix:$W/mon.sock,server,nowait" -qmp "unix:$W/qmp.sock,server,nowait" \
	-chardev "socket,id=con,path=$W/console.sock,server=on,wait=off,logfile=$W/serial.log" -serial chardev:con \
	> "$W/qemu.out" 2>&1 &
QPID=$!
trap 'kill $QPID 2>/dev/null || true' EXIT INT TERM
if [ -n "${SHOW:-}" ]; then	# macOS Screen Sharing wants a password; this one only guards localhost
	sleep 2; echo "set_password vnc thinframe" | nc -U -w 2 "$W/mon.sock" >/dev/null || true
	open "vnc://:thinframe@127.0.0.1:5901" || true
fi
echo "== booting (pid $QPID); serial: sim/qemu/work/serial.log (console: nc -U sim/qemu/work/console.sock); ssh -p $SSHP frame@127.0.0.1"
start=$(date +%s)
while kill -0 "$QPID" 2>/dev/null; do
	sleep "$SHOT_INTERVAL_S"
	[ -S "$W/mon.sock" ] || continue
	echo "screendump $W/shot.png -f png" | nc -U -w 2 "$W/mon.sock" >/dev/null || true
	if [ -s "$W/shot.png" ]; then
		[ -z "${KEEP_SHOTS:-}" ] || cp "$W/shot.png" "$OUT/seq/$(printf %04d $(( $(date +%s) - start ))).png"
		mv "$W/shot.png" "$OUT/frame.png"
	fi
	[ -z "${RUN_FOR_S:-}" ] || [ $(( $(date +%s) - start )) -lt "$RUN_FOR_S" ] || break
done
