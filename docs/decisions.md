# Decisions

## Buildroot, not Alpine, not Arch Linux ARM (decided 2026-10-18)
The first plan was Alpine. On 2026-10-18 we found that Alpine 3.24 and edge no longer package
`cog` or `wpewebkit` on any architecture (3.21 was the last), so "Alpine + cog" cannot be built
from packages. Buildroot is the canonical way to put WPE on a small ARM board: it cross-compiles
a complete, immutable root from a defconfig we own, with WPE WebKit 2.50 and cog 0.18.5 (DRM
platform) as first-class packages, and lets us pick the kernel, libc (musl) and init (BusyBox).
The result is smaller than Alpine would have been and every version is ours to bump. The cost is
a first WebKit build of 1 to 3 hours (cached afterwards) and a more opinionated tree for the next
person. Debian trixie armhf (cog 0.18.4, WPE 2.48.3, systemd, ~400 MB) was the fallback and is
documented in case Buildroot proves too heavy to maintain.

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

If cog's drm platform ever fails on this board, the fallback is `cage` + `cog --platform=wl`
(`COG_PLATFORM="wl"` in thinframe.conf; tfctl already knows both). Buildroot builds cog with
the drm platform (`BR2_PACKAGE_COG_PLATFORM_DRM`) and it is what the frame runs [V].

## Kernel: Buildroot builds mainline with the ormarch patches
Mainline needs two things this board does not have upstream: the JD9365DA panel init table and
the AIC8800 SDIO driver. ormarch carries the panel patch, the board DTS and the AIC8800 build
recipe, proven on hardware against 7.2.10. Buildroot builds the kernel itself: we point it at a
pinned mainline tag, apply the ormarch panel patch and DTS from `image/buildroot/board/pf1007l/`,
and add `aic8800` as a Buildroot kernel-module package. Phase one pins the same 7.2.x line
ormarch tested; bumping is a one-line change once the frame is verified.

## BusyBox init instead of systemd
Buildroot default. `/etc/inittab` respawns `tfctl kiosk`, which supervises cog itself (a software
update exits tfctl and init starts the new binary). `/etc/init.d/S90thinframe` is one line,
`tfctl boot`: provisioning, growing `/data`, hostname, the USB-C DHCP lease. The root is mounted
read-only; `/data` (p4) holds `etc/thinframe.conf` (`/etc/thinframe.conf` is a symlink to it, so
`tfctl` can replace it atomically; a file bind mount pins the old inode), `ssh/` (dropbear host key +
authorized_keys, bound over `/etc/dropbear`, which is also `~frame/.ssh`) and `iwd/`, both
bind-mounted from fstab. `dbus-daemon` runs because iwd takes its commands over D-Bus [V].
Logs, dhcpcd leases and the status page live in RAM (`/tmp`, `/run`). No syslogd, klogd, cron.
Kernel log goes to `ttyGS0` (USB-C CDC ACM, built in with CDC ECM `usb0` = 192.168.7.2) and `/dev/console`
is `tty1` (`console=ttyGS0,115200 console=tty1`; the last one wins). ttyGS0 reports no write room until a
USB host configures the gadget, so a user-space write to it waits forever on a wall brick **[V]** (read in
`u_serial.c` `gs_write_room`). The panel never shows console text (`FRAMEBUFFER_CONSOLE` off).

## SSH key only, user `frame`, no sudo at all
Dropbear, key-only. tfctl runs as root for DRM, backlight, clock and updates; there is nothing else to escalate to. Host key generated once into `image/secrets/` so re-flashing a card
does not change the fingerprint.
Exception: a `DEBUG=1` card (bench only, never released) also takes root over SSH with the same keys
(`/root/.ssh` -> `/etc/dropbear`, dropbear without `-w`; post-build.sh), to iterate at the desk.

## Content as a local file by default
`file:///var/lib/thinframe/www/index.html`. No web server needed on the frame. Set
`FRAME_URL` in `/etc/conf.d/thinframe` to point at anything else (Home Assistant, a
dashboard). The page itself can fetch whatever it wants.

## One Rust binary for everything custom
Shell scripts were the first draft. They pull in coreutils behaviour, fork a dozen processes
per invocation and are hard to test. `tfctl` is a single static musl binary (opt-level z, LTO,
panic=abort, stripped; under 2 MB, CI enforces it) with hand-rolled arg parsing and no crates unless a
task proves one necessary. It is the service entry point (spawns and supervises cog), the
updater, the Wi-Fi helper and the status tool. The rest of the system is stock Buildroot: BusyBox
(init and tools), eudev, iwd, dhcpcd, dropbear, dbus, mesa, WPE WebKit and cog.

## Nothing extra in the image
No avahi, no dbus, no ntp daemon, no git, no package manager at all. Debug tools (evtest,
libinput-tools, i2c-tools) are a separate Buildroot config fragment used only for a DEBUG build.

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
The frame stays exactly what it was (Buildroot + cog on DRM). What changes is where the page and
the decisions live: `frameserver`, a Go binary in a scratch Docker image, on whatever box has
spare CPU. The frame's `tfctl agent` polls it for config and posts heartbeats. Consequences:

- Dashboard changes are a container rebuild. The SD card is written once.
- Heavy work (image resizing, API calls, caching, scheduling) runs on the server. The frame's 1 GB
  holds only WebKit and the rendered page.
- Many frames, one server, per-frame config by id.
- Offline: the frame falls back to a local page; the dashboard should also cache itself.

Rejected: streaming pixels (VNC/RDP/WebRTC). Bandwidth and latency are worse than sending HTML
and letting the frame's GPU composite it, and it needs a heavier client than cog.

Transport: see "Two planes" below. The frame opens one WebSocket; the server pushes.

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
"Patched" means the kernel and every package in the image, not only tfctl. The frame has no
package manager: the image is immutable and replaced whole. Card layout grows from 2 to 4 partitions:

| Part | Content | Why |
|---|---|---|
| p1 FAT32 `PFBOOT` | U-Boot script, `extlinux.conf`, `uEnv.txt`, `zImage.a` `zImage.b`, DTBs | U-Boot picks the slot from `uEnv.txt` (`slot=a|b`, `tries=N`) |
| p2 ext4 `rootA` | Buildroot root, read-only mount | active or standby |
| p3 ext4 `rootB` | Buildroot root, read-only mount | standby or active |
| p4 ext4 `data` | `/etc/thinframe.conf`, ssh host keys, iwd profiles, logs, www fallback | survives updates; linked or bind-mounted into the read-only root |

