#!/usr/bin/env bash
# shellcheck disable=SC2329  # sourced by qemu-journey-test.sh after qemu-lib.sh (W, SHOTS)
# The screen of a SCREEN=1 virt guest: QMP multitouch taps, monitor screendumps, the on-screen
# keyboard. Pages are landscape (rotation 3) on the 800x1280 portrait panel: page x,y is panel 799-y,x.
qmp() { printf '{"execute":"qmp_capabilities"}\n%s\n' "$1" | nc -U -w 2 "$W/qmp.sock" >/dev/null; }
tap() {	# page x y. Axis x is the panel's short side (800): panel x = 799 - page y, panel y = page x.
	local ax=$(( (799 - $2) * 32767 / 799 )) ay=$(( $1 * 32767 / 1279 ))
	mt() { printf '{"type":"mtt","data":{"type":"%s","slot":0,"tracking-id":%s,"axis":"%s","value":%s}}' "$@"; }
	qmp "{\"execute\":\"input-send-event\",\"arguments\":{\"events\":[$(mt begin 1 x "$ax"),$(mt data 1 x "$ax"),$(mt data 1 y "$ay")]}}"
	sleep 0.15
	qmp "{\"execute\":\"input-send-event\",\"arguments\":{\"events\":[$(mt end -1 x "$ax")]}}"
}
grab() {	# file: one screendump
	rm -f "$1"; echo "screendump $1 -f png" | nc -U -w 3 "$W/mon.sock" >/dev/null || true
	local n=0; until [ -s "$1" ] || [ "$n" -ge 20 ]; do sleep 0.5; n=$((n + 1)); done; [ -s "$1" ]
}
sum() { shasum -a 256 "$1" | cut -c1-64; }
size() { stat -f %z "$1" 2>/dev/null || stat -c %s "$1"; }
# snap name [seconds]: a painted screen (not cog's bare background, over 8 KB as PNG) that held
# still for two grabs; TCG paints a page some seconds after it loads. Saved as journey/<name>.png.
LAST=""
snap() {
	local prev="" cur end=$(( $(date +%s) + ${2:-180} ))
	while [ "$(date +%s)" -lt "$end" ]; do
		grab "$W/s.png" || { sleep 3; continue; }; cur="$(sum "$W/s.png")"
		if [ "$cur" = "$prev" ] && [ "$(size "$W/s.png")" -gt 8000 ]; then
			cp "$W/s.png" "$SHOTS/$1.png"; LAST="$cur"; return 0
		fi
		prev="$cur"; sleep 3
	done
	cp "$W/s.png" "$SHOTS/$1-timeout.png" 2>/dev/null || true; return 1
}
changed() {	# name [seconds]: the screen differs from the last snap and holds still (a key landed)
	local prev="" cur end=$(( $(date +%s) + ${2:-60} ))
	while [ "$(date +%s)" -lt "$end" ]; do
		grab "$W/s.png" || continue; cur="$(sum "$W/s.png")"
		if [ "$cur" = "$prev" ] && [ "$cur" != "$LAST" ]; then cp "$W/s.png" "$SHOTS/$1.png"; LAST="$cur"; return 0; fi
		prev="$cur"; sleep 2
	done
	return 1
}
type_text() {	# the on-screen keyboard, lower-case letters and space
	local rows=("qwertyuiop" "asdfghjkl" "zxcvbnm") ys=(247 331 416) x0=(167 219 324) c i k
	for ((i = 0; i < ${#1}; i++)); do
		c="${1:i:1}"
		# A trailing space paints nothing, so do not wait long for it.
		if [ "$c" = " " ]; then tap 640 500; changed "key-$i" 15 || true; continue; fi
		for k in 0 1 2; do
			case "${rows[k]}" in *"$c"*) local pre="${rows[k]%%"$c"*}"; tap $(( x0[k] + ${#pre} * 105 )) "${ys[k]}" ;; esac
		done
		changed "key-$i" 60 || true
	done
}

