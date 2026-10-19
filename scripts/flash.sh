#!/usr/bin/env bash
# One-command flasher for thinframe. macOS only (for now).
#   curl -fsSL https://raw.githubusercontent.com/MikeO7/thinframe/main/scripts/flash.sh | bash
# Asks four questions, downloads the latest release image, writes the card with guards, drops
# provision.txt on the boot partition, ejects. Re-run with "--settings-only" to rewrite
# provision.txt on an already-flashed card without a full write.
#
# TASK 14 implements the TODOs. Keep the user-facing flow exactly as docs/flashing.md describes:
# four questions, one typed confirmation of the disk name, one sudo. Nothing else.
set -euo pipefail

REPO="MikeO7/thinframe"
STATE="$HOME/.thinframe"               # answers, ssh key, downloaded image
IMG="$STATE/thinframe-sd.img"
mkdir -p "$STATE"; chmod 700 "$STATE"

say()  { printf '\n\033[1m%s\033[0m\n' "$*"; }
stop() { echo "STOP: $*" >&2; exit 1; }
[ "$(uname)" = Darwin ] || stop "macOS only for now (Linux: TASK 14b)"

# ---- 1. questions (remembered in $STATE/answers for re-runs) ------------------
ask() { # var prompt [secret]
	local v="$1" p="$2" s="${3:-}" cur="${!1:-}" in
	if [ -n "$s" ]; then read -r -s -p "$p: " in; echo; else read -r -p "$p${cur:+ [$cur]}: " in; fi
	printf -v "$v" '%s' "${in:-$cur}"
}
# shellcheck disable=SC1091
[ -f "$STATE/answers" ] && . "$STATE/answers"
say "thinframe flasher"
ask WIFI_SSID "Wi-Fi network"
ask WIFI_PSK  "Wi-Fi password" secret
ask SERVER_URL "Server (e.g. http://nas.local:8080)"
ask FRAME_ID  "Frame name (e.g. kitchen)"
ask FRAME_TOKEN "Server token (FRAME_TOKEN from the server's compose)" secret
umask 077; printf 'WIFI_SSID=%q\nSERVER_URL=%q\nFRAME_ID=%q\n' "$WIFI_SSID" "$SERVER_URL" "$FRAME_ID" > "$STATE/answers"
[ -f "$STATE/frame_id_ed25519" ] || ssh-keygen -q -t ed25519 -N "" -C "thinframe@$(hostname -s)" -f "$STATE/frame_id_ed25519"

# ---- 2. image ------------------------------------------------------------------
if [ "${1:-}" != "--settings-only" ]; then
	say "Downloading the latest image"
	# TODO(TASK 14): gh-less download of the latest release asset + .sha256 via the GitHub API
	#   curl -fsSL https://api.github.com/repos/$REPO/releases/latest | grep browser_download_url
	#   verify sha256; cache in $IMG; skip if cached and hash matches.
	stop "TASK 14: release download not implemented (build locally with image/build.sh and copy to $IMG)"
fi

# ---- 3. find the card ----------------------------------------------------------
say "Looking for a card"
# TODO(TASK 14): list external physical disks 4..64 GB via diskutil; show "disk4  31.9 GB  SD Card Reader";
#   if exactly one, propose it; require the user to type its name (disk4) to continue.
#   Reuse the guards of ormarch image/tools/write-card.py (refuses internal, non-removable, mounted system volumes).

# ---- 4. write ------------------------------------------------------------------
# TODO(TASK 14): diskutil unmountDisk; sudo dd if=$IMG of=/dev/rdiskN bs=4m; readback sha256 of the
#   image length; fail loudly on mismatch.

# ---- 5. provision.txt on PFBOOT -------------------------------------------------
# TODO(TASK 14): wait for /Volumes/PFBOOT to mount (diskutil mountDisk), write:
#   WIFI_SSID, WIFI_PSK, SERVER_URL, FRAME_ID, FRAME_TOKEN, SSH_PUBKEY=$(cat $STATE/frame_id_ed25519.pub)
#   then: touch .metadata_never_index; diskutil eject.
say "Done. Card in the frame, power on, wait a minute, then open $SERVER_URL/admin"
