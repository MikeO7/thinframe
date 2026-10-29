#!/usr/bin/env bash
# thinframe SD card setup (macOS, Linux): curl -fsSL https://raw.githubusercontent.com/MikeO7/thinframe/main/scripts/flash.sh | bash
# Spec docs/flashing.md. Settings remembered (Wi-Fi + password from this Mac), latest release (sha256
# checked), waits for a removable 4-64 GB card, one y/N, one sudo for write + readback (progress bars),
# provision.txt onto PFBOOT, eject. --settings-only rewrites provision.txt only. Never touches eMMC.
set -euo pipefail

REPO="MikeO7/thinframe"
STATE="${THINFRAME_STATE:-$HOME/.thinframe}"; IMG="$STATE/thinframe-sd.img"
MIN_B=$((4 * 1000 ** 3)); MAX_B=$((64 * 1000 ** 3)); TOUCH=""
TOUCH_SHA=e11ffa8a33a56b78f8d2f57925fef1b67ea9a58c264bb5f3a8553ea52b14a9e6	# tfctl touchfw.rs SHA256
B=$'\033[1m'; D=$'\033[2m'; G=$'\033[32m'; N=$'\033[0m'
step() { printf '\n%s[%s/4] %s%s\n' "$B" "$1" "$2" "$N"; }
ok()   { printf '  %s✓%s %s\n' "$G" "$N" "$*"; }
stop() { printf '\n  \033[31m✗ %s\033[0m\n' "$*" >&2; exit 1; }
sha()  { if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }
card_ok() { [ "$2" = 1 ] && [ "$1" -ge "$MIN_B" ] && [ "$1" -le "$MAX_B" ]; }	# SIZE_BYTES REMOVABLE(1|0), no side effects
# provision_txt: the file tfctl consumes on first boot (image/boot/provision.txt.example).
provision_txt() {
	printf 'WIFI_SSID="%s"\nWIFI_PSK="%s"\nSERVER_URL=%s\nFRAME_ID=%s\nFRAME_TOKEN=%s\nSSH_PUBKEY=%s\n' \
		"$WIFI_SSID" "$WIFI_PSK" "$SERVER_URL" "$FRAME_ID" "$FRAME_TOKEN" "$SSH_PUBKEY"
}
# list_cards: "dev<TAB>bytes<TAB>label" for every removable disk that passes card_ok, no system volume.
list_cards() {
	local name size rm model mnt sys='^(/|/System(/.*)?|/private/var/vm|/boot.*|/home|/usr|/var)$'
	if [ "$(uname)" = Darwin ]; then
		for name in $(diskutil list external physical | awk '/^\/dev\/disk/ {sub("/dev/",""); print $1}'); do
			# every reader drains its pipe: an early exit breaks the writer and pipefail empties the list
			size=$(diskutil info "$name" | awk -F'[()]' '/Disk Size/ && !s {split($2,a," "); s=a[1]} END {print s+0}')
			rm=$(diskutil info "$name" | awk '/Removable Media: +(Removable|Yes)/ {r=1} END {print r+0}')
			card_ok "${size:-0}" "$rm" || continue
			# a disk the Mac booted from, or that holds a system volume, is never a card
			mnt=$(mount | awk -v d="/dev/${name}s" 'index($1, d) == 1 {print $3}' | grep -E "$sys" || true)
			[ -z "$mnt" ] || continue
			printf '%s\t%s\t%s\n' "$name" "$size" "$(diskutil info "$name" | awk -F': +' '/Media Name/ && !m {m=$2} END {print m}')"
		done
	else
		while read -r name size rm model; do
			card_ok "$size" "$rm" || continue
			mnt=$(lsblk -nr -o MOUNTPOINT "/dev/$name" | grep -E "$sys" || true)
			[ -z "$mnt" ] || continue
			printf '%s\t%s\t%s\n' "$name" "$size" "$model"
		done < <(lsblk -dnb -o NAME,SIZE,RM,MODEL | grep -v '^loop\|^zram')
	fi
}
# fetch_image: IMAGE=<file> as is, else the latest release asset + .sha256 (cached in $IMG).
fetch_image() {
	if [ -n "${IMAGE:-}" ]; then [ -f "$IMAGE" ] || stop "no such image: $IMAGE"; IMG=$IMAGE; ok "local image $IMAGE"; return; fi
	command -v zstd >/dev/null || stop "zstd is required (brew install zstd / apt install zstd)"
	local api asset want
	api=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest")
	asset=$(grep -o '"browser_download_url": *"[^"]*thinframe-sd.img.zst"' <<<"$api" | cut -d'"' -f4)
	[ -n "$asset" ] || stop "no thinframe-sd.img.zst in the latest release of $REPO"
	want=$(curl -fsSL "$asset.sha256" | cut -d' ' -f1)
	local tag; tag=$(grep -o '"tag_name": *"[^"]*"' <<<"$api" | cut -d'"' -f4)
	if [ -f "$IMG" ] && [ -f "$IMG.zsum" ] && [ "$(cat "$IMG.zsum")" = "$want" ]; then ok "thinframe $tag (already downloaded)"; return; fi
	curl -fL --progress-bar -o "$IMG.zst" "$asset"
	[ "$(sha "$IMG.zst")" = "$want" ] || stop "download hash mismatch"
	printf '  unpacking…\r'; zstd -d -q -f "$IMG.zst" -o "$IMG" && rm -f "$IMG.zst" && printf '%s' "$want" > "$IMG.zsum"
	ok "thinframe $tag downloaded and checked"
}
# Runs as root. MiB 0 (partition table) is zeroed first and written last: macOS mounts PFBOOT on every
# close of a disk with a table and writes into it. Live dd bar (SIGINFO/USR1). Prints head + rest sha256.
# shellcheck disable=SC2016
ROOT_WRITE='img=$1 raw=$2 total=$3 mib=$((total / 1048576)) sig=USR1 log=$(mktemp)
[ "$(uname)" = Darwin ] && sig=INFO; s256() { sha256sum 2>/dev/null || shasum -a 256; }
bar() { local p=$(($2 * 100 / total)) t=$(($(date +%s) - $3)) w=28 f; f=$((p * w / 100)); [ $t -gt 0 ] || t=1
	local r=$(($2 / t / 1000000)) e=0; [ $r -gt 0 ] && e=$(((total - $2) / 1000000 / r))
	printf "\r  %-9s %s%s %3d%%  %3d MB/s  %d:%02d left " "$1" "$(printf "%${f}s" | tr " " "#")" \
		"$(printf "%$((w - f))s" | tr " " ".")" "$p" "$r" $((e / 60)) $((e % 60)) >/dev/tty; }
