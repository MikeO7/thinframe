#!/usr/bin/env bash
# M2: the real Buildroot image (QEMU=1 variant: same rootfs, tfctl and cog, plus virt drivers and
# Mesa swrast) under `qemu-system-arm -M virt`. cog draws on virtio-gpu at the panel's native
# 800x1280; screendumps land in sim/out/frame.png every SHOT_INTERVAL_S seconds.
#   sim/qemu/run.sh            boot and keep shooting until Ctrl-C (or RUN_FOR_S seconds)
#   sim/qemu/run.sh tap X Y    touch the running guest at panel pixel X,Y (0..799, 0..1279)
# Env: SERVER_URL (as the guest sees it; default http://10.0.2.2:8080 = this Mac's :8080),
#      FRAME_ID (sim-qemu), FRAME_TOKEN, SHOT_INTERVAL_S (10), SSHP (2222), RUN_FOR_S.
# Build first: image/build.sh inputs && QEMU=1 image/build.sh rootfs. Slow (TCG), for correctness.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
Q="$ROOT/image/out/qemu"; W="$ROOT/sim/qemu/work"; OUT="$ROOT/sim/out"
SERVER_URL="${SERVER_URL:-http://10.0.2.2:8080}"; FRAME_ID="${FRAME_ID:-sim-qemu}"
FRAME_TOKEN="${FRAME_TOKEN:-dev-token-change-me}"; SHOT_INTERVAL_S="${SHOT_INTERVAL_S:-10}"
SSHP="${SSHP:-2222}"; KEY="$ROOT/image/secrets/frame_id_ed25519"
PW=800; PH=1280

qmp() {	# one QMP command (JSON) on the running guest
	printf '{"execute":"qmp_capabilities"}\n%s\n' "$1" | nc -U -w 2 "$W/qmp.sock" >/dev/null
}

if [ "${1:-}" = tap ]; then
	x=$(( ${2:?x} * 32767 / (PW - 1) )); y=$(( ${3:?y} * 32767 / (PH - 1) ))
	ev() { printf '{"type":"%s","data":%s}' "$1" "$2"; }
	move="$(ev abs "{\"axis\":\"x\",\"value\":$x}"),$(ev abs "{\"axis\":\"y\",\"value\":$y}")"
	qmp "{\"execute\":\"input-send-event\",\"arguments\":{\"events\":[$move,$(ev btn '{"down":true,"button":"left"}')]}}"
	sleep 0.1
	qmp "{\"execute\":\"input-send-event\",\"arguments\":{\"events\":[$(ev btn '{"down":false,"button":"left"}')]}}"
	exit 0
fi

for f in thinframe-sd.img zImage; do
	[ -f "$Q/$f" ] || { echo "STOP: no image/out/qemu/$f; run QEMU=1 image/build.sh rootfs" >&2; exit 1; }
done
mkdir -p "$W" "$OUT"; rm -f "$W"/*.sock
echo "== copy of the image (4 GiB) with provision.txt for $FRAME_ID -> $SERVER_URL"
cp -c "$Q/thinframe-sd.img" "$W/sim.img" 2>/dev/null || cp "$Q/thinframe-sd.img" "$W/sim.img"
truncate -s 4G "$W/sim.img"
{
	printf 'SERVER_URL=%s\nFRAME_ID=%s\nFRAME_TOKEN=%s\n' "$SERVER_URL" "$FRAME_ID" "$FRAME_TOKEN"
	[ ! -f "$KEY.pub" ] || printf 'SSH_PUBKEY=%s\n' "$(cat "$KEY.pub")"
} > "$W/provision.txt"
docker run --rm -v "$W":/w thinframe-br mcopy -o -i /w/sim.img@@4M /w/provision.txt ::/

# Modern virtio-mmio: virtio-gpu and virtio-input refuse the legacy transport (QEMU default).
qemu-system-arm -M virt -cpu cortex-a15 -m 1G -smp 4 -display none \
	-global virtio-mmio.force-legacy=false \
	-kernel "$Q/zImage" -append "root=PARTUUID=50463037-02 rootfstype=ext4 rootwait ro console=ttyAMA0 panic=10" \
	-drive "file=$W/sim.img,format=raw,if=none,id=disk" -device virtio-blk-device,drive=disk \
	-netdev "user,id=net,hostfwd=tcp:127.0.0.1:$SSHP-:22" -device virtio-net-device,netdev=net \
	-device "virtio-gpu-device,xres=$PW,yres=$PH" -device virtio-tablet-device -device virtio-keyboard-device \
	-monitor "unix:$W/mon.sock,server,nowait" -qmp "unix:$W/qmp.sock,server,nowait" \
	-chardev "socket,id=con,path=$W/console.sock,server=on,wait=off,logfile=$W/serial.log" -serial chardev:con \
	> "$W/qemu.out" 2>&1 &
QPID=$!
trap 'kill $QPID 2>/dev/null || true' EXIT INT TERM
echo "== booting (pid $QPID); serial: sim/qemu/work/serial.log (console: nc -U sim/qemu/work/console.sock); ssh -p $SSHP frame@127.0.0.1"
start=$(date +%s)
while kill -0 "$QPID" 2>/dev/null; do
	sleep "$SHOT_INTERVAL_S"
	[ -S "$W/mon.sock" ] || continue
	echo "screendump $W/shot.png -f png" | nc -U -w 2 "$W/mon.sock" >/dev/null || true
	[ ! -s "$W/shot.png" ] || mv "$W/shot.png" "$OUT/frame.png"
	[ -z "${RUN_FOR_S:-}" ] || [ $(( $(date +%s) - start )) -lt "$RUN_FOR_S" ] || break
done
