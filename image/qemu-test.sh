#!/usr/bin/env bash
# shellcheck disable=SC2016,SC2119  # guest commands are single-quoted on purpose; start_server takes an optional dir
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
check "data partition mounted, bound over dropbear and iwd; conf is a link into /data" \
	r 'for m in /data /etc/dropbear /var/lib/iwd; do grep -q " $m " /proc/mounts || exit 1; done; test "$(readlink /etc/thinframe.conf)" = /data/etc/thinframe.conf'
check "data partition grew to the card (> 1500 MiB)" r 'test "$(df -m /data | awk "NR==2 {print \$2}")" -gt 1500'
check "provision.txt applied and deleted" r 'grep -q "FRAME_ID=\"\?'"$ID"'" /etc/thinframe.conf && test ! -e /boot/provision.txt'
check "hostname is the frame id" r 'test "$(hostname)" = '"$ID"
check "tfctl kiosk runs under init" r 'ps -o args | grep -q "^/usr/bin/tfctl kiosk"'  # BusyBox here has no pgrep
check "cog and WPE WebKit are installed" r 'command -v cog && ls /usr/lib/libWPEWebKit-*.so*'
check "frame registered with frameserver over the network" wait_for 180 registered
check "status page was written before the dashboard (a boot state + frame id)" \
	r 'grep -qE "<h1>(joining |no Wi-Fi configured|reaching http)" /run/thinframe/status.html && grep -q "frame '"$ID"'" /run/thinframe/status.html'
r 'sed -n "s/.*<h1>\(.*\)<\/h1>.*/status page: \1/p" /run/thinframe/status.html; echo' | tee -a "$RES" || true
LOGO=iVBORw0KGgoAAAANSUhEUgAAAAQAAAADCAYAAAC09K7GAAAAPElEQVR42mN0UWiy2Xksvy3Pi8MnZi3HNSaX0E1pQqoyThF29xlWMf+ZzKLBaOPZyrBQf45manllY9xzAM7wEmsd43BoAAAAAElFTkSuQmCC
curl -fsS -X PUT -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
	-d '{"splash_text":"QEMU frame","splash_png":"'"$LOGO"'"}' "http://127.0.0.1:$PORT/api/frames/$ID/config" >/dev/null
check "splash text and logo pushed by the server land on /data" \
	wait_for 60 r 'grep -q "SPLASH_TEXT=\"QEMU frame\"" /etc/thinframe.conf && test -s /data/thinframe/splash.png'
check "no syslogd/klogd/crond running" r '! ps -o comm | grep -qxE "syslogd|klogd|crond"'
r 'ps -o pid,user,vsz,comm | grep -v "\[" ; free -m; df -h / /data /boot' > "$W/guest-state.txt" 2>&1 || true
echo "rootfs.ext4: $(wc -c < "$HERE/out/rootfs.ext4") bytes" | tee -a "$RES"
cat "$W/guest-state.txt" >> "$RES"
[ "$fail" = 0 ] && echo "qemu-test: all checks passed" | tee -a "$RES"
exit "$fail"
