#!/usr/bin/env bash
# Buildroot post-image ($BINARIES_DIR = output/images). Builds PFBOOT's files (boot.scr from
# ormarch's boot.cmd + boot.cmd.patch, per-slot extlinux, uEnv.txt), the data partition, the
# card image (genimage.cfg) and the OS bundle system-<ver>.tar the container ships (TASK 3).
set -euo pipefail
BOARD="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$BOARD/../../../.." && pwd)"
WORK="${THINFRAME_WORK:-$REPO/image/work}"
IN="$WORK/inputs"
VER="${THINFRAME_VERSION:-dev}"
B="$BINARIES_DIR"
S="$B/stage"
# Kernel log on the USB serial (ttyGS0); /dev/console (the last console=) on tty1, a VT with no
# framebuffer console. ttyGS0 reports no write room until a USB host configures the gadget, so a
# user-space write to it waits forever on a wall brick: the frame never started off a computer.
CMDLINE="console=ttyGS0,115200 console=tty1 quiet loglevel=0 vt.global_cursor_default=0 consoleblank=0"
CMDLINE="$CMDLINE fbcon=map:9 panic=10 cma=128M psi=1 g_cdc.dev_addr=02:50:46:10:07:01 g_cdc.host_addr=02:50:46:10:07:02"

rm -rf "$S" && mkdir -p "$S"
cp "$B/zImage" "$S/zImage.a"
cp "$B"/sun8i-a33-pf1007l-{full,min,pmic}.dtb "$IN/u-boot-pf1007l-L.bin" "$B/rootfs.ext4" "$S/"
cp "$REPO/image/boot/uEnv.txt" "$S/"
cp "$IN/boot.cmd" "$S/boot.cmd"
patch -s "$S/boot.cmd" < "$BOARD/boot.cmd.patch"
"$HOST_DIR/bin/mkimage" -C none -A arm -T script -n "thinframe A/B on ormarch build L" \
	-d "$S/boot.cmd" "$S/boot.scr" >/dev/null
render() { # slot part other otherpart
	sed -e "s|@SLOT@|$1|g; s|@PART@|$2|g; s|@OTHER@|$3|g; s|@OTHERPART@|$4|g; s|@CMDLINE@|$CMDLINE|g" \
		"$REPO/image/boot/extlinux.conf.in" > "$S/slot-$1.conf"
}
render a 2 b 3
render b 3 a 2

# Data partition (p4): the bind-mount sources of /etc/fstab, plus keys from image/secrets.
D="$S/data-root"
mkdir -p "$D/etc" "$D/iwd" "$D/thinframe" && install -d -m 0755 "$D/ssh"
cp "$REPO/image/rootfs/etc/thinframe.conf" "$D/etc/thinframe.conf"
[ -f "$WORK/secrets/authorized_keys" ] && install -m 0644 "$WORK/secrets/authorized_keys" "$D/ssh/"
[ -f "$WORK/secrets/dropbear_ed25519_host_key" ] &&
	install -m 0600 "$WORK/secrets/dropbear_ed25519_host_key" "$D/ssh/"
rm -f "$S/data.ext4"
"$HOST_DIR/sbin/mkfs.ext4" -q -L data -E root_owner=0:0 -d "$D" "$S/data.ext4" 64M

"$HOST_DIR/bin/genimage" --config "$BOARD/genimage.cfg" --inputpath "$S" --outputpath "$B" \
	--rootpath "$(mktemp -d)" --tmppath "$(mktemp -d)"

# OS bundle for A/B updates (tfctl sysupdate.rs reads zImage + rootfs.ext4.zst).
U="$S/bundle" && mkdir -p "$U"
cp "$B/zImage" "$U/zImage"
zstd -q -19 -T0 -f "$B/rootfs.ext4" -o "$U/rootfs.ext4.zst"
(cd "$U" && sha256sum zImage rootfs.ext4.zst > sha256)
printf '{"version":"%s","kernel":"7.2.10"}\n' "$VER" > "$U/version.json"
tar -C "$U" --owner=0 --group=0 -cf "$B/system-$VER.tar" zImage rootfs.ext4.zst sha256 version.json
cp "$S/boot.scr" "$S/slot-a.conf" "$B/"
echo "post-image: $B/thinframe-sd.img, $B/system-$VER.tar"
