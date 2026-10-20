#!/usr/bin/env bash
# shellcheck disable=SC2016  # guest commands are single-quoted on purpose
# QEMU boot test of the real card image (TASK 2 gate). Harness: image/qemu-lib.sh.
# Result: image/out/qemu-test.txt. No sudo, no devices.
set -euo pipefail
RES="$(cd "$(dirname "$0")" && pwd)/out/qemu-test.txt"
# shellcheck source=SCRIPTDIR/qemu-lib.sh
. "$(dirname "$0")/qemu-lib.sh"
prepare_image
start_server
echo "== boot (TCG emulation, a few minutes)"
start_qemu -no-reboot
started=$(date +%s)
check "login prompt on the serial console" wait_for 900 grep -q -a 'login:' "$W/serial.log"
echo "boot to login prompt: $(( $(date +%s) - started )) s" | tee -a "$RES"
check "U-Boot ran our boot.scr (slot a, no tries pending)" grep -q -a 'thinframe: slot a, tries left 0' "$W/serial.log"
check "dropbear: key login as frame" wait_for 120 r true
check "kernel 7.2.10" r 'uname -r | grep -qx 7.2.10'
check "root is read-only ext4" r 'grep -q " / ext4 ro," /proc/mounts'
check "PFBOOT mounted read-only at /boot" r 'grep -q " /boot vfat ro," /proc/mounts'
check "data partition mounted and bind-mounted over conf, dropbear, iwd" \
	r 'for m in /data /etc/thinframe.conf /etc/dropbear /var/lib/iwd; do grep -q " $m " /proc/mounts || exit 1; done'
check "data partition grew to the card (> 1500 MiB)" r 'test "$(df -m /data | awk "NR==2 {print \$2}")" -gt 1500'
check "provision.txt applied and deleted" r 'grep -q "FRAME_ID=\"\?'"$ID"'" /etc/thinframe.conf && test ! -e /boot/provision.txt'
check "hostname is the frame id" r 'test "$(hostname)" = '"$ID"
check "tfctl kiosk runs under init" r 'pgrep -f "tfctl kiosk"'
check "cog and WPE WebKit are installed" r 'command -v cog && ls /usr/lib/libWPEWebKit-*.so*'
check "frame registered with frameserver over the network" wait_for 180 registered
check "no syslogd/klogd/crond running" r '! pgrep -x syslogd && ! pgrep -x klogd && ! pgrep -x crond'
r 'ps -o pid,user,vsz,comm | grep -v "\[" ; free -m; df -h / /data /boot' > "$W/guest-state.txt" 2>&1 || true
echo "rootfs.ext4: $(wc -c < "$HERE/out/rootfs.ext4") bytes" | tee -a "$RES"
cat "$W/guest-state.txt" >> "$RES"
[ "$fail" = 0 ] && echo "qemu-test: all checks passed" | tee -a "$RES"
exit "$fail"