follow() { local s; s=$(date +%s); sleep 0.3; local pid; pid=$(pgrep -n -x dd)
	while kill -0 "$pid" 2>/dev/null; do kill -"$sig" "$pid" 2>/dev/null; sleep 1
		n=$(grep -ao "^[0-9]* bytes" "$log" | tail -1 | cut -d" " -f1); [ -z "$n" ] || bar "$1" "$n" "$s"; done
	bar "$1" "$total" "$s"; echo >/dev/tty; }
dd if=/dev/zero of="$raw" bs=1048576 count=1 conv=notrunc 2>/dev/null; sync; dd if="$img" of="$raw" bs=1048576 skip=1 seek=1 conv=notrunc 2>"$log" & w=$!; follow Writing; wait $w || exit 1; sync; : >"$log"
dd if="$raw" bs=1048576 skip=1 count=$((mib - 1)) 2>"$log" | s256 > "$log.sum" & follow Verifying
wait; dd if="$img" of="$raw" bs=1048576 count=1 conv=notrunc 2>/dev/null && sync || exit 1
echo "$(dd if="$raw" bs=1048576 count=1 2>/dev/null | s256 | cut -c1-64) $(cut -c1-64 "$log.sum")"; rm -f "$log" "$log.sum"'
# write_card DEV SIZE: re-check the card, unmount, write + read back as root, compare hashes.
write_card() {
	local dev="$1" bytes want got raw="/dev/$1"
	[ "$(list_cards | grep -c "^$dev	$2	")" = 1 ] || stop "$dev is no longer the card that was listed; re-run"
	bytes=$(wc -c < "$IMG" | tr -d ' '); printf '  %sreading the image (%s MB) to check the card against it…%s' "$D" $((bytes / 1000000)) "$N"
	want="$(dd if="$IMG" bs=1048576 count=1 2>/dev/null | sha -) $(dd if="$IMG" bs=1048576 skip=1 2>/dev/null | sha -)"; printf '\r\033[K'
	[ $((bytes % 1048576)) = 0 ] || stop "image size is not whole MiB"
	if [ "$(uname)" = Darwin ]; then diskutil unmountDisk "/dev/$dev" >/dev/null; raw="/dev/r$dev"; fi
	# shellcheck disable=SC2024  # the prompt reads the terminal, not a file
	sudo -v -p "  Your Mac password, to write the card: " </dev/tty || stop "sudo was refused"
	got=$(sudo bash -c "$ROOT_WRITE" _ "$IMG" "$raw" "$bytes")
	[ "$got" = "$want" ] || stop "the card reads back different from the image; do not use this card"; ok "written and verified"
}
# put_provision DEV: mount PFBOOT, write provision.txt, eject.
put_provision() {
	local dev="$1" mp
	if [ "$(uname)" = Darwin ]; then
		diskutil mountDisk "/dev/$dev" >/dev/null; mp=/Volumes/PFBOOT
		for _ in $(seq 30); do [ -d "$mp" ] && break; sleep 1; done
		[ -d "$mp" ] || stop "PFBOOT did not mount"
		provision_txt > "$mp/provision.txt"; touch "$mp/.metadata_never_index"
		[ -z "$TOUCH" ] || cp "$TOUCH" "$mp/gsl_m86_hd.fw"; diskutil eject "/dev/$dev" >/dev/null
	else
		mp=$(mktemp -d); sudo partprobe "/dev/$dev" 2>/dev/null || true; sleep 2
		sudo mount "/dev/${dev}1" "$mp" 2>/dev/null || sudo mount "/dev/${dev}p1" "$mp"
		provision_txt | sudo tee "$mp/provision.txt" >/dev/null; [ -z "$TOUCH" ] || sudo cp "$TOUCH" "$mp/"
		sudo umount "$mp"; rmdir "$mp"
		sudo eject "/dev/$dev" 2>/dev/null || true
	fi
	ok "settings${TOUCH:+ and touch firmware} on the card, card ejected"
}
ask() { local in; read -r -p "  $2${!1:+ [${!1}]}: " in </dev/tty; printf -v "$1" '%s' "${in:-${!1:-}}"
	case "${!1}" in *'"'*|*$'\n'*) stop "no quotes or newlines in answers";; esac; }
