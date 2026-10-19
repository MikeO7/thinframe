# Decisions

## Alpine, not Arch Linux ARM
Alpine armv7 minirootfs is ~3 MB; the finished root should be under 200 MB. musl + busybox
+ OpenRC start in a few seconds and idle at ~30 MB. Arch ARM gave us systemd and a desktop we
do not need. Alpine is still a real package manager, so `cog`, `iwd`, `openssh` are one
`apk add` away.

## cog on DRM, not a Wayland compositor, not X
The frame shows exactly one web page. A compositor only adds a process, a buffer copy and
~40 MB. `cog --platform=drm` (WPE WebKit, `wpebackend-fdo`) opens the DRM device itself,
reads touch via libinput and renders with GLES 2 on lima. One process, direct scanout.

| Option | Processes | RAM idle (est., armv7) | Notes |
|---|---|---|---|
| **cog drm** | cog + WPEWebProcess + WPENetworkProcess | ~150-250 MB | Chosen. [U] until measured |
| cage + cog wayland | + compositor | +40 MB | Fallback if drm platform fails on lima |
| cage + chromium | + 5-10 chromium processes | 400-600 MB | Too heavy for 1 GB, and Chromium armv7 builds are getting rare |
| X + surf/midori | + Xorg | ~250 MB | No advantage, older stacks |

Rotation: the panel is portrait. cog's drm platform takes a `rotation` platform param
(`COG_PLATFORM_DRM_ROTATION` env or `--platform-params=rotation=N`, N in quarter turns).
Verify the exact spelling against the installed cog version. [U]

If `apk info -L cog` has no drm platform on armv7, the fallback is `cage` + `cog --platform=wl`
and the service file already has the lines to swap in.

## Reuse the ormarch kernel
Mainline needs two things this board does not have upstream: the JD9365DA panel init table and
the AIC8800 SDIO driver. ormarch already builds both as modules against Arch's `linux-armv7`
7.2.10 and has a DTB that booted. A kernel is distro-neutral: we unpack the Arch package for
`/boot/zImage` and `/lib/modules/...` into the Alpine root. Later, if wanted, build the same
sources with Alpine's `abuild` and drop the Arch dependency (not a task now).

## OpenRC services instead of systemd units
Alpine default. `thinframe` is a plain `supervise-daemon` service so a crashed cog restarts.

## SSH key only, user `frame`, `doas` not `sudo`
Smaller, Alpine default. Host key generated once into `image/secrets/` so re-flashing a card
does not change the fingerprint.

## Content as a local file by default
`file:///var/lib/thinframe/www/index.html`. No web server needed on the frame. Set
`FRAME_URL` in `/etc/conf.d/thinframe` to point at anything else (Home Assistant, a
dashboard). The page itself can fetch whatever it wants.

## One Rust binary for everything custom
Shell scripts were the first draft. They pull in coreutils behaviour, fork a dozen processes
per invocation and are hard to test. `tfctl` is a single static musl binary (opt-level z, LTO,
panic=abort, stripped; target under 1 MB) with hand-rolled arg parsing and no crates unless a
task proves one necessary. It is the service entry point (spawns and supervises cog), the
updater, the Wi-Fi helper and the status tool. The rest of the system is stock Alpine: busybox,
OpenRC, eudev, seatd, iwd, dhcpcd, sshd.

## Nothing extra in the image
No avahi (dbus + daemon for a name the router already knows), no chrony (busybox ntpd), no git
(tfctl downloads a tarball), no htop/curl/rsync in the release image. Debug tools are in
`image/packages-debug.txt` and only go into a DEBUG build.

## Where dashboard performance actually comes from
`tfctl` is not on the hot path: it launches cog and sleeps. Frame rate and responsiveness are
decided by WPE WebKit (C++, GLES on lima) and by the page. Rules for `www/`:
- No framework. Plain HTML/CSS/JS, one file if possible.
- Animate only `transform` and `opacity` (GPU composited). No layout thrash, no box-shadow animation.
- Few DOM nodes. Update text, do not rebuild trees.
- Images pre-sized to the panel (800x1280 or 1280x800). No client-side scaling of 4K photos.
- Fetch data on a timer, render diffs. Keep JS heap small; the WebProcess has ~200 MB to live in.

If HTML ever becomes the bottleneck, the escape hatch is a Rust renderer drawing straight to
DRM/GBM with GLES (no browser at all). That is a different project; note it, do not start it.

## Thin client: brains in a container, frame is a display
The frame stays exactly what it was (Alpine + cog on DRM). What changes is where the page and
the decisions live: `frameserver`, a Go binary in a scratch Docker image, on whatever box has
spare CPU. The frame's `tfctl agent` polls it for config and posts heartbeats. Consequences:

