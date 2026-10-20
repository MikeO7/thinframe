# thinframe

A thin client for cheap Android photo frames. The frame (PF1007L, Allwinner A33, armv7, 1 GB)
runs a Buildroot-built Linux and one browser process; all the brains live in `frameserver`, a Go
binary in a Docker container on hardware with real horsepower. The frame's CPU, GPU and
RAM go to rendering the dashboard and nothing else.
The frame boots from microSD, joins Wi-Fi, accepts SSH, and shows one HTML page
full screen with working touch. The page, its data and the per-frame settings
come from the container (`server/`). The container also carries the frame's entire
operating system as an A/B update bundle: a weekly Buildroot rebuild on its LTS branch
keeps every frame patched with automatic rollback. The card is written once.
The container speaks MCP, so Claude can manage frames in plain language. The display stack is a single
process: `cog` (WPE WebKit) drawing directly on DRM/KMS. No X, no Wayland
compositor.

Status: **built and tested off the frame, not yet on it.** The server, `tfctl` and the
Buildroot card image are done. The image boots in QEMU (`-M orangepi-pc`: provisioning, A/B
update and rollback), and the sim container renders the dashboard. Nothing has run on the
PF1007L panel yet. `TASKS.md` lists what is left (sections 4 to 8 and 13 need the frame),
with a gate after each step. `docs/log.md` has the evidence for every checked gate.
`CLAUDE.md` has the rules for whoever (or whatever) finishes it.

## Why this exists

