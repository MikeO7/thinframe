#!/usr/bin/env bash
# Buildroot post-build ($1 = target dir). Shapes the read-only root: mount points, the data
# bind targets, no daemons we do not want, the fallback page, versions, touch firmware.
# Inputs from image/build.sh (environment): THINFRAME_VERSION, THINFRAME_WORK (image/work).
set -euo pipefail
T="$1"
REPO="$(cd "$(dirname "$0")/../../../.." && pwd)"
WORK="${THINFRAME_WORK:-$REPO/image/work}"

# Mount points and bind targets (fstab). /etc/dropbear must be a real directory, not
# Buildroot's symlink to /var/run/dropbear, so /data/ssh can be bound over it.
mkdir -p "$T/boot" "$T/data" "$T/var/lib/iwd" "$T/var/lib/thinframe"
rm -rf "$T/etc/dropbear" && mkdir -m 0755 "$T/etc/dropbear"
# The frame user's keys are dropbear's: one file on /data, written by `tfctl provision`.
install -d -m 0755 "$T/home/frame"
ln -sfn /etc/dropbear "$T/home/frame/.ssh"
# Config lives on /data; a symlink (not a file bind) so tfctl can replace it atomically.
ln -sfn /data/etc/thinframe.conf "$T/etc/thinframe.conf"
# The shipped defaults, for `tfctl boot` when it has to format /data fresh.
install -D -m 0644 "$REPO/image/rootfs/etc/thinframe.conf" "$T/usr/share/thinframe/thinframe.conf"
# dhcpcd keeps leases in /var/db; on a read-only root that is RAM.
ln -sfn ../tmp "$T/var/db"

# Root fs headroom (190 MiB, gate 200 MB): Buildroot never uninstalls, so a tree that built a
# package before an option went off keeps its files. DejaVu Serif and Sans Condensed are off in
# the defconfig but were 5.3 MB of the 2026.10.27 image. Also never used here: zoneinfo/right
# (leap-second zones; TZ names resolve in posix/), the sqlite3 shell (WebKit links the library),
# and udev's ID-to-name databases (hwdb 20-*: PCI/USB/OUI vendor names, 9 of hwdb.bin's 9.5 MB;
# input quirks are in 60-* and 70-*, kept).
rm -f "$T"/usr/share/fonts/dejavu/DejaVuSerif* "$T"/usr/share/fonts/dejavu/DejaVuSansCondensed*
rm -rf "$T/usr/share/zoneinfo/right" "$T/usr/bin/sqlite3"
if ls "$T"/etc/udev/hwdb.d/20-*.hwdb >/dev/null 2>&1; then
	rm -f "$T"/etc/udev/hwdb.d/20-*.hwdb
	"$HOST_DIR/bin/udevadm" hwdb --update --root "$T"
fi

# Nothing periodic and no logging daemon (README budget): drop BusyBox's syslog/klogd and
# crond, and seedrng (it writes to the read-only root).
rm -f "$T"/etc/init.d/S01syslogd "$T"/etc/init.d/S02klogd "$T"/etc/init.d/S*seedrng "$T"/etc/init.d/S50crond
# iwd, dhcpcd, dropbear run under init's respawn (inittab), not Buildroot's start-once scripts.
rm -f "$T"/etc/init.d/S40iwd "$T"/etc/init.d/S41dhcpcd "$T"/etc/init.d/S50dropbear

