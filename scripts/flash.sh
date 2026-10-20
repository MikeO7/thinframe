#!/usr/bin/env bash
# One-command flasher for thinframe (macOS and Linux). Spec: docs/flashing.md.
#   curl -fsSL https://raw.githubusercontent.com/MikeO7/thinframe/main/scripts/flash.sh | bash
# Four questions, latest release image (sha256 checked), a removable 4-64 GB card only,
# one typed confirmation, one sudo (write + readback), provision.txt onto PFBOOT, eject.
# "--settings-only" rewrites provision.txt on an already-flashed card. Never touches the eMMC.
set -euo pipefail

REPO="MikeO7/thinframe"
STATE="${THINFRAME_STATE:-$HOME/.thinframe}"
IMG="$STATE/thinframe-sd.img"
MIN_B=$((4 * 1000 ** 3)); MAX_B=$((64 * 1000 ** 3))

say()  { printf '\n\033[1m%s\033[0m\n' "$*"; }
stop() { echo "STOP: $*" >&2; exit 1; }
sha()  { if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1; }

# card_ok SIZE_BYTES REMOVABLE(1|0): the size/removable guard, no side effects.
card_ok() { [ "$2" = 1 ] && [ "$1" -ge "$MIN_B" ] && [ "$1" -le "$MAX_B" ]; }

# provision_txt: the file tfctl consumes on first boot (image/boot/provision.txt.example).
provision_txt() {
	printf 'WIFI_SSID="%s"\nWIFI_PSK="%s"\nSERVER_URL=%s\nFRAME_ID=%s\nFRAME_TOKEN=%s\nSSH_PUBKEY=%s\n' \
		"$WIFI_SSID" "$WIFI_PSK" "$SERVER_URL" "$FRAME_ID" "$FRAME_TOKEN" "$SSH_PUBKEY"
}

# list_cards: "dev<TAB>bytes<TAB>label" for every removable disk that passes card_ok and has
# no mounted system volume.
list_cards() {
	local name size rm model mnt
	if [ "$(uname)" = Darwin ]; then
		for name in $(diskutil list external physical | awk '/^\/dev\/disk/ {sub("/dev/",""); print $1}'); do
			size=$(diskutil info "$name" | awk -F'[()]' '/Disk Size/ {split($2,a," "); print a[1]; exit}')
			rm=$(diskutil info "$name" | grep -qE 'Removable Media: +(Removable|Yes)' && echo 1 || echo 0)
			card_ok "${size:-0}" "$rm" || continue
			printf '%s\t%s\t%s\n' "$name" "$size" "$(diskutil info "$name" | awk -F': +' '/Media Name/ {print $2; exit}')"
		done
	else
		while read -r name size rm model; do
			card_ok "$size" "$rm" || continue
			mnt=$(lsblk -nr -o MOUNTPOINT "/dev/$name" | grep -E '^(/|/boot.*|/home|/usr|/var)$' || true)
			[ -z "$mnt" ] || continue
			printf '%s\t%s\t%s\n' "$name" "$size" "$model"
		done < <(lsblk -dnb -o NAME,SIZE,RM,MODEL | grep -v '^loop\|^zram')
	fi
}

# fetch_image: latest release asset + .sha256, cached in $IMG when the hash matches.
fetch_image() {
	command -v zstd >/dev/null || stop "zstd is required (brew install zstd / apt install zstd)"
	local api asset sumurl want
	api=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest")
	asset=$(grep -o '"browser_download_url": *"[^"]*thinframe-sd.img.zst"' <<<"$api" | cut -d'"' -f4)
	[ -n "$asset" ] || stop "no thinframe-sd.img.zst in the latest release of $REPO"
	sumurl="$asset.sha256"
	want=$(curl -fsSL "$sumurl" | cut -d' ' -f1)
	if [ -f "$IMG" ] && [ -f "$IMG.zsum" ] && [ "$(cat "$IMG.zsum")" = "$want" ]; then echo "cached image is current"; return; fi
	curl -fL --progress-bar -o "$IMG.zst" "$asset"
	[ "$(sha "$IMG.zst")" = "$want" ] || stop "download hash mismatch"
	zstd -d -q -f "$IMG.zst" -o "$IMG" && rm -f "$IMG.zst" && printf '%s' "$want" > "$IMG.zsum"
}

