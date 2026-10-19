#!/usr/bin/env bash
# Write image/out/thinframe-sd.img to a microSD card on macOS, with guards.
# Usage: scripts/write-card-macos.sh image/out/thinframe-sd.img [/dev/diskN]
# Without a disk argument it lists candidates and exits.
# Guards (TASK 3): refuse non-removable, refuse >64 GB or <4 GB, refuse disks with a mounted
# system/home volume, require typed confirmation of the disk name, verify readback hash.
# ormarch has a hardened Python version: image/tools/write-card.py. Reuse it rather than
# re-inventing the guards.
set -euo pipefail
IMG="${1:?usage: write-card-macos.sh image.img [/dev/diskN]}"
DISK="${2:-}"
[ -f "$IMG" ] || { echo "no such image: $IMG" >&2; exit 1; }
if [ -z "$DISK" ]; then
	echo "Candidate disks:"; diskutil list external physical; exit 0
fi
echo "TODO(TASK 3): implement guards, then: diskutil unmountDisk $DISK && sudo dd if=$IMG of=${DISK/disk/rdisk} bs=4m status=progress && diskutil eject $DISK"
exit 1
