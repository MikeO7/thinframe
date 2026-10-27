#!/usr/bin/env bash
# U-Boot build "E" for `tfctl emmc install`: ormarch's build L (same U-Boot commit, patches 0001
# and 0002, container) plus image/uboot/pf1007l_emmc_defconfig and sun8i-a33-pf1007l-emmc.dts,
# which differ from build L only in the eMMC slot and a boot command that reads mmc_bootdev.
# Run by `image/build.sh inputs` after ormarch's `build.sh uboot`. Output in work/inputs/.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OM="$HERE/cache/ormarch"
IN="$HERE/work/inputs"
stop() { echo "STOP: $*" >&2; exit 1; }
C=$(sed -n 's/^UBOOT_COMMIT="\([0-9a-f]*\)".*/\1/p' "$OM/image/build.sh")
EPOCH=$(sed -n 's/^UBOOT_EPOCH=\([0-9]*\).*/\1/p' "$OM/image/build.sh")
if [ -z "$C" ] || [ -z "$EPOCH" ]; then stop "cannot read UBOOT_COMMIT/UBOOT_EPOCH from ormarch"; fi
SRC="$OM/image/cache/u-boot"
[ -d "$OM/refs/photo-frame-dashboard/diagnostics/u-boot/.git" ] && SRC="$OM/refs/photo-frame-dashboard/diagnostics/u-boot"
git -C "$SRC" cat-file -e "$C^{commit}" || stop "U-Boot $C not in $SRC (run ormarch build.sh uboot)"
docker image inspect pf1007l-uboot-build >/dev/null || stop "no pf1007l-uboot-build image (ormarch build.sh uboot)"
W="$HERE/work/uboot-emmc"; rm -rf "$W"; mkdir -p "$W" "$IN"
git -C "$SRC" archive --format=tar "$C" | docker run --rm -i -e SOURCE_DATE_EPOCH="$EPOCH" \
	-v "$OM/image/uboot":/u:ro -v "$HERE/uboot":/e:ro -v "$W":/out pf1007l-uboot-build bash -c '
set -euo pipefail
export CROSS_COMPILE=arm-linux-gnueabihf-
mkdir -p /tmp/src && cd /tmp/src && tar -xf -
patch -s -p1 < /u/0001-wes-pf1007l-uboot.patch
patch -s -p1 < /u/0002-pf1007l-linux-boot.patch
cp /e/pf1007l_emmc_defconfig configs/
cp /e/sun8i-a33-pf1007l-emmc.dts arch/arm/dts/
sed -i "s|^\tsun8i-a33-pf1007l-linux.dtb \\\\|&\n\tsun8i-a33-pf1007l-emmc.dtb \\\\|" arch/arm/dts/Makefile
grep -q "sun8i-a33-pf1007l-emmc.dtb" arch/arm/dts/Makefile
make -s O=/build pf1007l_emmc_defconfig >/dev/null
make -s O=/build -j"$(nproc)" > /out/build.log 2>&1 || { tail -40 /out/build.log; exit 1; }
cp /build/u-boot-sunxi-with-spl.bin /out/u-boot-pf1007l-E.bin
cp /build/.config /out/u-boot-pf1007l-E.config'
B="$W/u-boot-pf1007l-E.bin"; CFG="$W/u-boot-pf1007l-E.config"; L="$IN/u-boot-pf1007l-L.config"
python3 -I - "$B" <<'PY'
import sys
d = open(sys.argv[1], 'rb').read()
end = max(i for i, b in enumerate(d[:0x6000]) if b) + 1
print(f"build E: {len(d)} bytes, SPL code ends at {end:#x} (limit 0x5e00)")
if d[4:12] != b"eGON.BT0": sys.exit("STOP: no eGON header")
if end > 0x5e00: sys.exit("STOP: SPL is too close to the stack/global-data area")
if len(d) > (4 << 20) - 8192: sys.exit("STOP: does not fit before partition 1 (4 MiB)")
if b"ums: refused write" not in d: sys.exit("STOP: the read-only USB disk patch is not in the binary")
PY
grep -q '^CONFIG_MMC_SUNXI_SLOT_EXTRA=2$' "$CFG" || stop "build E has no eMMC slot"
# Build E must be build L in everything but the eMMC slot, the device tree and the boot command.
if [ -f "$L" ]; then
	d=$(diff <(grep '^CONFIG_' "$L" | sort) <(grep '^CONFIG_' "$CFG" | sort) | grep '^[<>]' |
		grep -v -E 'CONFIG_(MMC_SUNXI_SLOT_EXTRA|FASTBOOT_FLASH_MMC_DEV|DEFAULT_DEVICE_TREE|BOOTCOMMAND|OF_LIST|DEVICE_TREE_INCLUDES|SYS_CONFIG_NAME)=' || true)
	[ -z "$d" ] || { echo "$d"; stop "build E differs from build L in more than the eMMC"; }
fi
cp "$B" "$IN/u-boot-pf1007l-E.bin"; cp "$CFG" "$IN/"
echo "build E: config checks OK; $IN/u-boot-pf1007l-E.bin"
