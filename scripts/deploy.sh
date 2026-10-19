#!/usr/bin/env bash
# Push the working tree to a running frame: www/, image/rootfs/ overlay, and afctl if built.
# No rsync on the frame (not in the image): stream a tar over ssh.
# Usage: scripts/deploy.sh frame@alpineframe
set -euo pipefail
HOST="${1:?usage: deploy.sh user@host}"
HERE="$(cd "$(dirname "$0")/.." && pwd)"
AFCTL="$HERE/afctl/target/armv7-unknown-linux-musleabihf/release/afctl"
tar -C "$HERE/www" -cf - . | ssh "$HOST" 'doas rm -rf /var/lib/alpineframe/www/* && doas tar -C /var/lib/alpineframe/www -xf -'
tar -C "$HERE/image/rootfs" --exclude etc/alpineframe.conf --exclude 'etc/ssh/*' --exclude 'etc/iwd/*' -cf - . \
	| ssh "$HOST" 'doas tar -C / -xf -'
if [ -x "$AFCTL" ]; then
	ssh "$HOST" 'cat > /tmp/afctl && doas install -m 755 /tmp/afctl /usr/bin/afctl' < "$AFCTL"
fi
ssh "$HOST" 'doas rc-service alpineframe restart && afctl status'