# settings: remembered answers, else this Mac's Wi-Fi and address; the password from the Keychain.
settings() {
	[ -n "${WIFI_SSID:-}" ] || WIFI_SSID=$(ipconfig getsummary en0 2>/dev/null | awk -F' SSID : ' '/ SSID : /{print $2; exit}' || true)
	SERVER_URL=${SERVER_URL:-}; FRAME_ID=${FRAME_ID:-}; FRAME_TOKEN=${FRAME_TOKEN:-}
	printf '  Wi-Fi   %s\n  Server  %s\n' "${WIFI_SSID:-none, pick it on the frame}" "${SERVER_URL:-found on the network}"
	local a; read -r -p "  Enter to use these, e to edit: " a </dev/tty
	if [ "$a" = e ]; then ask WIFI_SSID "Wi-Fi network (empty = pick it on the frame)"; ask SERVER_URL "Server (empty = find it)"; fi
	while [ -z "$FRAME_ID" ]; do ask FRAME_ID "Name for this frame (e.g. kitchen)"; done	# one per frame, never remembered
	FRAME_ID=$(printf %s "$FRAME_ID" | tr '[:upper:] ' '[:lower:]-')
	if [ -n "$WIFI_SSID" ] && [ -z "${WIFI_PSK:-}" ]; then
		WIFI_PSK=$(security find-generic-password -D "AirPort network password" -a "$WIFI_SSID" -w 2>/dev/null || true)
		if [ -n "$WIFI_PSK" ]; then ok "Wi-Fi password from this Mac's Keychain"; else read -r -s -p "  Password for $WIFI_SSID: " WIFI_PSK </dev/tty; echo; fi; fi
	TOUCH=${PF1007L_TOUCH_FIRMWARE:-$STATE/gsl_m86_hd.fw}; [ -f "$TOUCH" ] && [ "$(sha "$TOUCH")" = "$TOUCH_SHA" ] || TOUCH=""
	[ -n "$TOUCH" ] || printf '  %snote: no touch firmware in %s; the screen will not take taps (docs/new-frame.md)%s\n' "$D" "$STATE" "$N"
	(umask 077; printf 'WIFI_SSID=%q\nSERVER_URL=%q\n' "$WIFI_SSID" "$SERVER_URL" > "$STATE/answers")
	[ -f "$STATE/frame_id_ed25519" ] || ssh-keygen -q -t ed25519 -N "" -C "thinframe@$(hostname -s)" -f "$STATE/frame_id_ed25519"; SSH_PUBKEY=$(cat "$STATE/frame_id_ed25519.pub")
}
# wait_card: the one card, waiting (with a spinner) until it is inserted.
wait_card() {
	local cards n i=0 s='|/-+'
	while :; do cards=$(list_cards); n=$(grep -c . <<<"$cards" || true)
		[ "$n" -le 1 ] || { echo "$cards"; stop "more than one card; remove the others"; }
		[ "$n" = 0 ] || break; printf '\r  %s insert the SD card into this computer…' "${s:i++%4:1}"; sleep 1; done
	[ "$i" = 0 ] || printf '\r%60s\r' ''; IFS=$'\t' read -r DEV SIZE LABEL <<<"$cards"
}
main() {
	mkdir -p "$STATE"; chmod 700 "$STATE"
	# shellcheck disable=SC1091
	local name=${FRAME_ID:-}; [ -f "$STATE/answers" ] && . "$STATE/answers"; FRAME_ID=$name	# never an old frame's name
	printf '\n%sthinframe%s %sSD card setup%s\n' "$B" "$N" "$D" "$N"
	step 1 "Settings"; settings
	step 2 "Image"; if [ "${1:-}" = "--settings-only" ]; then ok "settings only, the card keeps its image"; else fetch_image; fi
	step 3 "Card"; wait_card
	local a; read -r -p "  Erase $DEV ($((SIZE / 1000000000)) GB, $LABEL)? [y/N] " a </dev/tty; [ "$a" = y ] || [ "$a" = Y ] || stop "nothing written"
	step 4 "Write"; [ "${1:-}" = "--settings-only" ] || write_card "$DEV" "$SIZE"; put_provision "$DEV"
	printf '\n%sDone.%s Put the card in the frame and power it on. It joins %s,\nfinds the server, then shows a pairing code: type it on the server /admin page.\n\n' "$B" "$N" "${WIFI_SSID:-the Wi-Fi you pick on screen}"
}
[ "${FLASH_SOURCE_ONLY:-}" = 1 ] || main "$@"
