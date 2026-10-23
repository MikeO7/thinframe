#!/usr/bin/env bash
# Runs INSIDE the thinframe-uboot container (Dockerfile next to this file), started by
# image/uboot/build.sh. stdin: a tar of the pristine U-Boot tree. /u = image/uboot (read-only).
# /out = image/work/uboot. Applies patches/, builds the frame's U-Boot and the QEMU orangepi-pc
# one from the same tree, then checks the frame binary before it leaves.
set -euo pipefail
export CROSS_COMPILE=arm-linux-gnueabihf-
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-1791763200}"	# fixed build time: reproducible bytes
stop() { echo "STOP: $*" >&2; exit 1; }

mkdir -p /tmp/src && cd /tmp/src && tar -xf -
for p in /u/patches/*.patch; do patch -s -p1 < "$p" || stop "$(basename "$p") does not apply"; done

build() {	# $1 = defconfig, $2 = output name
	rm -rf /build && mkdir /build
	make -s O=/build "$1" > /dev/null
	make -s O=/build -j"$(nproc)" > "/out/$2.build.log" 2>&1 || { tail -40 "/out/$2.build.log"; stop "$2 did not build"; }
	cp /build/u-boot-sunxi-with-spl.bin "/out/$2.bin"
	cp /build/.config "/out/$2.config"
	cp /build/u-boot.dtb "/out/$2.dtb"
	echo "built $2: $(stat -c %s "/out/$2.bin") bytes, warnings: $(grep -ci warning "/out/$2.build.log" || true)"
}
build pf1007l_defconfig            u-boot-pf1007l
build pf1007l_qemu_opipc_defconfig u-boot-qemu-opipc

# Frame binary checks (docs/decisions.md). The SPL lives in the first 0x6000 bytes and must end
# before the early stack and global data at 0x5e00, or the A33 boot ROM runs into them.
bin=/out/u-boot-pf1007l.bin; cfg=/out/u-boot-pf1007l.config
[ "$(dd if="$bin" bs=1 skip=4 count=8 status=none)" = "eGON.BT0" ] || stop "no eGON header"
end=$(head -c 24576 "$bin" | od -An -v -tx1 -w1 | grep -n -v '^ 00$' | tail -1 | cut -d: -f1)
printf 'SPL code ends at %#x (limit 0x5e00)\n' "$end"
[ "$end" -le $((0x5e00)) ] || stop "SPL is too close to the stack/global-data area"
grep -qa "ums: refused write" "$bin" || stop "the read-only ums code is not in the binary"
for v in MMC_SUNXI_SLOT_EXTRA=-1 SUNXI_CONSOLE_NO_PINMUX=y CMD_UMS_READ_ONLY=y CMD_UMS_NO_ABORT=y \
	DRAM_ZQ=31739 DRAM_CLK=456 AXP_DCDC1_VOLT=3300 AXP_DCDC2_VOLT=1200 AXP_DCDC3_VOLT=1200 \
	AXP_ALDO1_VOLT=3300 CONS_INDEX=1 BOOTDELAY=-2; do
	grep -q "^CONFIG_$v\$" "$cfg" || stop "config check failed: $v"
done
echo "config checks: OK (DRAM, voltages, console, no eMMC slot, read-only USB disk)"
