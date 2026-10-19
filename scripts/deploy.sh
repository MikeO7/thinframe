#!/usr/bin/env bash
# Push the working tree to a running frame: www/, image/rootfs/ overlay, and tfctl if built.
# No rsync on the frame (not in the image): stream a tar over ssh.
# Usage: scripts/deploy.sh frame@thinframe
set -euo pipefail
HOST="${1:?usage: deploy.sh user@host}"
HERE="$(cd "$(dirname "$0")/.." && pwd)"
AFCTL="$HERE/tfctl/target/armv7-unknown-linux-musleabihf/release/tfctl"
tar -C "$HERE/www" -cf - . | ssh "$HOST" 'doas rm -rf /var/lib/thinframe/www/* && doas tar -C /var/lib/thinframe/www -xf -'
tar -C "$HERE/image/rootfs" --exclude etc/thinframe.conf --exclude 'etc/ssh/*' --exclude 'etc/iwd/*' -cf - . \
	| ssh "$HOST" 'doas tar -C / -xf -'
if [ -x "$AFCTL" ]; then
	ssh "$HOST" 'cat > /tmp/tfctl && doas install -m 755 /tmp/tfctl /usr/bin/tfctl' < "$AFCTL"
fi
ssh "$HOST" 'doas rc-service thinframe restart && tfctl status'
