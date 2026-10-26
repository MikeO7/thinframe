#!/usr/bin/env bash
# Build the server image on this Mac and run it on a Docker host over ssh (production).
#   scripts/server-deploy.sh HOST [DIR]      DIR on the host, default ~/thinframe
# The host keeps: DIR/compose.yml (written here), DIR/env (FRAME_TOKEN and your settings,
# made once, mode 600, never overwritten), DIR/data (frames, sources, assets).
# The container uses host networking (frames find it by UDP discovery on 41007) and reads
# the container list from the host's Docker socket for the services widget.
# Tagged local/thinframe-server: update managers that skip local/* leave it alone.
set -euo pipefail
host=${1:?usage: server-deploy.sh HOST [DIR]}
dir=${2:-thinframe}
root=$(cd "$(dirname "$0")/.." && pwd)
G=$'\033[32m' N=$'\033[0m'
ok() { printf '  %s✓%s %s\n' "$G" "$N" "$*"; }

case $(ssh -o BatchMode=yes "$host" uname -m) in
	aarch64 | arm64) arch=arm64 ;;
	x86_64) arch=amd64 ;;
	*) echo "unsupported host architecture" >&2; exit 1 ;;
esac
ver=$(git -C "$root" rev-parse --short HEAD)
tag=local/thinframe-server:$ver
docker build --platform "linux/$arch" --build-arg VERSION="$ver" -t "$tag" "$root/server" >/dev/null
ok "built $tag for linux/$arch"
docker save "$tag" | gzip -1 | ssh -o BatchMode=yes "$host" 'gunzip | docker load' >/dev/null
ok "image loaded on $host"

# shellcheck disable=SC2087 # expand $tag and $dir here, on purpose
ssh -o BatchMode=yes "$host" bash -s <<EOF
set -euo pipefail
cd ~ && mkdir -p "$dir/data" && chmod 700 "$dir" && cd "$dir"
if [ ! -f env ]; then
	tz=\$(readlink /etc/localtime 2>/dev/null | sed 's#.*/zoneinfo/##')
	(umask 077; printf 'FRAME_TOKEN=%s\nTZ=%s\n' "\$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')" "\${tz:-UTC}" > env)
	chmod 600 env  # a default ACL can widen what umask allowed
	echo "  made env with a new FRAME_TOKEN"
fi
cat > compose.yml <<'YML'
# Written by scripts/server-deploy.sh; edit env, not this file.
services:
  thinframe:
    image: $tag
    container_name: thinframe
    restart: unless-stopped
    network_mode: host
    env_file: env
    environment:
      DATA_DIR: /data
      SERVICES_DOCKER: /var/run/docker.sock
    volumes:
      - ./data:/data
      - /var/run/docker.sock:/var/run/docker.sock:ro
      - /etc/localtime:/etc/localtime:ro
      - /usr/share/zoneinfo:/usr/share/zoneinfo:ro
YML
docker compose -p thinframe up -d --remove-orphans >/dev/null
for _ in \$(seq 30); do
	curl -fsS -m 2 http://127.0.0.1:8080/healthz >/dev/null 2>&1 && break
	sleep 1
done
curl -fsS -m 2 http://127.0.0.1:8080/healthz >/dev/null
EOF
ok "running on $host:8080 ($tag)"
ssh -o BatchMode=yes "$host" "docker logs --tail 5 thinframe 2>&1"
