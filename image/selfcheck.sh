#!/usr/bin/env bash
# shellcheck disable=SC2329  # eq, ub_ok and layout_ok are called through check()
# Self-check of the card image (TASK 3 gate): reads bytes back, changes nothing.
#   image/selfcheck.sh IMG UBOOT_BIN SIG
set -euo pipefail
IMG="${1:?img}"; UB="${2:?u-boot}"; SIG="${3:?sig}"
fail=0
check() { local d="$1"; shift; if "$@"; then echo "PASS  $d"; else echo "FAIL  $d"; fail=1; fi; }
hex() { od -An -tx1 -j "$2" -N "$3" "$1" | tr -d ' \n'; }        # file offset count
str() { dd if="$IMG" bs=1 skip="$1" count="$2" 2>/dev/null; }
u32() { local h; h=$(hex "$IMG" "$1" 4); echo $((16#${h:6:2}${h:4:2}${h:2:2}${h:0:2})); }
eq()  { [ "$1" = "$2" ]; }
e=446; p1t=$(hex "$IMG" $((e + 4)) 1); p1f=$(hex "$IMG" $e 1); p1s=$(u32 $((e + 8))); p1n=$(u32 $((e + 12)))
types="$(hex "$IMG" 466 1) $(hex "$IMG" 482 1) $(hex "$IMG" 498 1)"
s2=$(u32 470); n2=$(u32 474); s3=$(u32 486); n3=$(u32 490); s4=$(u32 502); n4=$(u32 506)
sectors=$(($(wc -c < "$IMG") / 512))
layout_ok() { [ "$s2" -eq $((p1s + p1n)) ] && [ "$n2" -eq 2097152 ] && [ "$n3" -eq 2097152 ] \
	&& [ "$s3" -eq $((s2 + n2)) ] && [ "$s4" -eq $((s3 + n3)) ] && [ "$((s4 + n4))" -le "$sectors" ]; }

check "MBR disk signature 0x$SIG" eq "$(hex "$IMG" 440 4)" "${SIG:6:2}${SIG:4:2}${SIG:2:2}${SIG:0:2}"
check "MBR boot signature 55aa" eq "$(hex "$IMG" 510 2)" 55aa
ub_ok() { cmp -s "$UB" <(tail -c +8193 "$IMG" | head -c "$(wc -c < "$UB")"); }
check "U-Boot at 8 KiB is byte-equal to the build" ub_ok
check "eGON.BT0 header at 8 KiB + 4" eq "$(str 8196 8)" eGON.BT0
check "p1: FAT32 LBA, bootable, starts at 4 MiB, 256 MiB" eq "$p1t $p1f $p1s $p1n" "0c 80 8192 524288"
check "p2 p3 p4 are Linux (83)" eq "$types" "83 83 83"
check "rootA and rootB 1 GiB each, data follows, all inside the image" layout_ok
check "p1 volume label PFBOOT" eq "$(str $((p1s * 512 + 71)) 6)" PFBOOT
check "p1 is formatted FAT32 (boot sector type field)" eq "$(str $((p1s * 512 + 82)) 5)" FAT32
check "p2 is ext4 (superblock magic)" eq "$(hex "$IMG" $((s2 * 512 + 1080)) 2)" 53ef
check "p4 is ext4 labelled data" eq "$(hex "$IMG" $((s4 * 512 + 1080)) 2) $(str $((s4 * 512 + 1144)) 4)" "53ef data"
exit "$fail"
