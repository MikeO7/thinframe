#!/usr/bin/env bash
# Load the Obsidian demo onto a frame: the layout, then every sample source.
# Usage: FRAME_TOKEN=... load.sh [server-url] [frame-id]   (default http://localhost:41007 demo)
# Sample dates are written as @D0..@D6 (today .. today+6) and filled in here.
set -euo pipefail
url=${1:-http://localhost:41007}
id=${2:-demo}
here=$(cd "$(dirname "$0")" && pwd)
auth=()
if [[ -n ${FRAME_TOKEN:-} ]]; then auth=(-H "Authorization: Bearer $FRAME_TOKEN"); fi

day() { date -v+"$1"d +%F 2>/dev/null || date -d "+$1 day" +%F; }
subst=()
for n in 0 1 2 3 4 5 6; do subst+=(-e "s/@D$n/$(day "$n")/g"); done

curl -fsS -X PUT ${auth[@]+"${auth[@]}"} -H 'Content-Type: application/json' \
  --data-binary @"$here/layout.json" "$url/api/frames/$id/layout" >/dev/null
echo "layout -> $id"
for f in "$here"/sources/*.json; do
  key=$(basename "$f" .json)
  sed "${subst[@]}" "$f" | curl -fsS -X PUT ${auth[@]+"${auth[@]}"} -H 'Content-Type: application/json' \
    --data-binary @- "$url/api/sources/$key" >/dev/null
  echo "source $key"
done