# Boot files for an eMMC install (tfctl emmc install, TASKS E) and for PFBOOT (post-image.sh):
# ormarch's boot.cmd + boot.cmd.patch, with every "mmc 0:1" reading the device U-Boot booted
# from (board/sunxi/board.c mmc_bootdev: 0 = card, build L; 1 = eMMC, build E); U-Boot build E
# (image/uboot-emmc.sh); the DTBs, which have the eMMC on.
SH="$T/usr/share/thinframe/boot"
rm -rf "$SH" && install -d "$SH"
cp "$WORK/inputs/boot.cmd" "$BINARIES_DIR/boot.cmd"
patch -s "$BINARIES_DIR/boot.cmd" < "$(dirname "$0")/boot.cmd.patch"
# shellcheck disable=SC2016 # ${pf_dev} and ${mmc_bootdev} are U-Boot's, not the shell's
sed -i -e 's/mmc 0:1/mmc ${pf_dev}:1/g' \
	-e 's/^# This script writes only to .*/# This script writes only to partition 1 of the device it booted from (pf_dev)./' \
	-e '/^setenv pf_script L1$/a\
setenv pf_dev ${mmc_bootdev}\
if test -z "${pf_dev}"; then setenv pf_dev 0; fi' "$BINARIES_DIR/boot.cmd"
if grep -q 'mmc 0:1' "$BINARIES_DIR/boot.cmd" || ! grep -q '^setenv pf_dev' "$BINARIES_DIR/boot.cmd"; then
	echo "post-build: boot.cmd device rewrite failed" >&2; exit 1
fi
"$HOST_DIR/bin/mkimage" -C none -A arm -T script -n "thinframe A/B, card or eMMC" \
	-d "$BINARIES_DIR/boot.cmd" "$BINARIES_DIR/boot.scr" >/dev/null
install -m 0644 "$BINARIES_DIR/boot.scr" "$SH/boot.scr"
if [ "${VARIANT:-}" != qemu ]; then
	install -m 0644 "$WORK/inputs/u-boot-pf1007l-E.bin" "$SH/u-boot-emmc.bin"
	for d in full pmic; do
		f="$BINARIES_DIR/sun8i-a33-pf1007l-$d.dtb"
		[ -f "$f" ] || { echo "post-build: $f missing" >&2; exit 1; }
		install -m 0644 "$f" "$SH/"
	done
fi

# Offline fallback page (the dashboard itself comes from frameserver).
rm -rf "$T/var/lib/thinframe/www" && cp -a "$REPO/www" "$T/var/lib/thinframe/www"

ver="${THINFRAME_VERSION:-dev}"
echo "$ver" > "$T/etc/thinframe.system-version"
echo "$ver" > "$T/etc/thinframe.version"

# Proprietary touch firmware: staged by build.sh from PF1007L_TOUCH_FIRMWARE, never in Git.
if [ -f "$WORK/touch/gsl_m86_hd.fw" ]; then
	install -D -m 0644 "$WORK/touch/gsl_m86_hd.fw" "$T/lib/firmware/silead/gsl_m86_hd.fw"
	ln -sfn gsl_m86_hd.fw "$T/lib/firmware/silead/gsl1680-pf1007l.fw"
else
	echo "post-build: no touch firmware staged; touch will not work on this image" >&2
fi

# DEBUG=1 only: root over SSH with the frame user's keys, for quick iteration at the desk
# (copy a tfctl, edit /boot, remount). Release and normal images keep dropbear's -w (no root).
if [ "${VARIANT:-}" = debug ]; then
	ln -sfn /etc/dropbear "$T/root/.ssh"
	sed -i 's|/usr/sbin/dropbear -F -R -s -w |/usr/sbin/dropbear -F -R -s |' "$T/etc/inittab"
	grep -q 'dropbear -F -R -s >' "$T/etc/inittab" || { echo "post-build: debug root: inittab line not found" >&2; exit 1; }
fi

# What tfctl runs (Command::new): a tree that built a package before an option it needs was on
# lost the binary silently (iwd before readline: no iwctl, so no Wi-Fi from the screen, 2026-10-26).
for b in usr/bin/iwctl usr/libexec/iwd usr/bin/cog usr/bin/cogctl usr/bin/dbus-daemon; do
	[ -x "$T/$b" ] || { echo "post-build: $b missing; tfctl needs it" >&2; exit 1; }
done
