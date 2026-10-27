# thinframe

A thin client for cheap Android photo frames. The frame (PF1007L, Allwinner A33, armv7, 1 GB)
runs a Buildroot-built Linux and one browser process; all the brains live in `frameserver`, a Go
binary in a Docker container on hardware with real horsepower. The frame's CPU, GPU and
RAM go to rendering the dashboard and nothing else.
The frame boots from microSD, joins Wi-Fi, accepts SSH, and shows one HTML page
full screen with working touch. The page, its data and the per-frame settings
come from the container (`server/`). The container also carries the frame's entire
operating system as an A/B update bundle, built from the pinned Buildroot tree and installed
with automatic rollback. The card is written once.
The container speaks MCP, so Claude can manage frames in plain language. The display stack is a single
process: `cog` (WPE WebKit) drawing directly on DRM/KMS. No X, no Wayland
compositor.

Status: **running on a PF1007L every day.** One frame (the office) boots the Buildroot card,
shows the dashboard from a `frameserver` container and takes software and system updates from
it. The image also boots in QEMU (`-M orangepi-pc`: provisioning, A/B update and rollback), and
the sim container renders the dashboard in CI. `TASKS.md` still has open hardware gates that
were never written up with evidence; `docs/log.md` has the evidence for every checked one.
`CLAUDE.md` has the rules for working on it.

## What the frame shows

A dashboard with a nav rail: **Home** (clock, weather, today's calendar, cars, house summary),
**Cars** (Tesla status and controls), **House** (rooms, climate, doors, lights from Home
Assistant), **Cameras** (UniFi Protect snapshots and doorbell), **Shows** (new episodes) and
**System** (the server's containers). The layout is per frame and edited over MCP or `/admin`.
After 90 s without a touch the frame goes back to Home; a dark room turns the screen off and a
touch or a wave in front of the light sensor wakes it. Calendars come from ICS links or, for a
private iCloud calendar, CalDAV with an app-specific password. Anything else is an app: HTML
that Claude writes and installs with the MCP `put_app` tool (the Cars page is one; its source
is in `apps/`). Buttons call server-side hooks (Home Assistant, Tesla), never the frame.

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
Tooling : `tfctl`, one static Rust binary (musl, ~1.1 MB, CI fails above 2 MB): kiosk, agent, update, wifi, light sensor, splash
Update  : software = new `tfctl` from the server, rolled back after two fast crashes;
          system = whole root image written to the other A/B slot, rolled back if it does not boot
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

The frame's hardware is for the dashboard, not the OS. These are targets we set ourselves, not
hardware limits (the card gives each root slot 1 GB and the A33 has 1 GB of RAM): measured in
TASKS.md, and the only one CI enforces is the tfctl size.

| Thing | Target |
|---|---|
| Root filesystem | under 200 MB (the slot is 1 GB) |
| RAM used before cog starts | under 60 MB |
| RAM used with the page up | under 300 MB |
| Processes besides cog, BusyBox init + getty, dropbear, iwd + dbus-daemon, dhcpcd, udevd, udhcpd (USB link) | none |
| Custom code on the frame | one Rust binary, `tfctl`, under 2 MB (CI fails above), no shell helpers. Container is Go. |
| Boot to page | under 20 s |
| Touch to visible response | under 50 ms (one tap to a highlighted button) |
| Scroll / transition frame rate | 60 fps, no dropped frames in a 10 s swipe test |
| CPU idle with the page up | under 5 % |
| Processes awake on touch | cog's WebProcess only. tfctl sleeps, no cron, no logging daemon |

## Quick start

1. Server: `docker compose up -d` on any box with Docker.
2. Card: `curl -fsSL https://raw.githubusercontent.com/MikeO7/thinframe/main/scripts/flash.sh | bash`
   and answer four questions (Wi-Fi, password, server, frame name).
3. Card in the frame, power on, wait a minute. It appears in `/admin`.

Full walkthrough and troubleshooting: [docs/flashing.md](docs/flashing.md). Setting up another
frame from start to finish, with the server on a Mac: [docs/new-frame.md](docs/new-frame.md).

Change the dashboard: edit the frame's layout in `/admin` (or via MCP); widget templates live in `server/internal/widgets/`. The frame is never touched.

## Layout

| Path | What |
|---|---|
| `TASKS.md` | Ordered work list with gates. Start here. |
| `CLAUDE.md` | Rules for the agent finishing this |
| `docs/decisions.md` | Why Buildroot, why cog on DRM, why reuse the ormarch kernel work |
| `docs/hardware.md` | Facts copied from the upstream project, with confidence tags |
| `docs/flashing.md` | The three-step user guide for a new frame |
| `docs/new-frame.md` | Owner's runbook: server on a Mac (`scripts/server-mac.sh`), touch firmware, each new frame |
| `docs/update-and-ssh.md` | How to get in and how to change things on a running frame |
| `image/build.sh` | Builds the SD image in Docker (Buildroot external tree in `image/buildroot/`) |
| `image/boot/` | extlinux.conf and uEnv.txt templates for the FAT partition |
| `image/rootfs/` | Root overlay applied by Buildroot (`BR2_ROOTFS_OVERLAY`) |
| `www/` | Offline fallback page on the card (the dashboard itself comes from the server) |
| `apps/` | Source of the MCP apps installed on the server (`put_app`), e.g. the Cars page |
| `tfctl/` | Rust: the only custom code on the frame |
| `server/` | Go: the brains. Dockerfile builds a scratch image |
| `sim/` | Simulated frame container: headless WPE + agent, or QEMU of the real image |
| `scripts/` | `flash.sh` one-command flasher, `make-artifacts.sh` stages what the server carries |

License: MIT.
