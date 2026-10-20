#!/usr/bin/env bash
# Run Buildroot for image/build.sh (TASK 2/3). Called with the pins exported by build.sh.
# Volumes: thinframe-br (Buildroot tree + output; a Linux file system, fast and case-sensitive)
# and thinframe-ccache. Downloads go to image/cache/dl so they survive `docker volume rm`.
# Writes only image/out/ on the Mac. No sudo, no devices.
set -euo pipefail
: "${BUILDROOT_VERSION:?}" "${BUILDROOT_SHA256:?}" "${THINFRAME_VERSION:?}"
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
export TGZ="buildroot-$BUILDROOT_VERSION.tar.xz"
mkdir -p "$HERE/cache/dl" "$HERE/out"
[ -f "$HERE/cache/$TGZ" ] || curl -fL -o "$HERE/cache/$TGZ" "https://buildroot.org/downloads/$TGZ"
[ "$(shasum -a 256 "$HERE/cache/$TGZ" | cut -d' ' -f1)" = "$BUILDROOT_SHA256" ] || { echo "STOP: $TGZ hash" >&2; exit 1; }

docker build -q -t thinframe-br -f "$HERE/Dockerfile.br" "$HERE" >/dev/null
# shellcheck disable=SC2016
docker run --rm -i \
	-v thinframe-br:/br -v thinframe-ccache:/ccache -v "$HERE/cache/dl":/dl -v "$ROOT":/src \
	-e THINFRAME_VERSION -e BUILDROOT_VERSION -e TGZ -e JOBS="${JOBS:-}" \
	-e THINFRAME_WORK=/src/image/work -e BRTARGET="${BRTARGET:-}" -e VARIANT="${VARIANT:-}" \
	thinframe-br bash -euo pipefail -c '
		BR=/br/buildroot-$BUILDROOT_VERSION
		[ -f "$BR/Makefile" ] || tar -xJf "/src/image/cache/$TGZ" -C /br
		S=/src/image/secrets W=/src/image/work/secrets
		if [ -f "$S/ssh_host_ed25519_key" ] && [ -d "$W" ]; then
			dropbearconvert openssh dropbear "$S/ssh_host_ed25519_key" "$W/dropbear_ed25519_host_key" 2>/dev/null
		fi
		# Variants (debug, qemu) get their own tree: Buildroot never removes a package.
		O=/br/output${VARIANT:+-$VARIANT} DEST=/src/image/out
		[ "$VARIANT" != qemu ] || DEST=/src/image/out/qemu
		make -s -C "$BR" O=$O BR2_EXTERNAL=/src/image/buildroot pf1007l_defconfig
		if [ -n "$VARIANT" ]; then
			F=/src/image/buildroot/board/pf1007l/$VARIANT.fragment
			cat "$F" >> $O/.config
			make -s -C "$BR" O=$O olddefconfig
			# olddefconfig drops options whose dependencies are unmet; refuse that silently shrinking.
			grep "^BR2_" "$F" | while IFS= read -r o; do
				grep -qxF "$o" $O/.config || { echo "STOP: $VARIANT.fragment: $o dropped by olddefconfig" >&2; exit 1; }
			done
		fi
		make -s -C "$BR" O=$O tfctl-dirclean	# local package: always take the freshly staged binary
		# WebKit: ~4.5 GB per compile job. GeneratedSerializers.cpp alone OOM-killed cc1plus at 5 jobs
		# in 16 GB (8 jobs failed earlier too); GitHub runners have 16 GB as well.
		if [ -z "$JOBS" ]; then
			JOBS=$(( $(awk "/MemTotal/ {print \$2}" /proc/meminfo) / 4500000 ))
			[ "$JOBS" -ge 1 ] || JOBS=1; [ "$JOBS" -le "$(nproc)" ] || JOBS=$(nproc)
		fi
		echo "buildroot: $JOBS jobs"
		start=$(date +%s)
		set +o pipefail
		make -C "$BR" O=$O ${JOBS:+BR2_JLEVEL=$JOBS} $BRTARGET 2>&1 | tee $O/build.log | grep --line-buffered "^>>> "
		rc=${PIPESTATUS[0]}; set -o pipefail
		[ "$rc" = 0 ] || { tail -60 $O/build.log; exit 1; }
		echo "buildroot: $(( ($(date +%s) - start) / 60 )) min"
		[ -n "$BRTARGET" ] && exit 0
		I=$O/images
		mkdir -p $DEST
		cp $I/thinframe-sd.img $I/system-$THINFRAME_VERSION.tar $I/rootfs.ext4 $I/zImage $I/*.dtb \
			$I/boot.scr $I/slot-a.conf $DEST/
		cp $O/build.log $DEST/buildroot.log
		ls -l $I
	'