# write_card DEV: unmount, one sudo to write and read back, compare hashes.
write_card() {
	local dev="$1" bytes want got raw="/dev/$1" bs=4M
	bytes=$(wc -c < "$IMG" | tr -d ' '); want=$(sha "$IMG")
	if [ "$(uname)" = Darwin ]; then diskutil unmountDisk "/dev/$dev" >/dev/null; raw="/dev/r$dev"; bs=4m; fi
	say "Writing $bytes bytes to $raw (sudo asks for your password once)"
	got=$(sudo bash -c "dd if='$IMG' of='$raw' bs=$bs conv=sync 2>/dev/null; sync; \
		head -c $bytes '$raw' | { sha256sum 2>/dev/null || shasum -a 256; }" | cut -d' ' -f1)
	[ "$got" = "$want" ] || stop "readback mismatch ($got != $want); do not use this card"
	echo "verified"
}

# put_provision DEV: mount PFBOOT, write provision.txt, eject.
put_provision() {
	local dev="$1" mp
	if [ "$(uname)" = Darwin ]; then
		diskutil mountDisk "/dev/$dev" >/dev/null; mp=/Volumes/PFBOOT
		for _ in $(seq 30); do [ -d "$mp" ] && break; sleep 1; done
		[ -d "$mp" ] || stop "PFBOOT did not mount"
		provision_txt > "$mp/provision.txt"; touch "$mp/.metadata_never_index"; diskutil eject "/dev/$dev" >/dev/null
	else
		mp=$(mktemp -d); sudo partprobe "/dev/$dev" 2>/dev/null || true; sleep 2
		sudo mount "/dev/${dev}1" "$mp" 2>/dev/null || sudo mount "/dev/${dev}p1" "$mp"
		provision_txt | sudo tee "$mp/provision.txt" >/dev/null; sudo umount "$mp"; rmdir "$mp"
		sudo eject "/dev/$dev" 2>/dev/null || true
	fi
}

main() {
	mkdir -p "$STATE"; chmod 700 "$STATE"
	# shellcheck disable=SC1091
	[ -f "$STATE/answers" ] && . "$STATE/answers"
	say "thinframe flasher"
	ask() { local in; if [ "${3:-}" ]; then read -r -s -p "$2: " in </dev/tty; echo; else read -r -p "$2${!1:+ [${!1}]}: " in </dev/tty; fi
		printf -v "$1" '%s' "${in:-${!1:-}}"; case "${!1}" in *'"'*|*$'\n'*) stop "no quotes or newlines in answers";; esac; }
	ask WIFI_SSID "Wi-Fi network"; ask WIFI_PSK "Wi-Fi password" secret
	ask SERVER_URL "Server (e.g. http://nas.local:8080)"; ask FRAME_ID "Frame name (e.g. kitchen)"
	ask FRAME_TOKEN "Server token" secret
	(umask 077; printf 'WIFI_SSID=%q\nSERVER_URL=%q\nFRAME_ID=%q\n' "$WIFI_SSID" "$SERVER_URL" "$FRAME_ID" > "$STATE/answers")
	[ -f "$STATE/frame_id_ed25519" ] || ssh-keygen -q -t ed25519 -N "" -C "thinframe@$(hostname -s)" -f "$STATE/frame_id_ed25519"
	SSH_PUBKEY=$(cat "$STATE/frame_id_ed25519.pub")
	[ "${1:-}" = "--settings-only" ] || { say "Downloading the latest image"; fetch_image; }
	say "Looking for a card"
	local cards n dev ans; cards=$(list_cards); n=$(grep -c . <<<"$cards" || true)
	[ "$n" -ge 1 ] || stop "no removable 4-64 GB card found (insert it and re-run)"
	[ "$n" -eq 1 ] || { echo "$cards"; stop "more than one candidate; remove the others"; }
	IFS=$'\t' read -r dev size label <<<"$cards"
	echo "Found: $dev  $((size / 1000000000)) GB  $label"
	read -r -p "Type the disk name ($dev) to erase it and continue: " ans </dev/tty
	[ "$ans" = "$dev" ] || stop "confirmation did not match"
	[ "${1:-}" = "--settings-only" ] || write_card "$dev"
	put_provision "$dev"
	say "Done. Card in the frame, power on, wait a minute, then open $SERVER_URL/admin"
}

[ "${FLASH_SOURCE_ONLY:-}" = 1 ] || main "$@"
