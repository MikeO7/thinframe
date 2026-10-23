#!/usr/bin/env bash
# Build the frame's U-Boot. Mainline v2025.10 at one commit, plus image/uboot/patches (a console
# UART with no pins, a read-only ums, the PF1007L board), cross-compiled in Docker.
# Output, image/work/uboot/: u-boot-pf1007l.bin (+ .config, .dtb), the bootloader at 8 KiB on the
# card (SD only, no eMMC device, read-only USB disk mode), and u-boot-qemu-opipc.bin, the same
# tree for QEMU's orangepi-pc (image/build.sh test). The source is shallow-fetched into
# image/cache/u-boot; the build time is fixed (SOURCE_DATE_EPOCH), so image/out/uboot.sha256
# must come out the same on every run.
#   image/uboot/build.sh            (image/build.sh uboot calls this)
set -euo pipefail
UBOOT_REPO="https://github.com/u-boot/u-boot.git"
UBOOT_COMMIT="e50b1e8715011def8aff1588081a2649a2c6cd47"	# v2025.10
UBOOT_EPOCH=1791763200                                   	# 2026-10-12 00:00 UTC

HERE="$(cd "$(dirname "$0")" && pwd)"
IMAGE="$(cd "$HERE/.." && pwd)"
CACHE="$IMAGE/cache"; WORK="$IMAGE/work"; OUT="$IMAGE/out"
stop() { echo "STOP: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null || stop "missing tool: $1"; }
need git; need docker
mkdir -p "$CACHE" "$OUT" "$WORK/uboot"

src="$CACHE/u-boot"
if [ ! -d "$src/.git" ]; then
	git init -q "$src"
	git -C "$src" remote add origin "$UBOOT_REPO"
fi
git -C "$src" cat-file -e "$UBOOT_COMMIT^{commit}" 2>/dev/null \
	|| git -C "$src" fetch -q --depth 1 origin "$UBOOT_COMMIT"

docker image inspect thinframe-uboot >/dev/null 2>&1 || docker build -q -t thinframe-uboot "$HERE" >/dev/null
# Empty the output directory but keep it: a directory removed and made again can look empty to a
# container that mounts it right after (stale cache).
find "$WORK/uboot" -mindepth 1 -delete
git -C "$src" archive --format=tar "$UBOOT_COMMIT" | docker run --rm -i -e SOURCE_DATE_EPOCH="$UBOOT_EPOCH" \
	-v "$HERE":/u:ro -v "$WORK/uboot":/out thinframe-uboot /u/build-in-container.sh
(cd "$WORK/uboot" && shasum -a 256 u-boot-pf1007l.bin u-boot-qemu-opipc.bin) > "$OUT/uboot.sha256"
cat "$OUT/uboot.sha256"