- Dashboard changes are a container rebuild. The SD card is written once.
- Heavy work (image resizing, API calls, caching, scheduling) runs on the server. The frame's 1 GB
  holds only WebKit and the rendered page.
- Many frames, one server, per-frame config by id.
- Offline: the frame falls back to a local page; the dashboard should also cache itself.

Rejected: streaming pixels (VNC/RDP/WebRTC). Bandwidth and latency are worse than sending HTML
and letting the frame's GPU composite it, and it needs a heavier client than cog.

Transport is plain HTTP polling on the LAN. WebSocket push is a later refinement, not needed
at a 15 s poll.

## Languages: Rust on the frame, Go in the container (decided 2026-10-18)
Rust on the frame is for the 1 MB static binary and zero-allocation idle. The container has no
size pressure, so Go: stdlib HTTP server + `embed` for static files, `CGO_ENABLED=0` static
builds for amd64 and arm64 in one command, fast compiles, official MCP SDK. No other languages
anywhere: no Python helpers, no Node in the dashboard build. The page is hand-written HTML/CSS/JS.

## Updates flow from the container
Three layers, three mechanisms:

| Layer | How it updates | Who does it |
|---|---|---|
| Dashboard (HTML/JS/data) | Served live. `page_version` in config; agent reloads cog when it changes | server |
| Frame software (`tfctl`, `image/rootfs/` overlay) | Container image bundles `tfctl-armv7` and `overlay.tar.gz` with a version + sha256. Config poll says `software_version`; agent downloads from `/api/artifacts/<name>`, verifies, installs, restarts. Overlay excludes `thinframe.conf`, ssh and iwd | agent |
| Kernel, DTB, U-Boot | Card re-write. Rare. Optional later: A/B root partitions and `extlinux.conf` switching | human |

So "rebuild the container" is the only deploy action. Every frame converges within one poll.
Safety: agent keeps the previous `tfctl` as `tfctl.prev`; if the new one fails to start twice,
supervise-daemon runs the old one (TASK 11).

## Simulator
See `sim/README.md`. Tier 1 (headless WPE + native agent in a container) is the one worth
building: same engine as the frame, full control loop, no hardware.

## Full OS updates: A/B root slots, driven by the container
"Patched" means the kernel and every Alpine package, not only tfctl. The frame never runs `apk`:
the image is immutable and replaced whole. Card layout grows from 2 to 4 partitions:

| Part | Content | Why |
|---|---|---|
| p1 FAT32 `PFBOOT` | U-Boot script, `extlinux.conf`, `uEnv.txt`, `zImage.a` `zImage.b`, DTBs | U-Boot picks the slot from `uEnv.txt` (`slot=a|b`, `tries=N`) |
| p2 ext4 `rootA` | Alpine root, read-only mount | active or standby |
| p3 ext4 `rootB` | Alpine root, read-only mount | standby or active |
| p4 ext4 `data` | `/etc/thinframe.conf`, ssh host keys, iwd profiles, logs, www fallback | survives updates; bind-mounted over the read-only root |

Update flow: CI builds `system-<ver>.tar` = {zImage, modules, rootfs.ext4.zst, sha256}. The container
ships it under `/api/artifacts/`. The agent sees `system_version` change, streams the rootfs into the
standby slot, copies the kernel to `zImage.<slot>`, sets `slot=<new> tries=3` in `uEnv.txt`, reboots.
U-Boot's `boot.scr` (already has a counter in the ormarch build) decrements `tries`; after three failed
boots it falls back to the old slot. On success the agent's first heartbeat sets `tries=0`, marking the
slot good. The server shows each frame's slot, version and last-good in `/admin`.

Why not `apk upgrade` in place: no atomicity, no rollback, drift between frames, and a half-applied
upgrade on a 1 GB device with a flaky Wi-Fi link is the common case, not the edge case.

## Staying patched
- A scheduled GitHub Action rebuilds the image weekly against the current Alpine stable branch and
  the pinned kernel, runs the QEMU boot test, and publishes `system-<date>.tar` with the container
  image. Frames pick it up on their next poll. Alpine's security fixes reach the frame within a week.
- Kernel: pinned to the ormarch-built 7.2.x line until TASK K1 (build the same patches on Alpine's
  `linux-lts` via `abuild`) lands; then it tracks Alpine's kernel updates for free.
