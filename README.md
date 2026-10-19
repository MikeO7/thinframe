# thinframe

A thin client for cheap Android photo frames. The frame (PF1007L, Allwinner A33, armv7, 1 GB)
runs Alpine Linux and one browser process; all the brains live in `frameserver`, a Rust
binary in a Docker container on hardware with real horsepower. The frame's CPU, GPU and
RAM go to rendering the dashboard and nothing else.
The frame boots from microSD, joins Wi-Fi, accepts SSH, and shows one HTML page
full screen with working touch. The page, its data and the per-frame settings
come from the container (`server/`). The container also carries the frame's own
software updates: rebuild it and every frame updates itself. The card is written once. The display stack is a single
process: `cog` (WPE WebKit) drawing directly on DRM/KMS. No X, no Wayland
compositor.

Status: **stub.** The design is fixed and every file has its job written at the
top. Nothing here has booted yet. `TASKS.md` is the ordered list of what is
left, with a gate after each step. `CLAUDE.md` has the rules for whoever (or
whatever) finishes it.

## Why this exists

[pangoleen/ormarch-picture-frame](https://github.com/pangoleen/ormarch-picture-frame)
already runs a full Omarchy/Hyprland desktop on this frame. It proved the boot
chain, panel driver, touch, and Wi-Fi. thinframe reuses those proven, pinned
artifacts and swaps the Arch desktop for a ~150 MB Alpine root whose only job
is to render a web page. Memory and CPU go to the content.

## Hardware (tested unit, from the upstream project)

| Part | Value |
|---|---|
| Board | EFERCRO PF1007L_MB_V1.0 (Humblestead HM-PF1007-BK, AiMOR firmware) |
| SoC / RAM | Allwinner A33, 4x Cortex-A7, armv7 32-bit, 1 GB |
| Panel | MIPI-DSI 800x1280 native (portrait). Jadard JD9365DA class. Frame is used landscape: rotate 270 (Hyprland `transform 3`). |
| Touch | Silead GSL, I2C0 0x40, needs proprietary `gsl_m86_hd.fw` (not in Git) |
| Wi-Fi | AIC8800DC on SDIO, 2.4 GHz only, out-of-tree driver |
| GPU | Mali-400 MP2, Mesa lima, GLES 2 only |
| USB | One USB-C, device mode (powers the frame). Gadget serial + NCM network for recovery |
| Storage | 16 GB eMMC. **Never written.** We boot from SD only. |

## Architecture

```
 server (Docker, any box)                      frame (Alpine, 1 GB A33)
 ┌──────────────────────────┐    Wi-Fi LAN    ┌──────────────────────────┐
 │ frameserver (Rust)       │ <── GET /frame/id ── cog (WPE WebKit on DRM)│
 │  static/ dashboard       │ ── HTML/JS/data ──>│                          │
 │  /api/frames/<id>/config │ <── poll 15 s ───── tfctl agent (Rust)      │
 │  /api/frames/<id>/heartbeat <── status ───────│  (restart cog, reboot,   │
 │  /admin                  │                    │   rotation, update)      │
 │  /data volume (json)     │                    │ OpenRC sshd iwd dhcpcd   │
 └──────────────────────────┘                    └──────────────────────────┘
```

On the card:
```
microSD (MBR)
  8 KiB      u-boot-pf1007l-L.bin         (reused, pinned, from ormarch)
  4 MiB      p1 FAT32 PFBOOT              zImage, DTB, initramfs, extlinux.conf, boot.scr
  260 MiB    p2 ext4 afroot               Alpine rootfs (grows on first boot)

Kernel  : linux-armv7 7.2.10 (Arch Linux ARM package) + panel module + DTB
          + aic8800 modules, all built by the ormarch scripts. See docs/decisions.md.
Init    : OpenRC
Net     : iwd (Wi-Fi) + dhcpcd, usb gadget NCM as fallback
Access  : sshd, key only, user `frame`, passwordless sudo
Display : seatd + cog --platform=drm  (user `frame`, group video/input)
Content : SERVER_URL/frame/FRAME_ID from frameserver; local www/ only as offline fallback
Tooling : `tfctl`, one static Rust binary (musl, <1 MB): kiosk supervisor, update, wifi, status
Update  : `tfctl update` = fetch main tarball (no git on the frame) + apply overlay + restart
```

## Budget

The frame's hardware is for the dashboard, not the OS. Hard limits, checked in TASKS.md:

| Thing | Limit |
|---|---|
| Root filesystem | under 200 MB |
| RAM used before cog starts | under 60 MB |
| RAM used with the page up | under 300 MB |
| Processes besides cog, OpenRC, sshd, iwd, dhcpcd, udev, seatd | none |
| Custom code | one Rust binary, `tfctl`, under 1 MB, no shell helpers |
| Boot to page | under 20 s |

## Quick start (once TASKS.md is done)

```sh
export PF1007L_TOUCH_FIRMWARE=/path/to/gsl_m86_hd.fw
image/build.sh                 # makes image/out/thinframe-sd.img
scripts/write-card-macos.sh image/out/thinframe-sd.img
# insert card, power on, wait ~40 s
ssh frame@thinframe.local    # or frame@192.168.7.2 over the USB cable
```

Change the dashboard: edit `server/static/`, `docker compose up -d --build`. The frame is never touched.

## Layout

| Path | What |
|---|---|
| `TASKS.md` | Ordered work list with gates. Start here. |
| `CLAUDE.md` | Rules for the agent finishing this |
| `docs/decisions.md` | Why Alpine, why cog on DRM, why reuse the ormarch kernel |
| `docs/hardware.md` | Facts copied from the upstream project, with confidence tags |
| `docs/update-and-ssh.md` | How to get in and how to change things on a running frame |
| `image/build.sh` | Builds the SD image on a Mac with Docker |
| `image/boot/` | extlinux.conf and uEnv.txt templates for the FAT partition |
| `image/rootfs/` | Overlay copied over the Alpine minirootfs |
| `www/` | The page the frame shows |
| `tfctl/` | Rust: the only custom code on the frame |
| `server/` | The brains (Go recommended, see decisions.md) + Dockerfile + compose |
| `sim/` | Simulated frame: headless WPE + agent in a container |
| `scripts/` | Card writer wrapper, deploy over SSH |

License: MIT.
