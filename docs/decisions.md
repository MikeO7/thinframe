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

If `apk info -L cog` has no drm platform on armv7, the fallback is `cage` + `cog --platform=wl`
and the service file already has the lines to swap in.

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
Kernel console is `ttyGS0` (USB-C CDC ACM, built in with CDC ECM `usb0` = 192.168.7.2); the panel
never shows console text (`FRAMEBUFFER_CONSOLE` off).

## SSH key only, user `frame`, no sudo at all
Dropbear, key-only. tfctl runs as root for DRM, backlight, clock and updates; there is nothing else to escalate to. Host key generated once into `image/secrets/` so re-flashing a card
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
"Patched" means the kernel and every Alpine package, not only tfctl. The frame never runs `apk`:
the image is immutable and replaced whole. Card layout grows from 2 to 4 partitions:

| Part | Content | Why |
|---|---|---|
| p1 FAT32 `PFBOOT` | U-Boot script, `extlinux.conf`, `uEnv.txt`, `zImage.a` `zImage.b`, DTBs | U-Boot picks the slot from `uEnv.txt` (`slot=a|b`, `tries=N`) |
| p2 ext4 `rootA` | Buildroot root, read-only mount | active or standby |
| p3 ext4 `rootB` | Buildroot root, read-only mount | standby or active |
| p4 ext4 `data` | `/etc/thinframe.conf`, ssh host keys, iwd profiles, logs, www fallback | survives updates; linked or bind-mounted into the read-only root |

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
| `qemu` | the real SD image under `qemu-system-arm` (orangepi-pc board, serial console, user-mode net) | ~1 min boot | OpenRC, networking, A/B system update and rollback |

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
a verified eMMC backup is a hard prerequisite so the frame can be returned to stock.

## One-press pairing
Typing a server token into the flash script was the hardest step of onboarding. Now the token is
optional: a frame without one asks the server to pair (`POST /api/pair` with a random nonce only it
knows) and shows a PAIR button on its screen. The button posts that nonce from cog straight to the
server, which proves someone is at the frame; the server then mints a 256-bit per-frame token,
stores only its SHA-256, and hands the token once to the frame's next poll. A paired id refuses new
requests until the admin page forgets it, so pairing happens once per frame. The per-frame token
opens only that frame's own routes (config GET, heartbeat, control socket) plus release info and
update artifacts; listing, layout, actions and MCP keep needing `FRAME_TOKEN`. A 401 later clears
the frame's token and brings the button back. Discovery of the server is not automatic: the LAN
broadcast or mDNS a container would need does not cross Docker's bridge network or QEMU's user
network, so `SERVER_URL` still comes from the flash script. [V] in QEMU, see docs/log.md.

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
