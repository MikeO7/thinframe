#!/usr/bin/env bash
# Unit tests for the pure parts of scripts/flash.sh: size/removable guard, provision.txt, and
# card discovery (Linux path, with a fake lsblk). Nothing here touches a disk.
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

if [ "$(uname)" != Darwin ]; then
	shim="$(mktemp -d)"; trap 'rm -rf "$shim"' EXIT
	cat >"$shim/lsblk" <<SH
#!/usr/bin/env bash
case "\$*" in
*MOUNTPOINT*) case "\$*" in *sda*) printf '/\n/boot\n';; *) :;; esac;;
*) printf 'sda 500000000000 0 SSD\nsdb 31914983424 1 SD_Reader\nsdc 16000000000 1 Boot_USB\nloop0 1000000 1 \nsdd 2000000000 1 Tiny\n';;
esac
SH
	chmod +x "$shim/lsblk"
	# sdc carries a mounted system volume
	sed -i 's#\*sda\*) printf#*sda*|*sdc*) printf#' "$shim/lsblk"
	got="$(PATH="$shim:$PATH" list_cards | cut -f1 | tr '\n' ' ')"
	check "only the free removable card is listed" "$got" "sdb "
fi
[ "$fails" -eq 0 ] || exit 1