- Dependabot/Renovate on `server/` and `tfctl/` dependencies.
- Baseline on the frame: root fs read-only; no password login anywhere; sshd key-only, `frame` user
  only; `doas` for exactly the commands tfctl needs; iwd with WPA2/3 only; no inbound ports besides
  22; all frame→server traffic over the LAN with a shared bearer token (`FRAME_TOKEN`); artifacts
  verified by sha256 from the signed-in config response before install. Outbound HTTPS from the
  frame is unnecessary: the container fetches the world, the frame only talks to the container.
- Secrets never in the image: host key, token and Wi-Fi PSK live on `data` and are provisioned once.

## Two containers
`frameserver` is production: one binary, one volume, no build tools. `simframe` is development
only: it depends on `frameserver` and pretends to be a frame. They never share an image.
Root `compose.yml` runs the first by default and both with `--profile sim`.
`simframe` has two modes, picked by `SIM_MODE`:

| Mode | What runs | Speed | Tests |
|---|---|---|---|
| `wpe` (default) | headless WPE WebKit + native tfctl agent, screenshots to `sim/out/` | seconds | dashboard rendering, config loop, heartbeats, tfctl updates |
| `qemu` | the real SD image under `qemu-system-arm` (orangepi-pc board, serial console, user-mode net) | ~1 min boot | OpenRC, networking, A/B system update and rollback |

Neither mode has the DSI panel or the touch controller; those two gates stay on hardware.

## MCP in the backend
`frameserver` exposes an MCP server (streamable HTTP at `/mcp`, bearer token) so Claude Code or
Claude Desktop can drive it in natural language: "rotate the kitchen frame", "put the weather
widget top-left on all frames", "update every frame to the latest system image", "show me what
the sim frame looks like". Go: `github.com/modelcontextprotocol/go-sdk`. Tools are thin wrappers
over the same handlers the REST API and `/admin` use, so there is one code path.

Initial tool list (TASK S7):

| Tool | Does |
|---|---|
| `list_frames` | ids, ip, last heartbeat, versions, slot, temp, mem |
| `get_frame` / `set_frame_config` | read / change url, rotation, brightness, page_version, tags |
| `frame_action` | reboot, restart-kiosk, update-software, update-system, rollback |
| `get_screenshot` | PNG from the sim (and from real frames once TASK 12 adds a capture path) |
| `list_widgets` / `set_layout` | dashboard composition stored per frame as JSON the page reads |
| `get_logs` | last N heartbeats and agent events for a frame |
| `release_status` | what system/software versions the container carries and which frames lag |

Writes that touch many frames (`update_system` with no id) require `confirm: true`.

## Buttery smooth: the frame does nothing but render
Interaction latency is the product. Everything else in this repo exists to keep the frame idle
when a finger is not on it, and fully available when one is.

What the frame runs when the page is up: kernel, OpenRC (asleep), udev (asleep), seatd, sshd
(asleep until a connection), iwd + dhcpcd (asleep), tfctl (one thread, wakes every 15 s for a
~1 KB HTTP poll), cog's UIProcess + WebProcess + NetworkProcess. That is the whole list, and
TASK 9's `ps` audit fails if anything else appears.

Levers, in the order to pull them (TASK 13):
1. **GPU path**: confirm WPE uses lima GLES for compositing (`WEBKIT_SHOW_FPS=1`, check
   `/sys/kernel/debug/dri/0/`). Software rendering is the usual cause of a laggy frame.
2. **cpufreq**: `performance` governor while a touch is down, `ondemand` otherwise, or simply
   `performance` at a fixed 1.008 GHz: the A33's idle draw difference is small and the frame is
   mains powered. Measure both.
3. **Input**: libinput with no gestures, no tap timeouts; `cog` gets raw touch, the page uses
   `pointerdown`, not `click`, and `touch-action: none` where it scrolls itself.
4. **No memory pressure**: no swap, no zram, WebProcess memory cap via
   `WEBKIT_MEMORY_PRESSURE_*` so it GCs before the kernel reclaims.
5. **Page rules** (see "Where dashboard performance comes from"): the server pre-renders anything
   expensive (image resize, charts as SVG, data joins). The page receives finished HTML and small
   JSON. Transitions use `transform`/`opacity` only; `will-change` on the elements that move.
6. **Nothing periodic on the frame** besides tfctl's poll: no cron, no syslog to disk, no ntp
   daemon (one-shot `ntpd -q` at boot and on resume), `dmesg` ring only.
7. **Panel refresh**: run the page at the panel's native 59.7 Hz; no vsync-off tricks.

Measurement, not opinion: TASK 13 adds `tfctl bench`, which replays a touch via `evemu` on the
sim/qemu or a real frame and records touch→first-paint from `WEBKIT_SHOW_FPS` / a page-side
`performance.now()` beacon posted to the server. The numbers go into the heartbeat so `/admin`
shows latency per frame over time.
