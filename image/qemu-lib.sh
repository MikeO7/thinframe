#!/usr/bin/env bash
# shellcheck disable=SC2329,SC2034  # sourced by qemu-test.sh and qemu-ab-test.sh
# Shared QEMU harness. A COPY of image/out/thinframe-sd.img gets the test U-Boot for
# `-M orangepi-pc` at 8 KiB, the Orange Pi PC DTB and a serial console in the slot configs
# (same trick as ormarch's image/tools/qemu-test.sh). Everything else is the real image.
# A provision.txt points the frame at a frameserver on this machine. Writes only image/work/qemu/.
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"; ROOT="$(cd "$HERE/.." && pwd)"
W="$HERE/work/qemu"; IMG="$W/test.img"
PORT="${PORT:-18080}"; TOKEN=qemu-token; ID=sim-qemu; SSHP="${SSHP:-2222}"
KEY="$HERE/secrets/frame_id_ed25519"
SSH=(ssh -i "$KEY" -p "$SSHP" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null
	-o ConnectTimeout=5 -o LogLevel=ERROR frame@127.0.0.1)
PIDS=(); SRV=""; fail=0
cleanup() { for p in "${PIDS[@]:-}" $SRV; do kill "$p" 2>/dev/null || true; done; }
trap cleanup EXIT
check() {
	local d="$1"; shift
	if "$@" >/dev/null 2>&1; then echo "PASS  $d" | tee -a "$RES"; else echo "FAIL  $d" | tee -a "$RES"; fail=1; fi
}
r() { "${SSH[@]}" "$@"; }
api() { curl -fsS -H "Authorization: Bearer $TOKEN" "http://127.0.0.1:$PORT$1"; }
registered() { api /api/frames | grep -q "\"$ID\""; }
wait_for() { local n=0; until "${@:2}" >/dev/null 2>&1; do n=$((n + 5)); [ "$n" -ge "$1" ] && return 1; sleep 5; done; }

prepare_image() {
	for f in thinframe-sd.img sun8i-h3-orangepi-pc.dtb; do
		[ -f "$HERE/out/$f" ] || { echo "STOP: no image/out/$f, run image/build.sh rootfs" >&2; exit 1; }
	done
	[ -f "$HERE/work/inputs/u-boot-qemu-opipc.bin" ] || { echo "STOP: run image/build.sh inputs" >&2; exit 1; }
	mkdir -p "$W"; : > "$RES"; rm -f "$IMG" "$W/serial.log"
	echo "== test copy (4 GiB, so the data partition has room to grow)"
	cp -c "$HERE/out/thinframe-sd.img" "$IMG" 2>/dev/null || cp "$HERE/out/thinframe-sd.img" "$IMG"
	truncate -s 4G "$IMG"
	dd if="$HERE/work/inputs/u-boot-qemu-opipc.bin" of="$IMG" bs=1024 seek=8 conv=notrunc status=none
	cp "$HERE/out/sun8i-h3-orangepi-pc.dtb" "$W/"
	printf 'SERVER_URL=http://10.0.2.2:%s\nFRAME_ID=%s\nFRAME_TOKEN=%s\nSSH_PUBKEY=%s\n' \
		"$PORT" "$ID" "$TOKEN" "$(cat "$KEY.pub")" > "$W/provision.txt"
	docker run --rm -v "$W":/w thinframe-br bash -euo pipefail -c '
		o=@@4M; i=/w/test.img
		mcopy -o -i $i$o /w/sun8i-h3-orangepi-pc.dtb /w/provision.txt ::/
		for s in a b; do
			mcopy -n -i $i$o ::/extlinux/slot-$s.conf /w/slot-$s.conf
			sed -i "s|/sun8i-a33-pf1007l-full.dtb|/sun8i-h3-orangepi-pc.dtb|; s|console=ttyGS0,115200|console=ttyS0,115200|; s| quiet loglevel=0||" /w/slot-$s.conf
			mcopy -o -i $i$o /w/slot-$s.conf ::/extlinux/slot-$s.conf
		done'
	(cd "$ROOT/server" && go build -o "$W/frameserver" ./cmd/frameserver)
	rm -rf "$W/data"
}

# start_server [ARTIFACTS_DIR]: (re)start frameserver; frame state in $W/data survives restarts.
start_server() {
	[ -z "$SRV" ] || { kill "$SRV" 2>/dev/null || true; wait "$SRV" 2>/dev/null || true; }
	PORT=$PORT DATA_DIR="$W/data" ARTIFACTS_DIR="${1:-$W/artifacts}" FRAME_TOKEN=$TOKEN \
		"$W/frameserver" >> "$W/server.log" 2>&1 &
	SRV=$!
	wait_for 30 curl -fsS "http://127.0.0.1:$PORT/healthz"
}

# start_qemu [-no-reboot]: one board; without -no-reboot a guest reboot resets the board in place.
start_qemu() {
	qemu-system-arm -M orangepi-pc -m 1G -smp 4 -display none -monitor none "$@" \
		-serial "file:$W/serial.log" -drive "file=$IMG,format=raw,if=sd" \
		-nic "user,hostfwd=tcp:127.0.0.1:$SSHP-:22" > "$W/qemu.out" 2>&1 & PIDS+=($!)
}
