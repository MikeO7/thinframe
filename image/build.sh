#!/usr/bin/env bash
# Build the thinframe SD image on a Mac.
#
#   image/build.sh            all: inputs rootfs assemble sums
#   image/build.sh inputs     fetch/build U-Boot, kernel, DTB, panel + Wi-Fi modules via ormarch
#   image/build.sh rootfs     Buildroot (BR2_EXTERNAL image/buildroot) in Docker (image/Dockerfile.br)
#   image/build.sh assemble   genimage: MBR + FAT32 + 3x ext4 -> image/out/thinframe-sd.img
#   image/build.sh check      lint the Buildroot tree (no Docker, no network)
#   image/build.sh test       boot the image in QEMU (orangepi-pc board, same trick as ormarch)
#   image/build.sh clean
#
# Needs: git curl shasum python3 docker (OrbStack/Docker Desktop with arm/v7 emulation).
# Writes only to image/{cache,work,out,secrets}. No sudo. No devices touched.
set -euo pipefail

# ---- pinned inputs (TASK 0/1, pinned 2026-10-20) ---------------------------
ORMARCH_REPO="https://github.com/pangoleen/ormarch-picture-frame.git"
ORMARCH_COMMIT="4207b5c2f6d6615bf6a18fb725cc7a77dd532065"   # 2026-10-12 tree that booted the frame
BUILDROOT_VERSION="2025.02.18"   # current LTS (buildroot.org/download.html, 2026-10-20)
BUILDROOT_SHA256="e38ad1df6ea0479fff6419a87a64535d02131674d463be154a763ef725b55321"
KVER="7.2.10"

# card layout (same as ormarch so the same U-Boot boot.scr works)
SIG="50463037"                   # PARTUUID=50463037-01 .. -04
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
	say "inputs: ormarch $ORMARCH_COMMIT (U-Boot build L, DTS, panel patch)"
	need git; need docker
	local om="$CACHE/ormarch" in="$WORK/inputs"
	[ -d "$om/.git" ] || git clone -q "$ORMARCH_REPO" "$om"
	git -C "$om" cat-file -e "$ORMARCH_COMMIT^{commit}" 2>/dev/null || git -C "$om" fetch -q origin
	git -C "$om" checkout -q --detach "$ORMARCH_COMMIT"
	[ -z "$(git -C "$om" status --porcelain --untracked-files=no)" ] || stop "ormarch checkout is modified"
	bash "$om/image/build.sh" uboot
	rm -rf "$in"; mkdir -p "$in/dts/allwinner" "$in/linux-patches"
	cp "$om"/image/work/uboot/{u-boot-pf1007l-L.bin,u-boot-pf1007l-L.config,u-boot-qemu-opipc.bin,boot.cmd} "$in/"
	cp "$om/kernel/dts/sun8i-a33-pf1007l.dts" "$om"/image/dts/sun8i-a33-pf1007l-{min,pmic,full}.dts "$in/dts/allwinner/"
	cp "$om/kernel/patches/v$KVER/"*.patch "$in/linux-patches/"
	(cd "$in" && find . -type f | LC_ALL=C sort | xargs shasum -a 256) > "$OUT/inputs.sha256"
	cat "$OUT/inputs.sha256"
}

step_secrets() {
	say "secrets"
	[ -f "$SECRETS/frame_id_ed25519" ] || ssh-keygen -q -t ed25519 -N "" -C frame@thinframe -f "$SECRETS/frame_id_ed25519"
	[ -f "$SECRETS/ssh_host_ed25519_key" ] || ssh-keygen -q -t ed25519 -N "" -C thinframe-host -f "$SECRETS/ssh_host_ed25519_key"
	if [ -n "$TOUCH_FW" ]; then
		[ "$(sha "$TOUCH_FW")" = "$TOUCH_FW_SHA256" ] || stop "touch firmware hash mismatch"
	else
		echo "note: PF1007L_TOUCH_FIRMWARE not set; touch will not work"
	fi
}

step_rootfs() {
	say "rootfs: Buildroot $BUILDROOT_VERSION (pf1007l_defconfig)"
	need docker
	# TODO(TASK 2): docker build -f Dockerfile.br; run with $CACHE/dl, $CACHE/ccache, $WORK/br-out
	#   bind-mounted: make BR2_EXTERNAL=/src/image/buildroot pf1007l_defconfig && make.
	#   Gate: output/images/rootfs.ext4 < 200 MB.
	stop "TASK 2 not done: see TASKS.md"
}

step_assemble() {
	say "assemble: genimage, MBR sig $SIG, U-Boot @8KiB"
	# TODO(TASK 3): board/pf1007l/post-image.sh runs genimage; self-check reads back the MBR
	#   signature, U-Boot bytes at 8 KiB and partition starts; bundle emits system-<ver>.tar.
	stop "TASK 3 not done: see TASKS.md"
}

step_check() {
	say "check: Buildroot external tree"
	local br="$HERE/buildroot" f
	for f in external.desc external.mk Config.in configs/pf1007l_defconfig \
		board/pf1007l/{genimage.cfg,post-image.sh,boot.cmd,linux.config,linux.fragment} \
		package/tfctl/{Config.in,tfctl.mk} package/aic8800/{Config.in,aic8800.mk}; do
		[ -f "$br/$f" ] || stop "missing buildroot/$f"
	done
	grep -q "disk-signature = 0x$SIG" "$br/board/pf1007l/genimage.cfg" || stop "genimage signature != $SIG"
	grep -q "PARTUUID=$SIG-02" "$HERE/boot/extlinux.conf" || stop "extlinux root != PARTUUID=$SIG-02"
	grep -q "PARTUUID=$SIG-03" "$HERE/boot/extlinux.conf" || stop "extlinux slot b != PARTUUID=$SIG-03"
	grep -q "$KVER" "$br/configs/pf1007l_defconfig" || stop "defconfig kernel pin != $KVER"
	echo "ok"
}

step_sums() { say "sums"; (cd "$OUT" && shasum -a 256 ./*.img > thinframe-sd.img.sha256 && cat thinframe-sd.img.sha256); }
step_test()  { stop "TASK 2 gate: QEMU test not written (see ormarch image/tools/qemu-test.sh)"; }
step_clean() { rm -rf "$WORK" "$OUT"; echo "cleaned work/ and out/ (cache/ and secrets/ kept)"; }

case "${1:-all}" in
	all) step_inputs; step_secrets; step_rootfs; step_assemble; step_sums ;;
	inputs|secrets|rootfs|assemble|sums|test|check|clean) "step_$1" ;;
	*) stop "unknown step $1" ;;
esac
