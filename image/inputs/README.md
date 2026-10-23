# Boot inputs

The board-specific sources the image needs and nothing in Buildroot or mainline provides.
Vendored from [pangoleen/ormarch-picture-frame](https://github.com/pangoleen/ormarch-picture-frame)
at commit `4207b5c2f6d6615bf6a18fb725cc7a77dd532065` (2026-10-12, the tree that booted this frame),
MIT, `LICENSE.ormarch`. Byte-identical to upstream except `uboot/boot.cmd` (see below).

| Path | Used by | What |
|---|---|---|
| `dts/allwinner/sun8i-a33-pf1007l.dts` | Buildroot kernel (`BR2_LINUX_KERNEL_CUSTOM_DTS_DIR`) | the board: panel, touch, Wi-Fi, PMIC |
| `dts/allwinner/sun8i-a33-pf1007l-{min,pmic,full}.dts` | same | the three extlinux labels (`full` is the board plus the touch firmware name) |
| `linux-patches/0001-drm-panel-jadard-...-pf1007l-panel.patch` | Buildroot kernel (`BR2_LINUX_KERNEL_PATCH`) | JD9365DA panel init table for v7.2.10 |
| `uboot/0001-wes-pf1007l-uboot.patch` | `image/uboot-build.sh` | Wes Bos's A33 board support (photo-frame-dashboard 9dd323e7) |
| `uboot/0002-pf1007l-linux-boot.patch` | `image/uboot-build.sh` | "build L": SD only, no eMMC, read-only `ums`, the two defconfigs and the U-Boot DTS |
| `uboot/boot.cmd` | `post-image.sh` (mkimage to `boot.scr`), `qemu-lib.sh` | upstream's boot counter and USB disk fallback, plus the thinframe A/B block (`slot`, `tries` in `uEnv.txt`) |
| `uboot/Dockerfile`, `uboot/build-in-container.sh` | `image/uboot-build.sh` | ours: the cross toolchain and the build with its checks |

U-Boot itself is mainline v2025.10 (`e50b1e87...`), fetched by `image/uboot-build.sh`. Nothing here
is a binary.
