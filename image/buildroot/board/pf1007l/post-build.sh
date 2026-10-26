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
# dhcpcd keeps leases in /var/db; on a read-only root that is RAM.
ln -sfn ../tmp "$T/var/db"

# Nothing periodic and no logging daemon (README budget): drop BusyBox's syslog/klogd and
# crond, and seedrng (it writes to the read-only root).
rm -f "$T"/etc/init.d/S01syslogd "$T"/etc/init.d/S02klogd "$T"/etc/init.d/S*seedrng "$T"/etc/init.d/S50crond
# iwd, dhcpcd, dropbear run under init's respawn (inittab), not Buildroot's start-once scripts.
rm -f "$T"/etc/init.d/S40iwd "$T"/etc/init.d/S41dhcpcd "$T"/etc/init.d/S50dropbear

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
