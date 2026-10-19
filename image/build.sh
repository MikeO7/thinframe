#!/usr/bin/env bash
# Build the alpineframe SD image on a Mac.
#
#   image/build.sh            all: inputs rootfs assemble sums
#   image/build.sh inputs     fetch/build U-Boot, kernel, DTB, panel + Wi-Fi modules via ormarch
#   image/build.sh rootfs     Alpine armv7 root in Docker (linux/arm/v7)
#   image/build.sh assemble   MBR + FAT32 + ext4 image file
#   image/build.sh test       boot the image in QEMU (orangepi-pc board, same trick as ormarch)
#   image/build.sh clean
#
# Needs: git curl shasum python3 docker (OrbStack/Docker Desktop with arm/v7 emulation).
# Writes only to image/{cache,work,out,secrets}. No sudo. No devices touched.
set -euo pipefail

# ---- pinned inputs (TASK 0: fill the TODOs) --------------------------------
ORMARCH_REPO="https://github.com/pangoleen/ormarch-picture-frame.git"
ORMARCH_COMMIT="TODO-pin-to-the-commit-that-matches-your-frame"
ALPINE_VERSION="3.24"            # TODO verify current stable
ALPINE_MINIROOTFS_URL="https://dl-cdn.alpinelinux.org/alpine/v${ALPINE_VERSION}/releases/armv7/alpine-minirootfs-${ALPINE_VERSION}.0-armv7.tar.gz"
ALPINE_MINIROOTFS_SHA256="TODO"
KVER="7.2.10-1-armv7-ARCH"

# card layout (same as ormarch so the same U-Boot boot.scr works)
SIG="50463037"                   # PARTUUID=50463037-01 / -02
IMG_MIB=2048
P1_START_MIB=4
P1_MIB=256
TOUCH_FW="${PF1007L_TOUCH_FIRMWARE:-}"
TOUCH_FW_SHA256="e11ffa8a33a56b78f8d2f57925fef1b67ea9a58c264bb5f3a8553ea52b14a9e6"

HERE="$(cd "$(dirname "$0")" && pwd)"
CACHE="$HERE/cache"; WORK="$HERE/work"; OUT="$HERE/out"; SECRETS="$HERE/secrets"
mkdir -p "$CACHE" "$WORK" "$OUT" "$SECRETS"; chmod 700 "$SECRETS"

say()  { printf '\n==== %s\n' "$*"; }
stop() { echo "STOP: $*" >&2; exit 1; }
sha()  { shasum -a 256 "$1" | cut -d' ' -f1; }
need() { command -v "$1" >/dev/null || stop "missing tool: $1"; }

step_inputs() {
	say "inputs: ormarch artifacts"
	need git; need docker
	[ -d "$CACHE/ormarch/.git" ] || git clone "$ORMARCH_REPO" "$CACHE/ormarch"
	git -C "$CACHE/ormarch" fetch -q && git -C "$CACHE/ormarch" checkout -q "$ORMARCH_COMMIT"
	# TODO(TASK 1): run $CACHE/ormarch/kernel/build.sh and wifi/build.sh, and the uboot/dtbs
	# steps of its image/build.sh, then copy into $WORK/inputs/:
	#   u-boot-pf1007l-L.bin boot.scr linux-armv7-7.2.10-1-armv7h.pkg.tar.xz
	#   sun8i-a33-pf1007l-{min,pmic,full}.dtb  panel-*.ko  aic8800_*.ko  aic8800DC/ (firmware)
	# Write $OUT/inputs.sha256.
	stop "TASK 1 not done: see TASKS.md"
}

step_secrets() {
	say "secrets"
	[ -f "$SECRETS/frame_id_ed25519" ] || ssh-keygen -q -t ed25519 -N "" -C frame@alpineframe -f "$SECRETS/frame_id_ed25519"
	[ -f "$SECRETS/ssh_host_ed25519_key" ] || ssh-keygen -q -t ed25519 -N "" -C alpineframe-host -f "$SECRETS/ssh_host_ed25519_key"
	if [ -n "$TOUCH_FW" ]; then
		[ "$(sha "$TOUCH_FW")" = "$TOUCH_FW_SHA256" ] || stop "touch firmware hash mismatch"
	else
		echo "note: PF1007L_TOUCH_FIRMWARE not set; touch will not work"
	fi
}

step_rootfs() {
	say "rootfs: Alpine $ALPINE_VERSION armv7"
	need docker; need curl
	local tgz
	tgz="$CACHE/$(basename "$ALPINE_MINIROOTFS_URL")"
	[ -f "$tgz" ] || curl -fL -o "$tgz" "$ALPINE_MINIROOTFS_URL"
	[ "$(sha "$tgz")" = "$ALPINE_MINIROOTFS_SHA256" ] || stop "minirootfs hash mismatch (pin it in TASK 0)"
	# TODO(TASK 2): docker run --platform linux/arm/v7 alpine:$ALPINE_VERSION with bind mounts:
	#   $HERE/rootfs -> /overlay, $HERE/packages.txt, $WORK/inputs -> /in, $SECRETS -> /secrets,
	#   $WORK/rootfs -> /target. Run image/rootfs-build/inside.sh (write it) which:
	#   unpack minirootfs; apk add $(grep -v '^#' packages.txt); cp -a /overlay/. /target/;
	#   unpack kernel pkg: usr/lib/modules/$KVER -> /target/lib/modules/$KVER, boot/zImage -> $WORK/boot/;
	#   extra .ko -> /target/lib/modules/$KVER/updates/; depmod -b /target $KVER;
	#   firmware -> /target/lib/firmware/{aic8800_fw/SDIO/aic8800DC,silead}; user frame; doas;
	#   authorized_keys + host key; rc-update add for every service listed in TASKS.md step 2;
	#   chmod +x /target/usr/local/bin/*; tar the result to $WORK/rootfs.tar.
	stop "TASK 2 not done: see TASKS.md"
}

step_assemble() {
	say "assemble: MBR sig $SIG, U-Boot @8KiB, FAT32 @${P1_START_MIB}MiB, ext4"
	# TODO(TASK 3): copy ormarch image/tools/assemble-in-container.sh approach:
	#   truncate img; sfdisk with label-id 0x$SIG; dd u-boot at seek=8 bs=1k conv=notrunc;
	#   mkfs.vfat -n PFBOOT (mtools mcopy: zImage, *.dtb, extlinux/extlinux.conf, boot.scr, uEnv.txt);
	#   mkfs.ext4 -L afroot -d rootfs-dir (e2fsprogs -d needs no root);
	#   self-check: read back MBR, U-Boot bytes, partition starts.
	stop "TASK 3 not done: see TASKS.md"
}

step_sums() { say "sums"; (cd "$OUT" && shasum -a 256 ./*.img > alpineframe-sd.img.sha256 && cat alpineframe-sd.img.sha256); }
step_test()  { stop "TASK 2 gate: QEMU test not written (see ormarch image/tools/qemu-test.sh)"; }
step_clean() { rm -rf "$WORK" "$OUT"; echo "cleaned work/ and out/ (cache/ and secrets/ kept)"; }

case "${1:-all}" in
	all) step_inputs; step_secrets; step_rootfs; step_assemble; step_sums ;;
	inputs|secrets|rootfs|assemble|sums|test|clean) "step_$1" ;;
	*) stop "unknown step $1" ;;
esac
