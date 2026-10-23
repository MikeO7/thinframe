# PF1007L boot script (image/uboot/boot.cmd): boot counter, A/B slot choice, USB disk fallback.
# U-Boot loads /boot.scr from partition 1 (FAT) of the SD card and runs it.
# post-image.sh makes boot.scr from it with mkimage on every image/build.sh rootfs.
# For a change with no tools: put variables in /uEnv.txt (plain text).
#
# What it does:
#   1. Reads /pf/uboot.env  (state from the last start, written by this script)
#      and  /pf/linux-ok.env (written by Linux when the Mac configured the USB gadget).
#   2. Counts the starts (pf_boots, a HEX number).
#   3. Decides: Linux, or "USB disk mode" (ums) when the last Linux start gave no sign on USB.
#   4. Writes /pf/uboot.env again (one small file) BEFORE it starts Linux.
#   5. Starts Linux with /extlinux/extlinux.conf. If no label can load: USB disk mode.
#
# This script writes only to "mmc 0:1" (the SD card). This U-Boot has no eMMC device.

setenv pf_script tf1
setenv pf_mode auto
setenv pf_extlinux /extlinux/extlinux.conf
setenv pf_boots 0
setenv pf_last none

if load mmc 0:1 ${ramdisk_addr_r} /pf/uboot.env; then
	env import -t -r ${ramdisk_addr_r} ${filesize} pf_boots pf_last
fi
if load mmc 0:1 ${ramdisk_addr_r} /pf/linux-ok.env; then
	env import -t -r ${ramdisk_addr_r} ${filesize} pf_ok
fi
# Owner settings. Examples: pf_mode=linux | pf_mode=ums | pf_extlinux=/extlinux/other.conf
if load mmc 0:1 ${ramdisk_addr_r} /uEnv.txt; then
	env import -t -r ${ramdisk_addr_r} ${filesize}
fi

# thinframe A/B (TASK 3). uEnv.txt carries slot=a|b and tries=N (tfctl slot.rs writes it).
# tries=0: confirmed slot or fresh card; nothing is written. tries=N>0: this start spends one
# try BEFORE the kernel loads, so a later power cut cannot undo it, and the start that takes it
# to 0 boots the other slot (tries=3 = two starts on the new slot). setexpr prints hex and test
# reads decimal, so N stays below 10 (sysupdate.rs). pf_mode=linux in uEnv.txt keeps the USB
# disk fallback below for an explicit pf_mode=ums only.
# uEnv.txt naming no slot (lost or torn by a power cut; FAT has no journal): uEnv.bak, tfctl's
# copy of the state before its last write. Neither file: slot a, tries 0.
if test "${slot}" != "a" -a "${slot}" != "b"; then
	setenv slot
	setenv tries
	if load mmc 0:1 ${ramdisk_addr_r} /uEnv.bak; then
		env import -t -r ${ramdisk_addr_r} ${filesize} slot tries pf_mode
		echo "thinframe: uEnv.txt names no slot, using uEnv.bak"
	fi
fi
if test "${slot}" != "b"; then setenv slot a; fi
if test -z "${tries}"; then setenv tries 0; fi
if test ${tries} -gt 0; then
	setexpr tries ${tries} - 1
	if test ${tries} -eq 0; then
		if test "${slot}" = "a"; then setenv slot b; else setenv slot a; fi
	fi
	env export -t ${ramdisk_addr_r} slot tries pf_mode
	fatwrite mmc 0:1 ${ramdisk_addr_r} /uEnv.txt ${filesize}
fi
setenv pf_extlinux /extlinux/slot-${slot}.conf
echo "thinframe: slot ${slot}, tries left ${tries}"

# Did Linux confirm the last start? It copies pf_boots into pf_ok.
setenv pf_prev_ok no
if test "${pf_ok}" = "${pf_boots}"; then setenv pf_prev_ok yes; fi

setenv pf_next linux
if test "${pf_mode}" = "ums"; then setenv pf_next ums; fi
if test "${pf_mode}" = "auto" -a "${pf_last}" = "linux" -a "${pf_prev_ok}" = "no"; then
	setenv pf_next ums
fi

setexpr pf_boots ${pf_boots} + 1
setenv pf_last ${pf_next}
env export -t ${ramdisk_addr_r} pf_script pf_boots pf_last pf_prev_ok pf_mode
fatwrite mmc 0:1 ${ramdisk_addr_r} /pf/uboot.env ${filesize}
echo "pf1007l: start ${pf_boots} (hex), last start confirmed: ${pf_prev_ok}, now: ${pf_next}"

if test "${pf_next}" = "linux"; then
	sysboot mmc 0:1 any ${pxefile_addr_r} ${pf_extlinux}
	# Only reached when no label could load its files.
	setenv pf_last loadfail
	env export -t ${ramdisk_addr_r} pf_script pf_boots pf_last pf_prev_ok pf_mode
	fatwrite mmc 0:1 ${ramdisk_addr_r} /pf/uboot.env ${filesize}
fi

# USB disk mode: the Mac sees the SD card as a read-only USB disk ("USB download gadget").
# This script ends here; the built-in boot command (pf1007l_defconfig) then runs: ums 0 mmc 0
echo "pf1007l: USB disk mode"
