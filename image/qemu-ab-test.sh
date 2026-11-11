#!/usr/bin/env bash
# shellcheck disable=SC2329,SC2119  # helpers run via check(); start_qemu takes optional QEMU flags
# A/B system update and rollback in QEMU (TASK 12 gate), driven only by the server:
#   A (the image as built) -> B -> BAD (no init: the kernel panics, panic=10 reboots, U-Boot
#   spends the tries and flips back to B) -> C. No manual step touches the frame.
# Bundles B, BAD, C are the built system tar with /etc/thinframe.system-version rewritten
# (BAD also loses /sbin/init and /bin/sh). Result: image/out/qemu-ab-test.txt.
set -euo pipefail
# BOARD=virt runs the same test on the QEMU=1 image under -M virt (M2; see qemu-lib.sh).
# shellcheck source=SCRIPTDIR/qemu-lib.sh
. "$(dirname "$0")/qemu-lib.sh"
RES="$OUT/qemu-ab-test.txt"
A="$(built_version)"; B="$A-b"; BAD="$A-bad"; C="$A-c"
echo "== bundles $B $BAD $C from system-$A.tar"
make_bundles "$A" "$B" -bad "$BAD" "$C"

prepare_image
start_server
start_qemu
check "boot 1: A on slot a reports to the server" wait_for 1200 on "$A" a

start_server "$W/art-$B"
check "B written to slot b, rebooted, B reports from slot b" wait_for 1800 on "$B" b
check "U-Boot booted b with tries pending" grep -q -a 'thinframe: slot b, tries left 2' "$W/serial.log"
check "slot b confirmed by the agent (event)" wait_for 120 event slot-confirmed

start_server "$W/art-$BAD"
check "BAD written to slot a and the board rebooted into it" wait_for 1800 grep -q -a 'thinframe: slot a, tries left 2' "$W/serial.log"
check "BAD kernel panics (no init)" wait_for 600 grep -q -a 'No working init found' "$W/serial.log"
check "U-Boot spent the last try and flipped back to slot b" wait_for 900 grep -q -a 'thinframe: slot b, tries left 0' "$W/serial.log"
check "rolled back: B reports from slot b again" wait_for 1200 on "$B" b
check "agent refuses to flash BAD again (event)" wait_for 300 event system-update-skipped

start_server "$W/art-$C"
check "C written to slot a, rebooted, C reports from slot a" wait_for 1800 on "$C" a
grep -a -E 'thinframe: slot|No working init|tfctl: (agent|update)' "$W/serial.log" >> "$RES" || true
[ "$fail" = 0 ] && echo "qemu-ab-test: $A -> $B -> $BAD (rolled back) -> $C passed" | tee -a "$RES"
exit "$fail"
