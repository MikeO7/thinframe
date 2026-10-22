#!/usr/bin/env bash
# Unit tests for the pure parts of scripts/flash.sh: size/removable guard, provision.txt, and
# card discovery on both OS paths (fake uname, lsblk, diskutil, mount). Nothing here touches a disk.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export FLASH_SOURCE_ONLY=1
# shellcheck source=../scripts/flash.sh
. "$ROOT/scripts/flash.sh"
set +e
fails=0
check() { if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: got '$2' want '$3'"; fails=$((fails + 1)); fi; }

G=1000000000
card_ok $((32 * G)) 1; check "32 GB removable accepted" $? 0
card_ok $((2 * G)) 1;  check "2 GB refused" $? 1
card_ok $((128 * G)) 1; check "128 GB refused" $? 1
card_ok $((32 * G)) 0; check "fixed disk refused" $? 1

WIFI_SSID=Home WIFI_PSK='pass word' SERVER_URL=http://nas:8080 FRAME_ID=kitchen FRAME_TOKEN=t SSH_PUBKEY='ssh-ed25519 AAA x'
out="$(provision_txt)"
check "provision has ssid" "$(grep -c '^WIFI_SSID="Home"$' <<<"$out")" 1
check "provision has psk" "$(grep -c '^WIFI_PSK="pass word"$' <<<"$out")" 1
check "provision has server" "$(grep -c '^SERVER_URL=http://nas:8080$' <<<"$out")" 1
check "provision has pubkey" "$(grep -c '^SSH_PUBKEY=ssh-ed25519 AAA x$' <<<"$out")" 1

shim="$(mktemp -d)"; trap 'rm -rf "$shim"' EXIT
# Linux path: sda is the system disk, sdc carries a mounted /boot, loop0 and sdd fail card_ok.
printf '#!/usr/bin/env bash\necho Linux\n' >"$shim/uname"
cat >"$shim/lsblk" <<SH
#!/usr/bin/env bash
case "\$*" in
*MOUNTPOINT*) case "\$*" in *sda*|*sdc*) printf '/\n/boot\n';; *) :;; esac;;
*) printf 'sda 500000000000 0 SSD\nsdb 31914983424 1 SD_Reader\nsdc 16000000000 1 Boot_USB\nloop0 1000000 1 \nsdd 2000000000 1 Tiny\n';;
esac
SH
chmod +x "$shim/uname" "$shim/lsblk"
got="$(PATH="$shim:$PATH" list_cards | cut -f1 | tr '\n' ' ')"
check "linux: only the free removable card is listed" "$got" "sdb "

# macOS path: disk3 is a 32 GB card, disk4 a removable disk the Mac booted from, disk5 too big.
printf '#!/usr/bin/env bash\necho Darwin\n' >"$shim/uname"
cat >"$shim/diskutil" <<'SH'
#!/usr/bin/env bash
case "$1 $2" in
"list external") printf '/dev/disk3 (external, physical):\n/dev/disk4 (external, physical):\n/dev/disk5 (external, physical):\n';;
"info disk3") printf '   Disk Size: 31.9 GB (31914983424 Bytes)\n   Removable Media: Removable\n   Media Name: SD Card Reader\n';;
"info disk4") printf '   Disk Size: 64.0 GB (64000000000 Bytes)\n   Removable Media: Removable\n   Media Name: Boot Stick\n';;
"info disk5") printf '   Disk Size: 128.0 GB (128000000000 Bytes)\n   Removable Media: Removable\n   Media Name: Big\n';;
esac
SH
printf '#!/usr/bin/env bash\nprintf "/dev/disk3s1 on /Volumes/PFBOOT (msdos)\\n/dev/disk4s1 on / (apfs)\\n/dev/disk4s2 on /System/Volumes/Data (apfs)\\n"\n' >"$shim/mount"
chmod +x "$shim/diskutil" "$shim/mount"
got="$(PATH="$shim:$PATH" list_cards | tr '\t\n' '  ')"
check "macos: the booted removable disk and the big one are skipped" "$got" "disk3 31914983424 SD Card Reader "
[ "$fails" -eq 0 ] || exit 1