[pangoleen/ormarch-picture-frame](https://github.com/pangoleen/ormarch-picture-frame)
already runs a full Omarchy/Hyprland desktop on this frame. It proved the boot
chain, panel driver, touch, and Wi-Fi. thinframe reuses those proven, pinned
artifacts and swaps the Arch desktop for a ~150 MB Buildroot root whose only job
is to render a web page. Memory and CPU go to the content.

The OS is built with Buildroot: nothing is installed that is not in our defconfig, and every
version (WPE, cog, kernel, mesa) is pinned in this repo and bumped on purpose.

## Hardware (tested unit, from the upstream project)

| Part | Value |
|---|---|
| Board | EFERCRO PF1007L_MB_V1.0 (Humblestead HM-PF1007-BK, AiMOR firmware) |
| SoC / RAM | Allwinner A33, 4x Cortex-A7, armv7 32-bit, 1 GB |
| Panel | MIPI-DSI 800x1280 native (portrait). Jadard JD9365DA class. Frame is used landscape: rotate 270 (Hyprland `transform 3`). |
| Touch | Silead GSL, I2C0 0x40, needs proprietary `gsl_m86_hd.fw` (not in Git) |
| Wi-Fi | AIC8800DC on SDIO, 2.4 GHz only, out-of-tree driver |
| GPU | Mali-400 MP2, Mesa lima, GLES 2 only |
| USB | One USB-C, device mode (powers the frame). Gadget serial + CDC ECM network for recovery |
| Storage | 16 GB eMMC. **Never written.** We boot from SD only. |

## Architecture

```
 server (Docker, any box)                      frame (Buildroot, 1 GB A33)
 ┌──────────────────────────┐    Wi-Fi LAN    ┌──────────────────────────┐
 │ frameserver (Go)         │ <── GET /frame/id ── cog (WPE WebKit on DRM)│
 │  static/ dashboard       │ ── HTML/JS/data ──>│                          │
 │  /api/frames/<id>/ws     │ <══ WebSocket ════> tfctl agent (Rust)      │
 │   push config + actions  │     heartbeats     │  (restart cog, reboot,   │
 │  /admin                  │                    │   rotation, update)      │
 │  /data volume (json)     │                    │ BusyBox init dropbear iwd│
 └──────────────────────────┘                    └──────────────────────────┘
```

On the card:
```
microSD (MBR)
  8 KiB      u-boot-pf1007l-L.bin         (reused, pinned, from ormarch)
  4 MiB      p1 FAT32 PFBOOT 256 MiB      zImage.a/.b, DTBs, extlinux/slot-{a,b}.conf, boot.scr, uEnv.txt
  260 MiB    p2 ext4 rootA 1 GiB          Buildroot rootfs, read-only (slot a)
             p3 ext4 rootB 1 GiB          slot b, written by A/B updates
             p4 ext4 data, rest of card   thinframe.conf, ssh keys, iwd profiles

OS      : Buildroot (musl, BusyBox init), WPE WebKit 2.50 + cog 0.18.4 on DRM, mesa lima
Kernel  : mainline 7.2.x + ormarch panel patch/DTS + aic8800 module package
Init    : BusyBox init
Net     : iwd (Wi-Fi) + dhcpcd, USB-C gadget (CDC ECM 192.168.7.2) as fallback
Control : one outbound WebSocket from tfctl; server pushes, frame acks. No inbound ports. SSH for humans only.
Access  : dropbear, key only, user `frame`
Display : cog --platform=drm, launched and supervised by tfctl
Content : SERVER_URL/frame/FRAME_ID from frameserver; local www/ only as offline fallback
Tooling : `tfctl`, one static Rust binary (musl, <1 MB): kiosk supervisor, update, wifi, status
Update  : `tfctl update` = fetch main tarball (no git on the frame) + apply overlay + restart
```

## Containers

| Container | Role | Runs where |
|---|---|---|
| `frameserver` | the brains: dashboard, per-frame config, heartbeats, artifacts, `/admin`, `/mcp` | your server, always |
| `simframe` | a fake frame: headless WPE + agent (fast) or the real SD image in QEMU (full OS) | dev machine, `--profile sim` |

```sh
docker compose up -d                       # production: frameserver only
docker compose --profile sim up --build    # dev: frameserver + simframe, screenshots in sim/out/
```

## Budget

The frame's hardware is for the dashboard, not the OS. Hard limits, checked in TASKS.md:

| Thing | Limit |
|---|---|
| Root filesystem | under 200 MB |
| RAM used before cog starts | under 60 MB |
| RAM used with the page up | under 300 MB |
| Processes besides cog, BusyBox init + getty, dropbear, iwd + dbus-daemon, dhcpcd, udevd, udhcpd (USB link) | none |
| Custom code on the frame | one Rust binary, `tfctl`, under 1 MB, no shell helpers. Container is Go. |
| Boot to page | under 20 s |
| Touch to visible response | under 50 ms (one tap to a highlighted button) |
| Scroll / transition frame rate | 60 fps, no dropped frames in a 10 s swipe test |
| CPU idle with the page up | under 5 % |
| Processes awake on touch | cog's WebProcess only. tfctl sleeps, no cron, no logging daemon |

## Quick start (once TASKS.md is done)

1. Server: `docker compose up -d` on any box with Docker.
2. Card: `curl -fsSL https://raw.githubusercontent.com/MikeO7/thinframe/main/scripts/flash.sh | bash`
   and answer four questions (Wi-Fi, password, server, frame name).
3. Card in the frame, power on, wait a minute. It appears in `/admin`.

Full walkthrough and troubleshooting: [docs/flashing.md](docs/flashing.md).

Change the dashboard: edit the frame's layout in `/admin` (or via MCP); widget templates live in `server/internal/widgets/`. The frame is never touched.

## Layout

| Path | What |
|---|---|
| `TASKS.md` | Ordered work list with gates. Start here. |
| `CLAUDE.md` | Rules for the agent finishing this |
| `docs/decisions.md` | Why Buildroot, why cog on DRM, why reuse the ormarch kernel work |
| `docs/hardware.md` | Facts copied from the upstream project, with confidence tags |
| `docs/flashing.md` | The three-step user guide for a new frame |
| `docs/update-and-ssh.md` | How to get in and how to change things on a running frame |
| `image/build.sh` | Builds the SD image in Docker (Buildroot external tree in `image/buildroot/`) |
| `image/boot/` | extlinux.conf and uEnv.txt templates for the FAT partition |
| `image/rootfs/` | Root overlay applied by Buildroot (`BR2_ROOTFS_OVERLAY`) |
| `www/` | The page the frame shows |
| `tfctl/` | Rust: the only custom code on the frame |
| `server/` | Go: the brains. Dockerfile builds a scratch image |
| `sim/` | Simulated frame container: headless WPE + agent, or QEMU of the real image |
| `scripts/` | `flash.sh` one-command flasher, `deploy.sh` dev push |

License: MIT.