Update flow: CI builds `system-<ver>.tar` = {zImage, modules, rootfs.ext4.zst, sha256}. The container
ships it under `/api/artifacts/`. The agent sees `system_version` change, streams the rootfs into the
standby slot, copies the kernel to `zImage.<slot>`, sets `slot=<new> tries=3` in `uEnv.txt`, reboots.
U-Boot's `boot.scr` (already has a counter in the ormarch build) decrements `tries` before it loads the
kernel, and the start that takes it to 0 boots the old slot: `tries=3` means two starts on the new slot,
then the third start is the old one **[V]** (QEMU A/B test: "tries left 2", "1", then "slot b, tries left
0"). On success the agent's first heartbeat sets `tries=0`, marking the slot good. The server shows each
frame's slot, version and last-good in `/admin`.

Boot-state invariants (boot.cmd.patch, slot.rs, sysupdate.rs):
- A fresh card has `slot=a tries=0` (image/boot/uEnv.txt), no `zImage.b` and an all-zero p3 **[V]**
  (read from image/out/thinframe-sd.img). With tries=0 U-Boot never writes uEnv.txt and never flips,
  and the agent's unconfirmed-slot reboot needs tries>0, so a frame waiting at the Wi-Fi page is never
  rebooted towards the empty slot. `rollback` refuses while the other slot has no kernel.
- uEnv.txt that names no slot (lost in a power cut: FAT has no journal) falls back to `uEnv.bak`, the
  copy tfctl keeps of the state before its last write; tfctl reads the same fallback
  (`UEnv::load_effective`). A slot value other than a or b means a. **[U]** until the QEMU journey runs it.
- `tries` stays below 10: U-Boot's `setexpr` prints hex and `test` reads decimal.
- /data and /boot mount `/dev/thinframe-{data,boot}`, links `tfctl boot fsck` makes to the card found
  by MBR signature (eMMC skipped via sysfs `device/type`), not `LABEL=`, which could match an eMMC
  partition. The data file system is grown whenever its superblock is smaller than p4, so a power cut
  between the table write and resize2fs is finished on the next boot.

Why not a package manager upgrading in place: no atomicity, no rollback, drift between frames, and a half-applied
upgrade on a 1 GB device with a flaky Wi-Fi link is the common case, not the edge case.

## Staying patched
- `.github/workflows/release.yml` can rebuild the image weekly on Buildroot's LTS branch with the
  pinned kernel, run the QEMU boot test and publish `system-<date>.tar`. In practice the owner
  builds images on the Mac (`image/build.sh`) and stages them on the server; frames pick a new
  system up on their next poll and install it to the other A/B slot.
- Kernel: pinned to the ormarch-built 7.2.x line until TASK K1 (the same patches on current
  mainline stable) lands.
- Dependabot/Renovate on `server/` and `tfctl/` dependencies.
- Baseline on the frame: root fs read-only; no password login anywhere; sshd key-only, `frame` user
  only; `doas` for exactly the commands tfctl needs; iwd with WPA2/3 only; no inbound ports besides
  22; all frame→server traffic over the LAN with a shared bearer token (`FRAME_TOKEN`); artifacts
  verified by sha256 from the signed-in config response before install. Outbound HTTPS from the
  frame is unnecessary: the container fetches the world, the frame only talks to the container.
- Secrets never in the image: host key, token and Wi-Fi PSK live on `data` and are provisioned once.
- The token is not in `thinframe.conf`. The conf is world-readable on purpose: `frame` (the only
  ssh user, no root login) runs `tfctl status`, and the real-image test greps it. The token lives
  next to it in `/data/etc/thinframe.token`, mode 0600, root only; `FRAME_TOKEN` in the conf is
  read only as a leftover from an older tfctl and moved out on the next save. cog runs as `frame`,
  so a compromised web process cannot read the server's admin secret. A rollback to a tfctl older
  than this reads an empty token and pairs again (one press at the frame); nothing else is lost. [V]

## Two containers
`frameserver` is production: one binary, one volume, no build tools. `simframe` is development
only: it depends on `frameserver` and pretends to be a frame. They never share an image.
Root `compose.yml` runs the first by default and both with `--profile sim`.
`simframe` has two modes, picked by `SIM_MODE`:

| Mode | What runs | Speed | Tests |
|---|---|---|---|
| `wpe` (default) | headless WPE WebKit + native tfctl agent, screenshots to `sim/out/` | seconds | dashboard rendering, config loop, heartbeats, tfctl updates |
| `qemu` | the real SD image under `qemu-system-arm` (orangepi-pc board, serial console, user-mode net) | ~1 min boot | init, networking, A/B system update and rollback |

Neither mode has the DSI panel or the touch controller; those two gates stay on hardware.

## MCP in the backend
`frameserver` exposes an MCP server (streamable HTTP at `/mcp`, bearer token) so Claude Code or
Claude Desktop can drive it in natural language: "rotate the kitchen frame", "put the weather
widget top-left on all frames", "update every frame to the latest system image", "show me what
the sim frame looks like". Go: `github.com/modelcontextprotocol/go-sdk`. Tools are thin wrappers
over the same handlers the REST API and `/admin` use, so there is one code path.

**Protocol (2026-10-21).** The server is stateless streamable HTTP on go-sdk v1.8.0, which is the newest
release. It serves spec 2026-07-28 (`server/discover`, per-request `_meta`, `resultType`, `ttlMs`/`cacheScope`)
and, for older clients, the initialize handshake back to 2024-11-05. The SDK negotiates the version per
request. **[V]** in `internal/mcp/mcp_test.go` (raw JSON-RPC for every version), and against real clients:
Python `mcp` 2.3.0 and 1.30.0 (what Hermes Agent uses), the TypeScript Inspector CLI, and the Docker image.
- **Results are objects.** Clients on versions before 2026-07-28 validate `structuredContent` as an object.
  `list_frames` used to return a bare array, and Python `mcp` 1.30 failed on it with a pydantic
  `ValidationError`. The output is now `{frames}`, `{actions}` and `{events}`.
- **Hints.** Every tool has a title and annotations (read-only, destructive, idempotent; openWorld false).
  Clients use them to decide what needs a confirmation.
- **Origin.** `http.CrossOriginProtection` refuses cross-site browser requests, as the transport spec
  requires. CLI clients send no Origin and pass.
- **Auth.** It stays a static bearer token, with `WWW-Authenticate: Bearer` on a 401. An OAuth 2.1
  authorization server would be a second service for a home LAN. Every client named above takes a
  header. **[U]** enough until a client needs OAuth.

Initial tool list (TASK S7):

| Tool | Does |
|---|---|
| `list_frames` | ids, ip, last heartbeat, versions, slot, temp, mem |
| `get_frame` / `set_frame_config` | read / change url, rotation, brightness, page_version, tags |
| `frame_action` | reboot, restart-kiosk, update-software, update-system, rollback |
| `screenshot` | PNG of what a real frame's panel shows (2026-10-26, below; the sim had `get_screenshot`) |
| `list_widgets` / `set_layout` | dashboard composition stored per frame as JSON the page reads |
| `get_logs` | last N heartbeats and agent events for a frame |
| `release_status` | what system/software versions the container carries and which frames lag |
| apps (2026-10-26) | `app_guide`, `put_app`, `get_app`, `list_apps`, `rollback_app`, `delete_app`, `show_app`, `get_app_events`, `get_app_state`, `set_app_state`, `send_to_app`, `set_hook`/`list_hooks`/`delete_hook`, `set_poll`/`list_polls`/`delete_poll` (section "Apps", below) |

Writes that touch many frames (`update_system` with no id) require `confirm: true`.

## Buttery smooth: the frame does nothing but render
Interaction latency is the product. Everything else in this repo exists to keep the frame idle
when a finger is not on it, and fully available when one is.

What the frame runs when the page is up: kernel, BusyBox init (asleep), udev (asleep), dropbear
(asleep until a connection), dbus-daemon, iwd + dhcpcd (asleep), tfctl (one thread, wakes every 15 s for a
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

Measurement, not opinion: `tfctl bench [TAPS]` (TASK 13) taps the middle of the panel through a
virtual touchscreen. It uses `/dev/uinput` (`CONFIG_INPUT_UINPUT`) with raw ioctls; there is no
evemu and no crate. The tap takes the real path through udev, libinput and cog. The page's
`pointerdown` handler measures from the event's `timeStamp` to the frame after its highlight was
drawn (two `requestAnimationFrame`s). It sends a beacon with that number and `painted_at_ms`, the
wall clock at that frame.

The server keeps the last 50 samples in `frame.touch`, apart from the heartbeat. They used to sit
in `last_heartbeat.latency_ms`, so every heartbeat wiped them. `/admin` and MCP `list_frames` show
the last value, p50 and max. bench reads `painted_at_ms` back with the frame's token and prints
tap -> paint, which is everything but the touch controller's own I2C read. [V in QEMU, see
log.md; on the panel: U]

The WebProcess memory cap is cog's `--web-mem-limit=256` (MiB). WebKit drops caches at a third of
that and collects hard at half (cog defaults). There is no kill threshold: the server watchdog
already handles a hung page. Swap is off in the kernel (`# CONFIG_SWAP is not set`) and there is
no zram.

## Two planes: control over one outbound WebSocket, data inside the page
The frame never listens. `tfctl` dials `ws://<server>/api/frames/<id>/ws` with the bearer token
and keeps it open (ping every 30 s, reconnect with backoff 1 s to 60 s). Messages are small JSON
objects with a `type` field.

| Direction | type | Payload |
|---|---|---|
| frame → server | `hello` | tfctl_version, system_version, slot, ip, boot_id |
| frame → server | `heartbeat` (every 60 s, and on change) | mem_used_mb, temp_c, wifi_rssi, page_url, uptime_s (touch latency comes from the page's beacon) |
| frame → server | `ack` | id of the command, ok/error |
| server → frame | `config` | the full config object (same as GET /config) |
| server → frame | `action` | id + one of reboot, restart-kiosk, reload-page, update-software, update-system, rollback, screenshot |
| server → frame | `ping` / frame → server `pong` | |

Rules:
- Config pushes are idempotent: the frame applies the whole object and restarts cog only if a
  cog-affecting field changed.
- Actions carry an id and are acked. The server queues unacked actions and resends on reconnect.
- HTTP is still used for: first contact (GET /config before the socket is up), artifact downloads
  (range requests, resumable), and as a 60 s poll fallback if the socket cannot be established.
- The dashboard page has its own channel (SSE from the Go server at `/api/stream/<id>`) for live
  widget data. The control socket never carries page content, so a page bug cannot block updates.
- SSH is for humans. Nothing in the system automates over SSH.
- MQTT, if ever wanted for Home Assistant, is a server-side bridge. The frame does not learn MQTT.

Go side: `golang.org/x/net/websocket` is deprecated; use `github.com/coder/websocket` (minimal,
stdlib-style) or `nhooyr.io/websocket`. Rust side: hand-rolled RFC 6455 client over
`std::net::TcpStream` is ~250 lines (no TLS on the LAN) and keeps tfctl dependency-free; if TLS is
needed later, add `tungstenite` + `rustls` and re-check the 1 MB budget.

## Dashboard composition: server renders HTML from a layout JSON
Each frame has `layout.json` in its store entry: a grid (e.g. 4x3 on 1280x800) of widget
instances `{type, pos, size, props}`. The Go server renders the full page with `html/template`
from a widget library in `server/internal/widgets/` (clock, photo, weather, calendar, text,
iframe). The frame receives finished HTML plus small JSON over SSE for live values. No layout
logic, no templating and no framework on the frame. MCP `set_layout`/`list_widgets` edit the JSON;
`/admin` shows it as a form. Adding a widget = one Go file + one template; the frame is untouched.

## Photos: folder adapter first
`PHOTOS_DIR` (a NAS mount into the container) is the first and default source. The server scans,
resizes once to the panel size (1280x800 and 800x1280), strips EXIF, caches under `/data/cache`,
and serves `/api/data/photo/next?frame=<id>` with a per-frame rotation and a "recent first" bias.
Google Photos, iCloud shared albums and Immich are later adapters behind the same interface
(`PhotoSource` with `List()` and `Open(id)`). The frame only ever downloads a pre-sized JPEG.

## Clock comes from the server
The A33 has an RTC with no backup battery; the frame boots in 1970. The server's `hello` reply
carries `time_unix` and `tz`; `tfctl` sets the system clock and `TZ` on every connect. No NTP
daemon on the frame. Without a server the clock is wrong and the fallback page says so.

## Night mode and backlight
Config gains `brightness` (0 to 100) and `schedule`: `{"off":"23:00","on":"06:30","days":"mon-sun"}`
evaluated on the frame in local time, so it works through a server outage. Off means backlight 0
via `/sys/class/backlight/*/brightness` and cog paused (SIGSTOP) so the CPU idles; on restores
both and reloads the page. The server can override with the `screen` action (`on|off`) and
a frame reports `screen_state` in heartbeats. Touch during off turns the screen on for 60 s.

The light sensor (docs/hardware.md) also turns the screen off: median of 5 one-second readings
at or below 3 for 2 min = dark, at or above 10 for 3 s = lit (tfctl `light.rs`). Off when the
schedule or the dark says off; each acts only when its wish changes, so a server `screen`
action holds until then. `DARK_OFF=0` keeps the screen on in the dark. No crate: the two LRADC
registers are read through an mmap of `/dev/mem`, and the kernel's LRADC key driver is left out
(`linux.fragment`) because the reference-tablet DTS puts volume keys on the sensor's pin. The
thresholds are upstream's covered-hole numbers [D]; whether the panel's own glow lifts the
reading in a dark room is not measured [U]. `tfctl light` prints the value; the kiosk logs it
every 15 min.

## Boot experience
Android never runs: the boot ROM takes our U-Boot from the SD card before the eMMC. U-Boot cannot
light the DSI panel, so the first 5 to 10 s are dark on every possible software; the vendor has
the same limit. From the moment the kernel binds the panel the screen is ours: `tfctl splash`
draws the logo and one line of text directly to a DRM dumb buffer (no GLES, no compositor, no
extra process), the local status page continues the same look while Wi-Fi and the server come up,
then the dashboard takes over. Console output is routed away from the panel entirely.

## SD first, eMMC later
Running from SD is the only configuration where recovery is "remove the card". It stays the
default until A/B updates have proven themselves. Installing to the eMMC (TASKS section E) removes
the card and the vendor software entirely; recovery then needs a rescue card or FEL over USB, and
a verified eMMC backup is a hard prerequisite so the frame can be returned to stock. Done on
2026-10-26: "Running from the eMMC" below.

## One-press pairing
Typing a server token into the flash script was the hardest step of onboarding. Now the token is
optional: a frame without one asks the server to pair (`POST /api/pair` with a random nonce only it
knows) and shows a PAIR button on its screen. The button posts that nonce from cog straight to the
server, which proves someone is at the frame; the server then mints a 256-bit per-frame token,
stores only its SHA-256, and hands the token once to the frame's next poll. A paired id refuses new
requests until the admin page forgets it, so pairing happens once per frame. The per-frame token
opens only that frame's own routes (config GET, heartbeat, control socket) plus release info and
update artifacts; listing, layout, actions and MCP keep needing `FRAME_TOKEN`. A 401 later clears
the frame's token and brings the button back. [V] in QEMU, see docs/log.md.

## The frame finds its server on the LAN (2026-10-22)
A UDP probe to port 41007 (limited broadcast plus the USB link's 192.168.7.255); the server
answers with its HTTP port and the frame takes the address from the reply's source. A frame with
no `SERVER_URL` probes every 10 s on boot; a frame whose server stops answering probes after every
third failure and persists the new address. A frame with a token sends its id and a nonce and
accepts only a reply carrying HMAC-SHA256(sha256-hex(token), "thinframe-discover|id|nonce|port"),
which only a server holding that token (or its stored hash) can make, so answering a broadcast
does not collect a token. mDNS was not chosen: it needs a responder on the server host and a
resolver on the frame, where one datagram each way does the job. Broadcasts do not cross Docker's
bridge network or QEMU's user network: the server needs host networking there, or `SERVER_URL`
set by hand (`DISCOVERY=off` turns the responder off). [U] on the frame until measured.

## Seventies splash and its font
Chosen by the owner on 2026-10-20 from browser mockups. The lettering is Shrikhand (SIL Open Font
License 1.1, license next to the font in image/rootfs/usr/share/fonts/thinframe/). tfctl carries it
as an 84 KB signed distance field atlas (tfctl/assets/shrikhand.sdf) so one atlas draws every size
with smooth edges; the status and pairing pages load the TTF from the rootfs. The atlas is made on
the Mac by tfctl/tools/fontgen, a separate crate with its own workspace. Its crates (fontdue to
rasterize the TTF, miniz_oxide to deflate) never reach the frame binary, which still has no
dependencies. The animation redraws only what moves: angle and radius per pixel are computed once,
and each frame picks a ray colour per background pixel and blends the static layer over it.

## Dawn splash: the default boot screen
Chosen by the owner on 2026-10-21 from five browser mockups (frosted aurora, liquid orb, prism
panes, dawn frost, midnight lens). `SPLASH_STYLE=dawn` is the default; `seventies`, `retro` and
`plain` stay selectable, per frame, over MCP or the config.
- **Glass without a blur.** The splash is drawn in software on the A33 (no GLES, TASK 15). The
  scene behind the sheet is all soft gradients (a 160deg sunrise, two blobs, the sun), so
  frosting is the white tint alone plus a one-pixel rim, brighter on top, and a drop shadow
  outside the sheet. It looks the same as the mockup's `backdrop-filter` and costs nothing.
- **One thin font, two weights of use.** Poppins ExtraLight (SIL OFL 1.1) as a second 64 KB atlas
  (tfctl/assets/poppins-extralight.sdf, `fontgen ... 64 5`). Small text (status, address) is
  drawn with `draw_bold`, which pushes the signed-distance threshold out by half a pixel, so one
  atlas serves both the 176 px title and 16 px text. tfctl went from 879,260 to 963,036 bytes (armv7);
  the limit is 1 MB. The status page loads Poppins-Regular from the rootfs (160 KB).
- **Cheap animation.** Everything is built once into layers (sky without the sun, shadow, glass,
  ink). A frame copies the still picture and recomposes only the box the sun can reach (it drifts
  about 14 x 18 px over 12 s), through a 256-entry glow table indexed by squared distance, so
  there is no per-pixel square root. 150 ms per frame (seventies: 100 ms). The status dot breathes.
- **Same picture on the status page.** The backdrop PNG is the scene without status or address;
  the status page lays its line over it at the same place (126 px in, baseline 524 px down on
  the 1280x800 page).
- **The pages that wait for a person sit on the sheet itself.** Wi-Fi setup (list, password and
  keyboard, joining) and PAIR have no cards under `dawn`: the content is set straight on the
  frosted sheet, with ExtraLight headings, ink pill buttons and white glass keys
  (tfctl/src/dawnpage.rs, one stylesheet for both; the markup is unchanged, so the seventies CSS
  stays where it was and is used for the other styles). Their backdrop is the sheet alone
  (`write_page_backdrop`: the scene with an empty title, no mark and no lettering), because
  the big title would sit behind the list. Poppins Regular and ExtraLight come from the rootfs
  (320 KB), served to the Wi-Fi page as `/poppins.ttf` and `/poppins-light.ttf`.
- **Memory.** One journey run read 339 MB used with the page up against 282 to 284 MB for every
  other run, with this splash and without it. The next runs read 284 (dawn) and 282 (seventies):
  one-off, WebKit's collector. The journey's budget check now takes the median of three readings
  15 s apart.

## Wi-Fi setup on the frame's screen

A frame with a radio and no connection shows a Wi-Fi page before the PAIR page (TASK 17), so
setup is: power on, pick Wi-Fi, press PAIR. That way
no one has to type a password into `flash.sh`, and moving house only takes a tap. **[V in QEMU]**
- **Where the page lives.** Pairing posts to the server. Before Wi-Fi works there is no server, so
  the agent serves the page itself, from a small HTTP listener in tfctl on 127.0.0.1:8091.
  - It binds loopback only and checks the Host header, which defeats DNS rebinding.
  - It answers only while the setup page is on screen. A dashboard page cannot reach it once the
    frame is online.
  - It is std only, about 150 lines; no crate.
- **No D-Bus client.** tfctl drives iwd through `iwctl` (built only with readline, hence
  `BR2_PACKAGE_READLINE=y`; readline is GPL-3.0 and so is iwctl, iwd itself stays LGPL) and parses its tables, colour escapes
  included, as status.rs already does for RSSI.
  - The join is `iwctl --passphrase P station wlan0 connect SSID`, after
    `known-networks SSID forget`. A stale profile from provision.txt would otherwise win over the
    new password.
  - iwd writes the profile to /var/lib/iwd (bound to /data), so it survives updates.
  - The password sits in iwctl's argv for the seconds the join takes. Only root runs anything on
    the frame. **[U]** acceptable.
- **When it shows.**
  - Nothing is configured: at once.
  - A network is configured but has not come up: after 45 s, which gives iwd's autoconnect time
    at boot. The page then names the missing network.
  - Until then the status page says "joining <ssid>".
  - No `wlan0` at all (the Mac, a QEMU without hwsim): never.
- **What it joins.** psk and open networks only. Enterprise (8021x) and WEP are listed but cannot be
  tapped; they need a profile by hand (`tfctl wifi` or an iwd file). Hidden networks: not yet.
- **The keyboard.** It is about 30 lines of JS in the page: letters, shift, digits and symbols. The
  password shows as you type (a hide key masks it), because typing blind on a frame helps no one.
- **Sim.** The QEMU image alone adds `mac80211_hwsim` and hostapd, with access points
  "Groovy Wifi" (WPA2) and "Cafe" (open). S35simwifi starts them. It is a shell init script, but
  it lives in the qemu-overlay that only qemu.fragment includes, so no frame image carries it.


## Touch firmware from the frame's own eMMC

The Silead table `gsl_m86_hd` is the vendor's, so it may not be in Git or in a published image.
An owner who builds the image passes it in with `PF1007L_TOUCH_FIRMWARE`. A release image has none,
and without it the frame cannot be tapped, so Wi-Fi setup and PAIR (TASK 17) would be stuck.
- **Where it comes from.** Every PF1007L still has its Android on the eMMC. `tfctl boot` reads it from
  there, at the Android boot image at sector 172,032 (recovery at 2,400,256 is the fallback). From that image
  it takes the gzip cpio ramdisk, then `gslX680new.ko`, then the `gslx680_fw_grp` entry. The method is
  ormarch E025/E028 and `image/vendor/extract_gsl_fw.py`, ported. **[D]** offsets, **[V]** parser on a
  synthetic image.
- **Safety.** The eMMC is opened with `File::open`, which is read-only, and only after its MBR is
  checked as *not* ours. The module has no write path. "Never write the eMMC" holds by construction.
- **Trust.** The result is used only if it has the silead page layout and the SHA-256 the project
  tested (e11ffa8a…). Any other firmware, from a different panel batch, is refused and logged.
- **Loading.** The root is read-only, so the file goes to `/data/thinframe/firmware/silead/` and
  `/sys/module/firmware_class/parameters/path` points there. tfctl then asks the I2C bus to probe the
  unbound `silead` device again (`/sys/bus/i2c/drivers_probe`). It does this on every boot, because
  udev loads `silead` before /data is mounted.
- **Cost.** No crate. It reuses png.rs's inflate and sha256.rs. +13 KB, so tfctl is 863 KB.

## Remote debugging: reports, a log that survives the boot, debug mode
The frame has no root login and its log lived on tmpfs, so the only view of a bad boot was a
serial cable. Three small things fix that without a logging daemon or a second shell script:
`tfctl boot` writes every step line to `/dev/kmsg` as well as the console (dmesg keeps it for
the whole boot); a thread in `tfctl kiosk` copies new log bytes to `/data/thinframe/log/current.log`
every 15 s and rotates it to `previous.log` on a new boot id (bounded at 512 KB, the only card
writes logging causes); and a `report` action posts one bounded text file with versions, config,
uEnv, mounts, processes, dmesg, Wi-Fi state and both logs to `POST /api/frames/<id>/report`, which
the server keeps (last 20) and serves to `/admin`, the API and MCP (`list_reports`, `get_report`).
Debug mode (`debug` in the frame config, `DEBUG=1` in the conf) adds socket traffic, config
writes and cog's command line to the log, runs cog with `G_MESSAGES_DEBUG=all`, and sends a
report on the first contact of every tfctl process. The token stays out of all of it (it is not
in the conf). Cost: 11.7 KB of binary (995,284 bytes, 4.7 KB under the limit). [V] in QEMU.

## Hardware watchdog, and budgets that are targets
A frame no one is standing next to must recover from a hung kernel on its own. The A33's
watchdog (`sunxi_wdt`, 16 s, built into the kernel by `linux.fragment`) is opened by a thread at
the top of `tfctl kiosk` and petted every 5 s. Not NOWAYOUT: if tfctl dies the kernel keeps the
timer armed only until the respawned tfctl reopens it, and a frame with no tfctl at all stays
dark rather than rebooting every 16 s. Upstream saw the reset work on this board (ormarch E041).
QEMU exercises the same code with the virt machine's `i6300esb` (`-watchdog-action reset`). [V] in
QEMU, [U] on the frame until the first hang.

The README budgets (rootfs 200 MB, 300 MB with the page up, tfctl under 1 MB) were our own
targets, not hardware limits: each root slot is 1 GB and the board has 1 GB of RAM. They stay as
targets to measure against; the only gate is tfctl under 2 MB in CI, raised from 1 MB because a
few kilobytes of headroom was costing iteration time (report, log mirror and debug mode cost
12 KB together). The point is still that the OS stays small so the dashboard gets the machine.

## Boot time and Wi-Fi that stays up (2026-10-22)
Measured on the frame, kernel start to dashboard: from about 25 s (status page at 18.4 s) to
14.8 s, and the link no longer drops in the first minute [V]. What cost the time, and the fix:

| Cost | Fix |
|---|---|
| udev and iwd waited ~3 s for the RNG ("crng init done" at 4.2 s) | `seedrng` at sysinit with the seed on /data; the kiosk saves a creditable seed once the RNG is ready, and shutdown saves one more. RNG ready at 1.0 s. |
| Two software renders of the splash scene, ~4 s each on the A33, before cog started | `rendercache.rs`: the pixels and the status page backdrop are kept on /data, keyed by every input plus the tfctl binary and logo; a torn file is a miss. The first frame of an animated splash leaves out the address (new every boot), the animation adds it. |
| Wi-Fi joins declined at boot, then iwd waited 60 s and more (blacklist, up to a day by default) | `he_on=0` for the AIC8800 (with 802.11ax the access point declined joins and dropped the frame ~31 s after each, reason 34); iwd's blacklist 10 s flat; the agent asks iwd to rejoin the strongest saved network while Wi-Fi is down. |
| "Connected" before DHCP: the agent hit "network unreachable" and moved its server | online means an IPv4 address on wlan0; dhcpcd without the ARP probe, IPv4 only |
| A failed cable probe waited 20 s | retried after 2 s; the move off the cable address asks each interface's broadcast address, every minute |

Radio power save is off too: the frame is on mains power. The reasons came from `wifitrace.rs`,
which runs iwmon (kernel `CONFIG_NLMON`) and logs only link events and their codes, so the next
Wi-Fi problem on a frame no one is near is readable in its log.

## Power can be pulled at any time (2026-10-23)
A frame on a wall gets unplugged mid-write. Every file tfctl writes to the card now ends up as the
old content or the new, never torn or empty, and no state change happens before what it depends
on is on the card [V in tests; the cut itself U, see docs/log.md]:

| Write | Before | Now |
|---|---|---|
| Software update | `tar` of an overlay into the running root, then the binary swap | No overlay: root changes ship only as A/B system updates. The new binary is staged and synced, the marker synced, then one rename. A marker whose version is not installed (cut before the rename) is dropped without an event. |
| Crash-loop rollback | Three starts within 60 s of wall time; the A33 has no clock battery, so two power cuts right after an update looked like a crash loop and rolled a good update back | Starts carry the boot id and uptime; only starts in the same boot count. |
| A/B switch | uEnv.txt could name the new slot before its zImage was on the card; the "attempted" marker was written after | zImage synced, then the attempt marker (synced), then uEnv.txt. |
| Wi-Fi profile, authorized_keys, events, splash.png, starts | written in place | `update::write_synced`: temp file, fsync, rename, directory fsync |
| provision.txt | removed even when /data was not mounted (settings lost) and before the writes were synced | kept for the next boot when storage was read-only; synced before it goes |
| Log copy on /data | trimmed by truncating in place | the tail written to a temp file and renamed over it |
| thinframe.conf | one invalid byte stopped `tfctl boot` and the kiosk | read lossily; the lines that parse count |

## Boot: supervised services, nothing waits (2026-10-23)
Kernel start to dashboard on screen, warm boot: 14.3 s before, 11.7 s now; init scripts done at
1.9 s (was 5.4 s with the udev settle) [V 2026-10-23, audit4 build].
- iwd, dhcpcd, dropbear and udhcpd are inittab `respawn` lines instead of init.d scripts started
  once: a daemon that dies comes back within a second (iwd died at boot once and the frame stayed
  offline until its slot fell back). iwd logs to `/var/log/iwd.log` (RAM).
- `udevadm settle` does not wait (`/etc/default/udevd` SETTLE_TIMEOUT=0): it held rcS for 3.9 s,
  and nothing after it needs a settled device tree (tfctl waits for what it uses).
- `tfctl` reads cog, WebKit and the libraries into the page cache in a thread at kiosk start
  (`prewarm.rs`), and the splash stays until the agent names the page (`FIRST_PAGE_HOLD`, 12 s at
  most), so cog starts once, on the dashboard, instead of on the status page and again.
- The agent starts the moment wlan0 or its SDIO device exists, not after a fixed wait.
- When Wi-Fi has been down 20 s, the frame also looks for the server on the USB cable, so a frame
  with broken Wi-Fi can still be reached and fixed.
- Every kiosk log line, cog's too, carries the uptime (`logstamp.rs`), so a boot timeline comes
  from the log alone.
- Tried and dropped: iwd `[DriverQuirks] DefaultInterface=*` (keeps the driver's wlan0 instead of
  re-creating it). On the AIC8800 the frame then found no saved network and iwd exited [V
  2026-10-23, two boots].

## Tabs, pushed sources, display-only controls (2026-10-25)
- **Data is pushed, not fetched, for the new widgets.** Cars, rooms, shows and the like come from
  services that need private credentials. The server stores named JSON sources (`PUT /api/sources/{key}`,
  bearer) and replays them to the page; whatever talks to Home Assistant or a car API pushes there.
  The server gains no connector code and no new dependency. Only `docker ps` over `ssh` is built in,
  because it needs nothing but an ssh key [V on the owner's server, 2026-10-25].
- **Personal data stays out of Git.** The repo ships the widgets, the theme and a demo with made-up
  names; a real frame's layout and sources live in `DATA_DIR`. Images are `asset:` files there,
  served publicly like the page (the frame's browser has no token), limited to raster images and
  woff2 so a file opened directly cannot run script.
- **Controls are display-only.** The page is public by design (cog has no token), so a button on it
  that unlocks a car could be pressed by anyone on the LAN. Acting needs a per-frame credential on
  the page plane first; until then the cards show state only.
- **No font in the repo.** Inter is OFL but 300 KB+; the Obsidian theme uses it when the owner drops
  `inter.woff2` into `DATA_DIR/assets` and falls back to DejaVu Sans, the frame's only font.
- **No new crates or modules.** Rendering is the existing html/template plus page.js; tabs are CSS
  pages toggled by class, no animation beyond opacity, for the Mali-400.

## Production on the home server, the Mac for testing (2026-10-25)
- The server runs as a container on the owner's Docker host, built on the Mac and shipped with
  `docker save | ssh docker load` (scripts/server-deploy.sh), tagged `local/…` so the host's
  update manager does not try to pull it from a registry.
- Host networking: frames find the server by UDP broadcast on 41007, which a bridged port map
  does not receive.
- Container status comes from the host's Docker socket, mounted read-only. The server only sends
  `GET /containers/json`, but anyone who took over the process could use the socket fully (`:ro`
  protects the file, not the API) [V: Docker docs]. The same host already gives the socket to
  two management containers; a filtering socket proxy is the step up if that changes.
- The Mac server keeps `DISCOVERY=off`, so a frame that loses production cannot land on it.
- Page speed on the A33: widgets on hidden tabs are drawn when shown, repeated data is skipped
  in the page and on the server, icons are parsed once, no blurred shadows. In WPE 2.48 (sim,
  arm64 container): load 70 ms, tab switch 1 to 38 ms, all draws 25 ms [V 2026-10-25, sim].


## HTTP port 41007, the same number as discovery (2026-10-25)
- The server's default HTTP port moves from 8080 to **41007** (`PORT`). 8080 is crowded on the
  owner's LAN; 41007 is already thinframe's UDP discovery port, and TCP and UDP ports do not
  collide, so one number covers both: "thinframe lives on 41007". `PORT` still overrides it;
  in compose it sets both sides of the port map and the port the discovery reply carries.
- Existing frames with `SERVER_URL=http://<host>:8080` move on their own [V 2026-10-26: the
  office frame went from `http://server.nox:8080` to `http://192.168.1.5:41007` 22 s after the
  server moved, untouched; `FRAME_URL` moved with it]: after
  REDISCOVER_EVERY failed connects the agent broadcasts on UDP 41007, the server (host
  networking) answers with its new port and an HMAC proof, and tfctl persists
  `SERVER_URL=http://<server-ip>:41007` (`agent.rs` `move_server`). The new URL uses the
  replying address, not the old hostname. A frame with a pinned `FRAME_URL` keeps that page URL.
- Fallback when rediscovery cannot work (server with `DISCOVERY=off`, other subnet): rewrite the
  card's settings with `scripts/flash.sh --settings-only` and the new URL, or type it in
  Settings on the frame (hold the screen 5 s, below).

## Settings on the frame: hold the screen 5 s (2026-10-26)
- The frame must never need a card or a computer to recover. A finger held on the screen for 5 s,
  on any page, opens Settings: change Wi-Fi (also while connected), forget saved networks, find
  the server on the LAN or type its address, see addresses and versions, restart [V 2026-10-26:
  opened on the frame by an injected 7 s BTN_TOUCH hold].
- The hold is read by tfctl from the touch device, not by the page, so it works over the
  dashboard, the status page, a broken page and with no server at all. Taps never add up to a
  hold; the finger may move. 5 s is long enough that nobody does it by accident on a dashboard.
- tfctl serves it on 127.0.0.1:8092, like the Wi-Fi setup page: loopback only, Host header
  checked, 404 unless on screen. It closes after 3 min untouched; a server `restart-kiosk` waits
  while it is open (the page watchdog sees the dashboard go quiet).
- A typed address without a port is tried on 41007, 8080 and 80 (`GET /healthz`); one that does
  not answer can still be saved ("use anyway"). A LAN search first asks for a server that proves
  it knows this frame's token, then for any server, which then shows PAIR.
- A frame with no `SERVER_URL` and no Wi-Fi opens Settings on Wi-Fi after 45 s: the Wi-Fi setup
  page belongs to the agent, which starts only once a server is known.
- Pages change in the running cog: cog gets a private D-Bus session bus
  (`/run/thinframe/cog-bus`) and the kiosk sends `cogctl open URL`, falling back to a restart.
  Settings came up in 0.9 s this way; by restart it took 8 s (2.2 s to end cog, 3.9 s of
  splash, 2 s to load) [V 2026-10-26].

## Apps: MCP writes any page, sandboxed (2026-10-26)
- Widgets cover what someone thought of in advance. For everything else MCP writes an **app**: a
  folder of HTML, CSS, JS, images and fonts (index.html required; up to 64 files and 4 MB). The
  frame shows it full screen (`show_app` sets the frame's page URL to
  `/apps/<name>/index.html?frame=<id>`) or in any grid cell as the widget `app`. `app_guide` is
  the reference an MCP client reads first. [V 2026-10-26: Go tests, and Chromium (Playwright)
  driving both modes against a local server.]
- **Versions.** Every `put_app` is a new version in `DATA_DIR/apps/<name>/v<N>/`; the last 10 are
  kept and `rollback_app` switches back. Pages showing the app reload on their own after a put or
  rollback. `delete_app` is refused while a frame shows the app, so no frame loses its page.
  `put_app merge:true` sends only the changed files.
- **Sandbox.** Every app document is served with `Content-Security-Policy: sandbox allow-scripts
  allow-forms ...` (no `allow-same-origin`), full screen as well as in the widget's iframe. It
  runs in an opaque origin: it cannot read this server's pages or the admin page's cached Basic
  credentials, post admin forms (they refuse cross-origin) or reach `/api` with a token it does
  not have. Content written by a model that read the web stays harmless to the server. Cost:
  no localStorage or cookies (they throw), hence server-side app state.
- **SDK.** The server injects `/apps-sdk.js` first into every HTML file: `window.thinframe` with
  `data`/`on` (sources, live), `emit` (buttons), `state` (JSON per app on the server, 64 keys,
  64 KB, pushed to every frame), `listen` (events from MCP `send_to_app`). Full screen it opens
  its own event stream (`/api/stream/<id>?apps=1`) and sends the page watchdog's alive beacon.
  In a widget it asks the dashboard over `postMessage`, which keeps the page's one stream: WebKit
  allows six connections per host, so one stream per app widget would starve the page. The
  dashboard forwards only an allowlist (sources, its own frame's emit, its app's state). On the frame's
  WPE WebKit the full-screen mode works as in Chromium (CSP sandbox on a top-level document,
  opaque-origin fetch with `Access-Control-Allow-Origin: *`) [V 2026-10-26, docs/log.md]; the
  widget bridge is [V] in Chromium, [U] on the frame.
- **Public endpoints**, like the dashboard plane (the page has no token): `/apps/*`,
  `/apps-sdk.js`, `GET /api/data/source/<key>`, `GET|POST /api/apps/<name>/state[/<key>]`,
  `POST /api/frames/<id>/emit`. They answer any origin. Emit is limited to 20 per 10 s per frame,
  state writes to 80 per 10 s per app; emit needs an existing frame and never creates one.
  Sources become readable by anyone on the LAN, as they already were through any page's stream.
- **Buttons and outside APIs without secrets in pages.** A *hook* (`set_hook`) is a named HTTP
  request the server makes when an app emits that name: URL, method, headers (tokens) and a body
  template with `{{data}}`. A *poll* (`set_poll`) fetches a URL every 15 s or more into a source.
  Both live in `DATA_DIR/hooks.json` / `polls.json` (mode 600), are set only over MCP, and are
  listed with header values redacted. The server makes requests to URLs the owner chose; a page
  can only pick a hook by name.
- **Errors.** The SDK reports uncaught errors and rejections as `app-error` emits (5 per load),
  so `get_app_events` shows what broke after a put.

## Screenshots of the real frame (2026-10-26)
- The `screenshot` action makes tfctl read the scanned-out framebuffer through DRM (no GPU
  readback), turn it upright in the page orientation and POST raw RGB to
  `/api/frames/<id>/screenshot?w=&h=` with its paired token. The server stores the last 10 per
  frame as PNG; MCP `screenshot` waits up to 25 s and returns the image (scaled to 800 px wide by
  default). [V 2026-10-26: capture on the frame 342 ms, 1280x800, upright, colours right;
  upload path tested in Go.]
- An MCP client can now see what it built: `put_app`, `show_app`, `screenshot`, fix, repeat.

## Aurora theme and the left rail (2026-10-26)
- The owner found the dashboard plain. `theme: "aurora"`: translucent "glass" cards (a white
  gradient at 4 to 9 % with a 1 px lit top edge) over coloured light, two or three radial
  gradients on one fixed layer behind the page. The light and the accent colour follow the
  active tab's icon (`html[data-mood]`); on Home they follow the time of day
  (`html[data-daypart]`: dawn, day, dusk, night, checked every 5 min). Big numbers get a
  white-to-translucent text gradient. Empty cards get a dashed outline instead of a solid card.
- Mali-400: nothing animates. No backdrop-filter (glass is plain alpha over the light), no
  animated shadows; the active tab's shadow repaints only on a tab change. [U] touch-to-paint
  stays where obsidian was: measure with `frame_action bench` while the screen is on.
- The tab bar moved to a rail on the left by default (`nav: left|bottom`): on a 1280x800
  landscape panel the bottom pill took height from every grid row; width is the spare axis.


## Aurora, turned up: Manrope, drifting light, grain (2026-10-26)
- Type: Manrope (SIL OFL 1.1, `server/internal/widgets/fonts/OFL.txt`), one variable woff2
  (weights 200 to 800) embedded in the server and served at `GET /fonts/manrope.woff2`
  (public, like the page). Without it the frame falls back to DejaVu Sans. [V] renders in
  Chromium from the server; [U] WPE on the frame, check with a screenshot.
- Motion: none since the touch work below (the drifting light was removed). No backdrop-filter
  or blend modes.
- No animation on a tab switch. Cards rising for 0.55 s made a switch take 150 to 1800 ms
  [V 2026-10-26, `tfctl bench 8 24,5 50,5` taps the nav rail]; an opacity fade per card or of
  the whole page cost the same (each gives WebKit a new composited layer to raster on the
  first frame). Without: p50 28 ms, max 47 ms. The drift costs nothing measurable.
- A static SVG noise layer at 7 % opacity breaks up gradient banding. Cards get a corner sheen
  and a 1 px top edge in the tab's accent; bars, the energy "now" bar and status dots carry a
  small fixed glow in the accent colour; the Home clock has a soft glow behind it.
- Touch first (2026-10-26, owner: "touch has to be insanely responsive"). Measured on the frame
  with `tfctl bench TAPS X,Y ...` and a page that reported every frame for 1 s after a tap [V]:
  - Cards dimmed to 70 % on touch with a 0.25 s fade: 0.8 to 0.9 s of stalled frames per tap,
    single frames up to 340 ms. An instant dim still 0.43 s, a border highlight 0.3 s: any change
    to an Aurora card makes WebKit redraw it on the Mali-400. Cards no longer react visually;
    after the change a card tap stalls nothing (next frame 20 to 50 ms).
  - Tab switch: the tab now lights on touch-down and the page follows two frames later. The
    mood moved from `<html>` to each page and tab (an inherited colour change redrew the whole
    screen, ~300 ms), the light no longer drifts (the running animation cost 100 to 150 ms per
    switch), and blurred shadows, gradient card fills and gradient-clipped numbers went (another
    25 to 30 %). Before: up to 1.8 s. After: highlight p50 60 to 80 ms, page drawn 250 to 300 ms
    after the touch; first visit of a tab after a reload up to ~650 ms.
  - Tried and rejected: CPU painting (`WEBKIT_SKIA_ENABLE_CPU_RENDERING=1`, 2x slower), 4 CPU
    painting threads or GPU-affine hybrid painting (no change; GPU-only won later, see "Painting
    on the GPU only"), keeping hidden pages as layers
    (opacity 0 or off-screen: WebKit drops their tiles; opacity .004 was slower).
  - The floor, plain theme with the same page code: highlight 80 to 140 ms. Faster than that
    needs less on the page, not CSS tricks.

## Painting on the GPU only, and GPU hangs (2026-10-26)
- Owner: the screen "periodically tears / shakes when the touch screen is used". WPE's default
  Skia mode is hybrid: each 512 px tile is painted on a free CPU worker or else on the GPU, so
  neighbouring tiles, and one tile from one redraw to the next, came from two rasterizers that
  draw text, gradients and pictures slightly differently. tfctl now starts cog with
  `WEBKIT_SKIA_CPU_PAINTING_THREADS=0` (every tile on the GPU). Tab switch on the frame, live
  data copy, 10 switches [V]: hybrid 300 to 800 ms, GPU-only p50 314 ms (tap to page painted
  p50 281 ms, max 444 ms). Whether the tearing is gone is [U] until the owner has used it.
- `--web-mem-limit` 256 to 512 MiB. At 256 the page (~100 MB) sat above WebKit's one-third
  threshold, so it purged decoded pictures and tiles every ~25 s ("Memory pressure relief", 41
  times in 1,000 s) and repainted them on the next touch: the "periodic" part. At 512: none in
  the first 250 s [V]. The frame has 1 GB.
- `COG_ENV="NAME=value ..."` in `/etc/thinframe.conf` (or `/data/etc`) adds or overrides cog's
  environment, to try a WebKit knob without a new tfctl. Local only: the server cannot set it.
- GPU hang guard (`gpuwatch.rs`). Killing cog mid-frame gave the Mali-400 an MMU page fault;
  lima's reset then failed on every job ("pp0 job timeout" / "bus stop timeout" every 10 s) and
  the screen stayed black for 11 minutes while the page itself ran, so the server watchdog saw
  it alive [V dmesg 78129 to 78795 s]. A new cog cleared it. tfctl now reads `/dev/kmsg` and
  restarts the kiosk on the second lima job timeout within 30 s, at most once per 2 minutes.
- Rejected: pinning the GPU at 384 MHz (devfreq `min_freq`): no change to tap latency.
  Removing the Aurora background layers: 60 ms less paint, same total. Keeping hidden pages
  as layers (z-index, transform, opacity stacking): WebKit repaints on each, see "Touch first".

## Cameras and the doorbell (UniFi Protect) (2026-10-26)
- The owner uses UniFi Protect. The server reads it through Protect's official Integration API
  (API key, `X-API-KEY`): `GET /v1/cameras`, `GET /v1/cameras/{id}/snapshot`, and the
  `subscribe/events` websocket for rings. Paths and message shapes from the uiprotect library
  [D]; [U] on the owner's console until a ring is logged. No new dependency: the websocket client
  is `coder/websocket`, already used for the frame socket.
- Snapshots, not video. The frame's WebKit cannot play RTSP(S), and a 640-wide JPEG every few
  seconds is cheap for the A33 to decode. Each picture is decoded off screen and swapped in;
  one request per picture in flight. The server shares one snapshot fetch per camera per second
  between frames.
- The page plane is public (no token), so the server only serves cameras a `cameras` widget on
  some layout lists (or all, when the widget lists none) and doorbells that rang in the last
  day. Anyone on the LAN who can open a dashboard can see those cameras, as they can see the
  dashboard itself.
- A ring: every page gets `{"doorbell":{id,camera,name,age_s}}` and pops the camera over the
  page for 90 s (a dismissed ring stays dismissed across reloads, kept in localStorage). An off
  screen has a stopped browser, so every connected frame also gets `wake`: the screen comes on
  for 2 minutes like a touch, then the schedule and the light decide again. A plain `screen on`
  would hold until the next schedule change, all night. The page the wake reloads gets the ring
  from the replay on connect. [U] time from ring to picture on the frame: cog restarts on wake.
- Protect's public events carry no face names, so "who is it" is the live picture.

## Calendar recurrence (2026-10-26)
- A family calendar is mostly repeating events, and the feed carries each one once with an
  RRULE. The server now expands them itself, stdlib only (`server/internal/api/rrule.go`), on
  every read of the cached feed, so the cache stays right as days pass. [V] unit tests.
- Supported: FREQ DAILY, WEEKLY, MONTHLY, YEARLY; INTERVAL; COUNT; UNTIL (date, UTC or floating
  date-time); BYDAY (with ordinals like 2TU, -1FR for MONTHLY and YEARLY); BYMONTHDAY (negative
  counts from the month's end); BYMONTH; BYSETPOS ("first weekday of the month"). WKST is ignored (weeks start on Monday). EXDATE (several
  lines, comma lists, TZID, VALUE=DATE). A VEVENT with RECURRENCE-ID replaces that occurrence;
  with STATUS:CANCELLED it removes it. Cancelled events are dropped. [V] unit tests.
- Each occurrence keeps the event's length (DTEND, else DURATION); all-day events keep whole
  days. Rules repeat in DTSTART's zone (its TZID; UTC for a `Z` time; `TZ` for floating and
  all-day), so a 9:00 meeting stays 9:00 across DST. [V] unit test across 2026-11-01 in
  America/Los_Angeles.
- Only occurrences from a day before now to 60 days ahead are made. A rule without COUNT jumps
  straight to the window; at most 5000 days, weeks, months or years are walked per rule, so a
  bad rule cannot spin (a COUNT rule that starts thousands of periods back shows nothing).
- Not supported: BYWEEKNO, BYYEARDAY, BYHOUR and finer, HOURLY and finer, RDATE,
  RANGE=THISANDFUTURE. Such a rule is not expanded: the event shows once, at DTSTART, and the
  rest of the feed still works. [U] whether the owner's iCloud feeds use any of them: check the
  frame against the Mac's Calendar for a week.
- `CALENDAR_ICS_URL` takes several URLs (spaces, commas or newlines). Each feed keeps its own
  10-minute cache and backoff; they are read in parallel and merged by start. A failing feed is
  logged and left out; the widget says "calendar unavailable" only when every feed fails with
  nothing cached. `webcal://` is read as `https://`.

## Private calendars: CalDAV for iCloud, the secret iCal address for Google (2026-10-26)
- The owner wants a calendar on the frame without making it public. iCloud has no private feed
  link: Share Calendar's public link is readable by anyone who has it [D]. So the server reads
  iCloud the way a phone does, over CalDAV (RFC 4791) with an app-specific password. Google
  calendars have a "secret address in iCal format", which `CALENDAR_ICS_URL` already reads; its
  CalDAV needs OAuth, which would mean a Google Cloud project and a consent screen for no gain.
- `caldav.go`, standard library only (net/http, encoding/xml): PROPFIND the principal, its
  calendar-home-set, the calendars in it (event calendars only), then one calendar-query REPORT
  per calendar for a day back to 62 days ahead. Each event's calendar-data goes through
  `ParseICS` and the same recurrence expansion as the feeds, so both paths behave alike. Same
  10-minute cache and backoff (`refresher`). CalDAV and the feeds merge in `Calendars`.
- Redirects are followed by hand. Go's client turns a redirected PROPFIND into a GET and drops
  the Authorization header on another host. The password is sent only to the configured host or
  hosts under its parent domain (iCloud answers from `pNN-caldav.icloud.com`), and to an IP
  address only exactly; never to a host a redirect or an href names outside that.
- The password lives only in the server's env file (mode 600 on Nox). The widget shows event
  times and titles only, as it does for feeds.

## More from the light sensor: wave, motion, auto-brightness (2026-10-26)
- The owner asked for "a motion sensor". The PF1007L has one analog light sensor (LRADC0, 6 bits;
  light.rs), no PIR or proximity part. tfctl now reads it 10 times a second (it was once a
  second; the kiosk loop passes every 100 ms when the sensor exists) and `sense.rs` derives:
  - **Wave** (`wave`): a dip to a quarter of a lit baseline (at least 12) for 0.15 to 1.5 s, then
    back. Off screen: on for 60 s like a touch. On: the page turns to the next tab (frame event
    `light wave`, the server relays `{"gesture":"wave"}`), or closes a camera view.
    [D] upstream E064: 60..62 lit, 7..14 partly covered, 0..2 covered; [U] on this frame.
  - **Motion** (`motion_wake`, experimental): the median of the last 0.3 s moves 3 counts and 8 %
    from the baseline, once per 10 s, only with a baseline of 8..57. A bright room reads 60..62,
    the converter's top, where a passing shadow changes nothing, so motion works in lamp-lit
    rooms only. Each detection is a `frame:light motion <value>` event in get_logs, to tune by.
    It wakes the screen when the room is not dark (e.g. off by schedule). [U]
  - **Auto-brightness** (`auto_brightness`): the backlight follows the room light in the vendor's
    bands (10/17/…/59 → 25 % to 100 % of `brightness`), 4 points a second at most.
- [V 2026-10-26] In the dark office at night the sensor read 0 in ~2,000 reads over 20 s with
  the backlight on: the panel's own light does not reach the sensor, so auto-brightness cannot
  chase itself.
- Off by default; the server's frame config turns each on (MCP `set_frame_config`).

## QA pass 2 (2026-10-26)
- Every tab and button clicked on a copy of the live data (hooks pointed at a local sink): all
  car commands reach the `car` hook, the rail, the House sub-tabs and the wave gesture switch
  pages. The wave gesture had been broken since tabs switch on pointerdown: it sent a click.
- Room cards look like switches, so a tap now does something: it emits `room_on` or
  `room_off` with `{"area_id"}` (the room's `area`, else Home Assistant's default id from its
  name). With no such hook on the server the card says "Room lights are not connected yet" and
  nothing flips; with one, the card flips at once and the next rooms update confirms it.
- System: themes' `.w` padding no longer pushes the services page off the grid; services are
  chips that wrap, and with an odd number of groups the biggest takes the full row.
- Aurora no longer lists Inter as a fallback: Chrome fetched `/api/data/asset/inter.woff2` (404)
  for any glyph Manrope lacks. The weekly forecast fills its card; climate zones sit at the
  bottom; the home cars spread out; an empty day in Shows says "Nothing new"; the "have it"
  check is a green mark that no longer wraps; the toast appears without sliding.

## Recovery from anything (2026-10-26)
Owner: "allow recovery from any part of the system", and later run from the eMMC with no card.
Every layer now has a way back that needs no computer where one is possible.

| Fails | What brings it back |
|---|---|
| Wi-Fi password changed, router gone | Wi-Fi setup opens by itself after 45 s without the saved network; Settings → Wi-Fi any time |
| Server down, moved, renamed | Status page says so; LAN search; Settings → Server: find on the network or type an address |
| Pairing lost (token mismatch) | 401 clears the token; the frame shows a pairing code |
| Someone wants to start over | Settings → Reset: forgets Wi-Fi, server and pairing (config saved first), restarts |
| How to reach Settings | Gear at the bottom of the dashboard rail; Settings button on the boot/status screen; a 5 s hold anywhere |
| A page tries to misuse Settings | Any page may open it (once per 10 s); every other Settings form must come from Settings itself (Origin check) |
| No server for the clock | SNTP (pool.ntp.org, time.google.com) at start and daily; the RTC is written whenever the time is set and the kernel reads it at boot |
| GPU hang (lima job timeouts) | Kiosk restart on the second timeout in 30 s; still hung within 10 min of that restart: reboot |
| Kernel oops | `panic_on_oops=1` (set by `tfctl boot`) + `panic=10`: reboot |
| tfctl's loop stuck | Hardware watchdog is petted only while the kiosk loop beats (120 s), so the SoC resets |
| tfctl crashes or bad tfctl update | init respawns; two fast crashes after an update put `tfctl.prev` back |
| Bad system update | U-Boot tries count down to the other slot; a slot that never reaches the server is rebooted after 10 min |
| /data corrupt | `e2fsck -p`, then `-y`; still not mounting and either no ext4 superblock or errors left uncorrected: formatted fresh with the shipped config, frame starts like new. Never on a read error, never on another disk |
| Root slot full | Updates are atomic renames; RESBLKS=0 frees the reserved 5 % in the next image (the 190 MB image gate stays) |
| Display memory | CMA 64 → 128 MB (`cma=128M`) in the next image: 4 screen buffers of 4 MB plus WebKit's left 6 MB free and `eglSwapBuffers` failed with EGL_BAD_ALLOC during page changes [V 2026-10-26] |
| Everything above fails | Pull the card (card system) / insert the rescue card (eMMC system, see below); USB-C link 192.168.7.2 with SSH |

Not done yet: a recovery screen drawn by tfctl itself when cog cannot start at all (today the
splash stays up), reading the power key, and a serial login on the USB console.

### Running from the eMMC (TASKS section E, 2026-10-26)
Owner: "Prepare the system to become the main OS and no SD. Once you are comfortable that it's
all solid and recoverable, do it." The card the frame is installed from stays the rescue card.

**Why a card always wins.** The A33 boot ROM tries the card before the eMMC [D, ormarch E017];
this frame booted its card for weeks while Android's boot0 sat on the eMMC [V]. So with a
thinframe card inserted the eMMC is never started, whatever it holds.

**The eMMC** [V 2026-10-26, `/sys/block/mmcblk2`, debugfs `ios` and `ext_csd`]: "016G92",
manfid 0x11, 10/2014, 30,777,344 sectors (15,758,000,128 bytes), 8-bit DDR52 under Linux with no
errors over two full reads. The vendor set EXT_CSD PARTITION_CONFIG 0x48 (boot from boot0,
BOOT_ACK on) and BOOT_BUS_CONDITIONS 0x01, and boot0 holds a copy of Android's boot0; boot1 is
empty. U-Boot's sunxi SPL says the boot ROM tries such a boot partition before the user area.

**Backup** [V]: `sha256sum /dev/mmcblk2` on the frame (read 1), then `zstd -1` streamed to the
Mac and hashed after decompression (read 2): equal (`ad866fb8c6d8…`), 549 MB compressed, with
boot0/boot1 dumps. Copies on the Mac (`~/Documents/scratch/photo-frame-dashboard/backups/
pf1007-emmc-full/`) and on Nox (`~/docker/apps/thinframe/backups/office-emmc-2026-10-26/`),
mode 600, outside Git (the image can hold account data). The 2 GiB dump of Oct 18 differs in
12 of 32 chunks, from Android's boot partition on: Android changed after it. `tfctl emmc
backup` (raw to stdout, hash to stderr and `/data/thinframe/emmc-backup`) does it in one pass.

**Which disk is which.** The eMMC gets the card's layout with its own MBR signature `50463045`
("PF0E"), so its PARTUUIDs never clash with a card's. `slot::find_disk` follows the signature of
the running root: booted from the card it never returns the eMMC (as before, sysfs
`device/type` MMC is skipped); booted from the eMMC it returns only the eMMC. Every writer
(`tfctl boot`, update, sysupdate) goes through it. Kernel aliases keep the card at mmcblk0 and
the eMMC at mmcblk2.

**Boot chain.** U-Boot build E (`image/uboot-emmc.sh`) is ormarch's build L from the same tree
and container (build L reproduces the card's bootloader byte for byte, `d12cb373…` [V]) with
`MMC_SUNXI_SLOT_EXTRA=2`, the eMMC node on in its DTS, and a boot command that loads `boot.scr`
from `mmc ${mmc_bootdev}:1` (`board/sunxi/board.c`: 0 = card, 1 = eMMC; its DT aliases `mmc1`
to the eMMC). The script stops if the config differs from build L in anything else (the
fastboot default device follows the slot; fastboot never runs). One `boot.scr` serves both:
every `mmc 0:1` reads `${pf_dev}`, which is `mmc_bootdev` or 0. The kernel DTBs `full` and
`pmic` have the eMMC on; `min` stays card-only.

**Install** (`tfctl emmc install --backup-sha256 HEX`): only from the card, the eMMC opened
`O_EXCL`, the running slot confirmed, HEX equal to the recorded backup. Order: clear the vendor
eGON at 8 KiB; PARTITION_CONFIG to 0 (read back) and boot0's header cleared, so the boot ROM
reads the user area as on any A33 with a fresh eMMC; the MBR; p1 to p3 copied from the card and
read back by hash with the page cache dropped; on p1 the extlinux roots move to `50463045`,
`boot.scr` and the DTBs come from the image, uEnv names the running slot with tries 0;
`mkfs.ext4` p4 and a copy of /data; U-Boot last, read back. Until that last write the eMMC has
no boot code, so a cut or an error midway leaves a frame that starts from the card (or FEL),
never a half system.

| Fails | Way back |
|---|---|
| eMMC system does not start | Insert the rescue card and power on: the card wins |
| eMMC system broken later | Rescue card, then `tfctl emmc install` again (no backup needed over thinframe) |
| Rescue card lost | Flash a new one on the Mac with `scripts/flash.sh` |
| No card works | FEL over USB-C from the Mac (`sunxi-fel`, ormarch docs) |
| Back to Android | From the card: the image to `/dev/mmcblk2`, `boot0.bin` to `mmcblk2boot0`, PARTITION_CONFIG 0x48 [U: not tested, it needs a second frame] |
