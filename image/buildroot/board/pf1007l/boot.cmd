# slot/tries handling, extends ormarch's boot.scr (TASK 3). Compile: mkimage -C none -A arm -T script -d boot.cmd boot.scr
setenv slot a
setenv tries 0
if fatload mmc 0:1 ${scriptaddr} uEnv.txt; then env import -t ${scriptaddr} ${filesize}; fi
if test ${tries} -gt 0; then
	setexpr tries ${tries} - 1
	if test ${tries} -eq 0; then
		if test ${slot} = a; then setenv slot b; else setenv slot a; fi
	fi
	env export -t ${scriptaddr} slot tries
	fatwrite mmc 0:1 ${scriptaddr} uEnv.txt ${filesize}
fi
if test ${slot} = a; then setenv part 2; else setenv part 3; fi
setenv bootargs "root=PARTUUID=50463037-0${part} rootfstype=ext4 ro rootwait console=tty1 quiet loglevel=3"
echo "thinframe: booting slot ${slot} tries ${tries}"
