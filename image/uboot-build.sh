#!/usr/bin/env bash
# Build the two sunxi U-Boots from pinned mainline source plus the image/inputs/uboot patches, in
# Docker. Output: image/work/uboot/u-boot-pf1007l-L.bin (+ .config), the frame's bootloader at
# 8 KiB on the card (build L: SD only, no eMMC device, read-only USB disk mode), and
# u-boot-qemu-opipc.bin, the same tree for QEMU's orangepi-pc (image/build.sh test).
# The source is one commit, shallow-fetched into image/cache/u-boot; the build time is fixed
# (SOURCE_DATE_EPOCH), so image/out/uboot.sha256 must come out the same on every run.
#   image/uboot-build.sh            (image/build.sh uboot calls this)
set -euo pipefail
UBOOT_REPO="https://github.com/u-boot/u-boot.git"
UBOOT_COMMIT="e50b1e8715011def8aff1588081a2649a2c6cd47"	# v2025.10, the commit the frame booted
UBOOT_EPOCH=1791763200                                   	# 2026-10-12 00:00 UTC

HERE="$(cd "$(dirname "$0")" && pwd)"
CACHE="$HERE/cache"; WORK="$HERE/work"; OUT="$HERE/out"; U="$HERE/inputs/uboot"
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

docker image inspect thinframe-uboot >/dev/null 2>&1 || docker build -q -t thinframe-uboot "$U" >/dev/null
# Empty the output directory but keep it: a directory removed and made again can look empty to a
# container that mounts it right after (stale cache).
find "$WORK/uboot" -mindepth 1 -delete
git -C "$src" archive --format=tar "$UBOOT_COMMIT" | docker run --rm -i -e SOURCE_DATE_EPOCH="$UBOOT_EPOCH" \
	-v "$U":/u:ro -v "$WORK/uboot":/out thinframe-uboot /u/build-in-container.sh
(cd "$WORK/uboot" && shasum -a 256 u-boot-pf1007l-L.bin u-boot-qemu-opipc.bin) > "$OUT/uboot.sha256"
cat "$OUT/uboot.sha256"
