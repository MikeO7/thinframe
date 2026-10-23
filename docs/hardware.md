# Hardware facts

All copied from `pangoleen/ormarch-picture-frame` (`docs/hardware.md`, `docs/experiments.md`,
`PLAN.md`) and tagged [D] here until re-measured on this image.

| Item | Value | Tag |
|---|---|---|
| SoC | Allwinner A33 (sun8i), FEL id 00001667 | D |
| Boot | Boot ROM reads SPL from 8 KiB of SD before eMMC. U-Boot v2025.10 + `image/uboot/patches`. Console UART0 unused; `CONFIG_SUNXI_CONSOLE_NO_PINMUX=y` because PL2 is Wi-Fi enable | D |
| Kernel | Arch `linux-armv7 7.2.10-1`, release `7.2.10-1-armv7-ARCH` | D |
| DTB | `sun8i-a33-pf1007l.dtb` (panel + touch + Wi-Fi + PMIC), from `image/buildroot/board/pf1007l/dts` | V |
| Panel | DSI 4-lane RGB888 800x1280 @ 59.7 Hz, `panel-jadard-jd9365da-h3` with added pf1007l entry | D |
| Rotation | Hyprland `transform 3` gave correct landscape; Android used `ro.sf.rotation 90` | D |
| Touch | Silead, I2C0 0x40, firmware `/lib/firmware/silead/gsl_m86_hd.fw` (sha256 e11ffa8a…). Hyprland needed touch transform 6 (vertical flip) on top of output transform 3 | D |
| Wi-Fi | AIC8800DC SDIO on MMC1. Driver radxa-pkg/aic8800 e9e43ed7, 17 of 29 Debian patches. Firmware `/lib/firmware/aic8800_fw/SDIO/aic8800DC/`. 2.4 GHz only | D |
| Enable pins | Wi-Fi: PL2 then PL6 after 5 ms, and AXP GPIO0 (in DTB) | D |
| PMIC | AXP223 on RSB. CPU supply DCDC3 | D |
| GPU | Mali-400 MP2, lima, GLES 2 | D |
| USB | MUSB OTG, device mode only in practice. Gadget ACM + NCM used for recovery console | D |
| Accelerometer | sc7a20 on I2C1 0x19, `st-accel-i2c` works | D |
| Speaker | A33 codec, amp enable PH09. Not working yet upstream | D |
| Light sensor | I2C1 0x23 LTR-501 class, no response upstream | D |
| eMMC | 16 GB, kept untouched | D |
