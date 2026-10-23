#!/usr/bin/env bash
# Build the thinframe SD image on a Mac.
#
#   image/build.sh            all: uboot secrets rootfs assemble sums
#   image/build.sh uboot      build U-Boot (frame + QEMU) from pinned source and image/inputs/uboot (image/uboot-build.sh)
#   image/build.sh rootfs     Buildroot (BR2_EXTERNAL image/buildroot) in Docker (image/br-run.sh);
#                             its post-image.sh runs genimage -> image/out/thinframe-sd.img + system-<ver>.tar
#   image/build.sh assemble   self-check of the card image (MBR signature, U-Boot, partitions)
#   image/build.sh check      lint the Buildroot tree (no Docker, no network)
#   image/build.sh test       boot the image in QEMU (orangepi-pc board with the QEMU U-Boot)
#   image/build.sh abtest     A/B system update + rollback in QEMU, driven by a local frameserver
#   image/build.sh clean
#
# Needs: git curl shasum python3 docker, cargo-zigbuild. Buildroot cross-compiles: no emulation.
# RELEASE=1 leaves image/secrets out of the image (public release builds).
# DEBUG=1 adds board/pf1007l/debug.fragment (evtest, strace); version gets -debug.
# QEMU=1 builds the -M virt variant (qemu.fragment) into image/out/qemu/ for sim/qemu/run.sh.
# Writes only to image/{cache,work,out,secrets}. No sudo. No devices touched.
set -euo pipefail

# ---- pinned inputs (TASK 0/1, pinned 2026-10-20; U-Boot pin in uboot-build.sh) ----
BUILDROOT_VERSION="2025.02.18"   # current LTS (buildroot.org/download.html, 2026-10-20)
BUILDROOT_SHA256="e38ad1df6ea0479fff6419a87a64535d02131674d463be154a763ef725b55321"
KVER="7.2.10"

# card layout (what image/inputs/uboot/boot.cmd expects: p1 FAT, root slots p2/p3)
SIG="50463037"                   # PARTUUID=50463037-01 .. -04
TOUCH_FW="${PF1007L_TOUCH_FIRMWARE:-}"
TOUCH_FW_SHA256="e11ffa8a33a56b78f8d2f57925fef1b67ea9a58c264bb5f3a8553ea52b14a9e6"

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
VERSION="${THINFRAME_VERSION:-$(date -u +%Y.%m.%d)-$(git -C "$ROOT" rev-parse --short=8 HEAD)}"
VARIANT=""
[ "${DEBUG:-0}" != 1 ] || VARIANT=debug
[ "${QEMU:-0}" != 1 ] || VARIANT="${VARIANT:+x}qemu"
case "$VARIANT" in x*) echo "STOP: DEBUG=1 and QEMU=1 together" >&2; exit 1 ;; esac
if [ -n "$VARIANT" ]; then
	[ "${RELEASE:-0}" != 1 ] || { echo "STOP: RELEASE=1 with a $VARIANT build" >&2; exit 1; }
	VERSION="$VERSION-$VARIANT"
fi
CACHE="$HERE/cache"; WORK="$HERE/work"; OUT="$HERE/out"; SECRETS="$HERE/secrets"
mkdir -p "$CACHE" "$WORK" "$OUT" "$SECRETS"; chmod 700 "$SECRETS"

say()  { printf '\n==== %s\n' "$*"; }
stop() { echo "STOP: $*" >&2; exit 1; }
sha()  { shasum -a 256 "$1" | cut -d' ' -f1; }
need() { command -v "$1" >/dev/null || stop "missing tool: $1"; }

