#!/usr/bin/env bash
# Build the thinframe SD image on a Mac.
#
#   image/build.sh            all: inputs rootfs assemble sums
#   image/build.sh inputs     fetch/build U-Boot, kernel, DTB, panel + Wi-Fi modules via ormarch
#   image/build.sh rootfs     Buildroot (BR2_EXTERNAL image/buildroot) in Docker (image/br-run.sh);
#                             its post-image.sh runs genimage -> image/out/thinframe-sd.img + system-<ver>.tar
#   image/build.sh assemble   self-check of the card image (MBR signature, U-Boot, partitions)
#   image/build.sh check      lint the Buildroot tree (no Docker, no network)
#   image/build.sh test       boot the image in QEMU (orangepi-pc board, same trick as ormarch)
#   image/build.sh abtest     A/B system update + rollback in QEMU, driven by a local frameserver
#   image/build.sh clean
#
# Needs: git curl shasum python3 docker, cargo-zigbuild. Buildroot cross-compiles: no emulation.
# RELEASE=1 leaves image/secrets out of the image (public release builds).
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
ROOT="$(cd "$HERE/.." && pwd)"
VERSION="${THINFRAME_VERSION:-$(date -u +%Y.%m.%d)-$(git -C "$ROOT" rev-parse --short=8 HEAD)}"
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
	say "rootfs: Buildroot $BUILDROOT_VERSION (pf1007l_defconfig), version $VERSION"
	need docker; need cargo-zigbuild
	[ -f "$WORK/inputs/u-boot-pf1007l-L.bin" ] || stop "run image/build.sh inputs first"
	command -v cargo >/dev/null || PATH="$(echo "$HOME"/.rustup/toolchains/stable-*/bin):$PATH"
	(cd "$ROOT/tfctl" && cargo zigbuild -q --release --target armv7-unknown-linux-musleabihf)
	mkdir -p "$WORK/tfctl" && cp "$ROOT/tfctl/target/armv7-unknown-linux-musleabihf/release/tfctl" "$WORK/tfctl/"
	rm -rf "$WORK/secrets" "$WORK/touch"
	if [ "${RELEASE:-0}" != 1 ]; then	# release images carry no keys; dropbear makes its own
		install -d -m 0700 "$WORK/secrets"
		[ ! -f "$SECRETS/frame_id_ed25519.pub" ] || cp "$SECRETS/frame_id_ed25519.pub" "$WORK/secrets/authorized_keys"
	fi
	if [ -n "$TOUCH_FW" ]; then mkdir -p "$WORK/touch" && cp "$TOUCH_FW" "$WORK/touch/gsl_m86_hd.fw"; fi
	export BUILDROOT_VERSION BUILDROOT_SHA256 THINFRAME_VERSION="$VERSION"
	bash "$HERE/br-run.sh"
	echo "$VERSION" > "$OUT/system-version"
	local sz; sz=$(wc -c < "$OUT/rootfs.ext4" | tr -d ' ')
	echo "rootfs.ext4: $sz bytes (gate: under 200 MB)"; [ "$sz" -lt 200000000 ] || stop "rootfs over budget"
}

step_assemble() {
	say "assemble: self-check of $OUT/thinframe-sd.img (genimage ran in post-image.sh)"
	bash "$HERE/selfcheck.sh" "$OUT/thinframe-sd.img" "$WORK/inputs/u-boot-pf1007l-L.bin" "$SIG"
}

step_check() {
	say "check: Buildroot external tree"
	local br="$HERE/buildroot" f
	for f in external.desc external.mk Config.in configs/pf1007l_defconfig \
		board/pf1007l/{genimage.cfg,post-build.sh,post-image.sh,boot.cmd.patch,linux.fragment,busybox.fragment,users.txt} \
		package/tfctl/{Config.in,tfctl.mk} package/aic8800/{Config.in,aic8800.mk}; do
		[ -f "$br/$f" ] || stop "missing buildroot/$f"
	done
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
	all) step_inputs; step_secrets; step_rootfs; step_assemble; step_sums ;;
	inputs|secrets|rootfs|assemble|sums|test|abtest|check|clean) "step_$1" ;;
	*) stop "unknown step $1" ;;
esac
