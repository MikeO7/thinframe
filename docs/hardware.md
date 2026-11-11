# Hardware facts

All copied from `pangoleen/ormarch-picture-frame` (`docs/hardware.md`, `docs/experiments.md`,
`PLAN.md`) and tagged [D] here until re-measured on this image. A row tagged V names the date of
the measurement on this frame in `docs/log.md`; the rest of such a row is still [D].

| Item | Value | Tag |
|---|---|---|
| SoC | Allwinner A33 (sun8i), FEL id 00001667 | D |
| Boot | Boot ROM reads SPL from 8 KiB of SD before eMMC (card wins over an installed eMMC: V 2026-10-27). U-Boot v2025.10 + Wes Bos patch + "build L" patch. Console UART0 unused; `CONFIG_SUNXI_CONSOLE_NO_PINMUX=y` because PL2 is Wi-Fi enable | D |
| Kernel | Mainline 7.2.10 built by Buildroot with the ormarch panel patch and DTS (`image/build.sh` KVER); on the frame: 7.2.10, panel bound on DSI-1. Upstream ran the Arch package `linux-armv7 7.2.10-1` [D] | V 2026-10-29 |
| DTB | `sun8i-a33-pf1007l-full.dtb` (panel + touch + Wi-Fi + BT UART). `-min` and `-pmic` are debug steps | D |
| Panel | DSI 4-lane RGB888 800x1280 @ 59.7 Hz, `panel-jadard-jd9365da-h3` with added pf1007l entry | D |
| Rotation | cog `rotation=3` is upright landscape (`tfctl screenshot`: 1280x800 from card1, rotation 3). Upstream: Hyprland `transform 3`; Android used `ro.sf.rotation 90` [D] | V 2026-10-26 |
| Touch | Silead, I2C0 0x40, firmware `/lib/firmware/silead/gsl_m86_hd.fw` (sha256 e11ffa8a…, chip ID 0x50910000: V 2026-10-22). With cog rotation 3 the libinput matrix is the swap `0 1 0 1 0 0` (`90-touch.rules`), targets within 21 px. Upstream: Hyprland touch transform 6 on top of output transform 3 [D] | V 2026-10-26 |
| Wi-Fi | AIC8800DC SDIO on MMC1. Driver radxa-pkg/aic8800 e9e43ed7, 17 of 29 Debian patches. Firmware `/lib/firmware/aic8800_fw/SDIO/aic8800DC/`. 2.4 GHz only | D |
| Enable pins | Wi-Fi: PL2 then PL6 after 5 ms, and AXP GPIO0 (in DTB) | D |
| PMIC | AXP223 on RSB. CPU supply DCDC3 | D |
| GPU | Mali-400 MP2, lima, GLES 2 | D |
| USB | MUSB OTG, device mode only in practice. Our kernel: built-in CDC composite gadget, ACM (`ttyGS0`) + ECM (`usb0`), seen on the Mac as 0525:a4aa with ping. Upstream used ACM + NCM [D] | V 2026-10-29 |
| Accelerometer | sc7a20 on I2C1 0x19, `st-accel-i2c` works | D |
| Speaker | A33 codec, amp enable PH09. Not working yet upstream | D |
| Light sensor | Analog, behind the hole at the bottom left of the bezel, on LRADC0 (0x01c22800). Vendor mode ctrl `0x00c00041`, 6-bit `LRADC_DATA0` (0x01c2280c): 60..62 lit, 0..2 covered (ormarch E063/E064). The fex `[ls_para]` I2C 0x23 entry is a template leftover: nothing answers there. Mainline has no driver; `tfctl light` reads it through /dev/mem: 62 in a lit room, 0 in the dark | V 2026-10-26 |
| eMMC | 16 GB (15,758,000,128 bytes, `/dev/mmcblk2` on our kernel). Written only through tfctl (decisions.md "Running from the eMMC") | V 2026-10-26 |