step_uboot() {
	say "uboot: build L for the frame and the QEMU orangepi-pc build (image/uboot-build.sh)"
	bash "$HERE/uboot-build.sh"
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
	say "rootfs: Buildroot $BUILDROOT_VERSION (pf1007l_defconfig), version $VERSION"
	need docker; need cargo-zigbuild
	[ -f "$WORK/uboot/u-boot-pf1007l-L.bin" ] || stop "run image/build.sh uboot first"
	command -v cargo >/dev/null || PATH="$(echo "$HOME"/.rustup/toolchains/stable-*/bin):$PATH"
	(cd "$ROOT/tfctl" && cargo zigbuild -q --release --target armv7-unknown-linux-musleabihf)
	mkdir -p "$WORK/tfctl" && cp "$ROOT/tfctl/target/armv7-unknown-linux-musleabihf/release/tfctl" "$WORK/tfctl/"
	rm -rf "$WORK/secrets" "$WORK/touch"
	if [ "${RELEASE:-0}" != 1 ]; then	# release images carry no keys; dropbear makes its own
		install -d -m 0700 "$WORK/secrets"
		[ ! -f "$SECRETS/frame_id_ed25519.pub" ] || cp "$SECRETS/frame_id_ed25519.pub" "$WORK/secrets/authorized_keys"
	fi
	if [ -n "$TOUCH_FW" ]; then mkdir -p "$WORK/touch" && cp "$TOUCH_FW" "$WORK/touch/gsl_m86_hd.fw"; fi
	export BUILDROOT_VERSION BUILDROOT_SHA256 THINFRAME_VERSION="$VERSION" VARIANT
	bash "$HERE/br-run.sh"
	[ "$VARIANT" != qemu ] || return 0	# not a card for the frame: no budget, no system-version
	echo "$VERSION" > "$OUT/system-version"
	local sz; sz=$(wc -c < "$OUT/rootfs.ext4" | tr -d ' ')
	echo "rootfs.ext4: $sz bytes (gate: under 200 MB)"; [ "$sz" -lt 200000000 ] || stop "rootfs over budget"
}

step_assemble() {
	say "assemble: self-check of $OUT/thinframe-sd.img (genimage ran in post-image.sh)"
	bash "$HERE/selfcheck.sh" "$OUT/thinframe-sd.img" "$WORK/uboot/u-boot-pf1007l-L.bin" "$SIG"
}

step_check() {
	say "check: Buildroot external tree and boot inputs"
	local br="$HERE/buildroot" f
	for f in external.desc external.mk Config.in configs/pf1007l_defconfig \
		board/pf1007l/{genimage.cfg,post-build.sh,post-image.sh,linux.fragment,busybox.fragment,debug.fragment,qemu.fragment,linux-qemu.fragment,users.txt} \
		package/tfctl/{Config.in,tfctl.mk} package/aic8800/{Config.in,aic8800.mk}; do
		[ -f "$br/$f" ] || stop "missing buildroot/$f"
	done
	for f in dts/allwinner/sun8i-a33-pf1007l{,-min,-pmic,-full}.dts \
		linux-patches/0001-drm-panel-jadard-jd9365da-h3-add-pf1007l-panel.patch \
		uboot/{0001-wes-pf1007l-uboot.patch,0002-pf1007l-linux-boot.patch,boot.cmd,Dockerfile,build-in-container.sh}; do
		[ -f "$HERE/inputs/$f" ] || stop "missing image/inputs/$f"
	done
	grep -q "^setenv pf_extlinux /extlinux/slot-\${slot}.conf\$" "$HERE/inputs/uboot/boot.cmd" || stop "boot.cmd has no A/B slot block"
	grep -q "disk-signature = 0x$SIG" "$br/board/pf1007l/genimage.cfg" || stop "genimage signature != $SIG"
	grep -q "PARTUUID=$SIG-0@PART@" "$HERE/boot/extlinux.conf.in" || stop "extlinux root != PARTUUID=$SIG-0N"
	grep -q "VALUE=\"$KVER\"" "$br/configs/pf1007l_defconfig" || stop "defconfig kernel pin != $KVER"
	echo "ok"
}

step_sums() { say "sums"; (cd "$OUT" && shasum -a 256 thinframe-sd.img > thinframe-sd.img.sha256 && cat thinframe-sd.img.sha256); }
step_test()  { bash "$HERE/qemu-test.sh"; }
step_abtest() { bash "$HERE/qemu-ab-test.sh"; }
step_clean() { rm -rf "$WORK" "$OUT"; echo "cleaned work/ and out/ (cache/ and secrets/ kept)"; }

case "${1:-all}" in
	all) step_uboot; step_secrets; step_rootfs; step_assemble; step_sums ;;
	uboot|secrets|rootfs|assemble|sums|test|abtest|check|clean) "step_$1" ;;
	*) stop "unknown step $1" ;;
esac
