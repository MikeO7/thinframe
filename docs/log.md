# Log

Dated evidence for every gate in TASKS.md. Newest at the bottom.

## 2026-10-18
- Repo stubbed. Nothing built, nothing booted. All hardware facts are [D] from ormarch.

## 2026-10-18 (later)
- Alpine 3.24/edge: `apk search cog` and `wpewebkit*` empty on armv7 and aarch64 (pkgs.alpinelinux.org). Base switched to Buildroot 2026.08.
- frameserver (Go): `go test ./...` green incl. WebSocket push/ack and MCP SDK client end-to-end; scratch image 10.9 MB.
- tfctl: 32 unit tests, clippy clean; armv7 musl static 576,812 B.
- Mac end-to-end: frameserver + `tfctl agent` over WebSocket: online in /api/frames; PUT rotation persisted to the conf; `screen off` acked ok; `update-system` acked with the expected not-implemented error.
- Sim (Debian trixie cog 0.18.4 headless + WPEWebDriver 2.48.3 + real tfctl): `/frame/sim` rendered by WPE; layout PUT changed `sim/out/frame.png`; rotation 0 → 375x600, rotation 3 → 800x500. cog headless is hardcoded 800x600 (cog-platform-headless.c), so the page renders at panel size via `?sim=WxH` and scales to fit; the driver crops.
- Scaffolded (stubs, nothing built [U]): `image/buildroot/` BR2_EXTERNAL tree (external.desc/mk, Config.in, `pf1007l_defconfig`, genimage.cfg with MBR sig 0x50463037 and A/B/data layout, boot.cmd slot/tries logic, post-image.sh stub, packages `tfctl` and `aic8800`), `image/Dockerfile.br`. `image/build.sh` is now Buildroot-based; `build.sh check` lints the tree and runs in CI. TASK 1/2/3 gates remain open: the aic8800 pin, ormarch commit, kernel config, post-image and boot.scr compile need a Docker host.
- `tests/e2e-native.sh` (real frameserver + real `tfctl agent`, no Docker): frame registers, rotation PUT persisted to the agent's conf, `screen` action acked, second server with `make-artifacts.sh` output serves `tfctl-armv7.sha256` matching `sha256sum` and puts `software_version` in config. Passes locally; wired into CI as job `e2e`.
- `server/internal/artifacts` now has tests (versions, sha256 sidecar, traversal/hidden refusal). Fixed the sha cache key (was `string(rune(size))`).
- `scripts/flash.sh` implemented for macOS and Linux (release download + sha256, guarded card discovery, typed confirmation, one sudo write with readback, provision.txt). `tests/flash-test.sh` passes (size guard, provision.txt, discovery via fake lsblk). The write/mount path has not run against a real card [U].
- release.yml: tag builds now stage artifacts into the server image; the image/release-asset job is present but disabled (`if: false`) until TASK 2/3 produce an image.
- Closed PRs #19–#21 per owner; their commits live on in this branch.
- tfctl: `statuspage.rs` writes `/var/lib/thinframe/www/status.html` (reason + IP) when the agent falls back; default `FALLBACK_URL` now points at it. 61 unit tests, clippy clean. Not seen on a panel [U].
- tfctl `png.rs`: own inflate (stored/fixed/dynamic) + PNG decoder (8-bit grey/GA/RGB/RGBA, filters 0-4), bomb guard, splash blits `/etc/thinframe/splash.png` with alpha in every rotation. 67 unit tests incl. a real dynamic-Huffman block and a filtered RGBA PNG made with zlib; clippy clean. Host (x86_64) release binary: 603,208 B, under the 1 MB budget. Not drawn on a panel [U].
- server: calendar adapter (`internal/api/calendar.go`, ICS parse/unfold/all-day/UTC, 10 min cache) and `calendar` widget pushed over SSE like weather; tests for parsing, selection and rendering. RRULE recurrence not expanded [U]. All server tests pass on Go 1.27.

## 2026-10-20
- TASK 0: Buildroot pinned to the LTS `2025.02.18` (buildroot.org lists 2025.02.18 LTS, 2026.05.3 old
  stable, 2026.08 stable on 2026-10-20), tarball sha256 `e38ad1df…5321`. Package versions in that
  tree [V]: WPE WebKit 2.50.5, cog 0.18.4, libwpe 1.16.3, wpebackend-fdo 1.14.3, mesa 24.0.9 (2026.08
  has the same WPE; cog 0.18.5, mesa 26.1.8). Kernel stays 7.2.10 (kernel.org tarball sha256
  `2feaa704…ebf3`, matches ormarch). ormarch pinned to `4207b5c2f6d6615bf6a18fb725cc7a77dd532065`.
- TASK 1: `image/build.sh inputs` ran twice on the Mac (Podman, arm64, no armv7 emulation needed):
  U-Boot build L 536,544 B, SPL ends 0x450a (limit 0x5e00), ormarch config checks OK; QEMU U-Boot
  576,128 B. `diff` of the two `image/out/inputs.sha256` files: identical (U-Boot honours
  SOURCE_DATE_EPOCH). `u-boot-pf1007l-L.bin` sha256 `d12cb373…c1c2`.
- S5c/S6 server audit. Already on main: `hello` reply with `time_unix`/`tz`/`server_version`, open-meteo and ICS
  adapters pushed over SSE, artifacts handler + sidecars, `software_version` in config. Added: `GET /api/data/weather`
  and `GET /api/data/calendar` (404 when unconfigured), SSE re-push every 10 min (was on connect only), open-meteo
  `BaseURL` + HTTP status check + 1 MB body limit, stale reading kept on fetch failure, negative temps rounded
  correctly. New `internal/api/data_test.go` (httptest fakes, no network): open-meteo query/cache/error paths, ICS
  fetch/cache/404, data endpoints, SSE push + refresh, `hello` `tz`=`$TZ` and `time_unix` within 2 s, config
  `software_version`/`system_version`/`artifacts` sums and token-gated `/api/artifacts/` file + sidecar.
  `cd server && gofmt -l .` → empty; `go vet ./...` → clean; `go test -count=1 -race ./...` → 6 packages ok
  (15 tests in internal/api); new tests passed `-count=20 -race`. `server/static/index.html` still loads
  picsum.photos but no route serves it [V, read in server.go]; left in place.
- TASK 2: first full Buildroot build on the Mac (Podman VM, 8 CPU, 16 GB): 65 min warm-start after
  the toolchain; WPE WebKit 2.50.5 OOM-killed cc1plus at 8 jobs, passes at 5 (`br-run.sh` now runs one
  job per 2.8 GB of RAM). `rootfs.ext4` 199,229,440 B (190 MiB ext4, about 180 MiB used, 174 MiB of
  files; largest: libWPEWebKit 65 MiB, libicudata 31 MiB) [V]. tfctl armv7 654,940 B.
- TASK 3: `image/build.sh assemble`: self-check 11/11 PASS (MBR 0x50463037, U-Boot L byte-equal at
  8 KiB, eGON header, p1 FAT32 PFBOOT 256 MiB at 4 MiB, 1 GiB rootA/rootB, ext4 magic, data label).
  The first run caught p1 formatted FAT16 under an 0x0c entry; now `mkfs.fat -F 32 -s 4 -h 8192`
  exactly as ormarch's assemble. `thinframe-sd.img` sha256 `91be0cab…ad6a` [V].
- TASK 2/3 QEMU gate (`image/build.sh test`, orangepi-pc, QEMU U-Boot at 8 KiB of a copy, kernel
  built here): 14/14 PASS, login prompt in 20 to 25 s under TCG; our boot.scr ran (slot a, tries 0);
  dropbear key login as `frame`; 7.2.10; root ro ext4; /boot vfat ro; /data grew to 1788 MiB of a
  4 GiB card; provision.txt applied and deleted; hostname = frame id; `tfctl kiosk` under init;
  frame registered with a frameserver on the Mac; no syslogd/klogd/crond. 28 MiB used of 1002 (no
  cog: the QEMU board has no panel) [V in QEMU; the frame itself is TASK 4]. Fixed on the way: config
  writes failed with EROFS (rename next to a file bind mount), so `/etc/thinframe.conf` is now a
  symlink into /data; BusyBox `S50crond` and `S01seedrng` removed; the test used `pgrep`, which this
  BusyBox lacks. Output: image/out/qemu-test.txt.
- TASK 12 rollback gate (`image/build.sh abtest`), server-driven only: A (slot a) → B written to slot
  b, `tries left 2`, `frame:slot-confirmed` → BAD (no /sbin/init, /bin/sh) written to slot a: kernel
  `No working init found` twice (`tries left 2`, `1`), panic=10 reboot, U-Boot flipped to b (`slot b,
  tries left 0`), B reports from b again, agent emits `frame:system-update-skipped` and does not
  re-flash BAD → C written to slot a and reports from a. 10/10 PASS [V in QEMU]. Output:
  image/out/qemu-ab-test.txt.
- M4: `tests/e2e.sh` (compose project `thinframe-e2e`, port 18088, temp data/out dirs): sim frame
  registers; screenshot at rotation 0 is 375x600; MCP `tools/call set_frame_config {"rotation":3}`
  returns rotation 3; the next screenshot is 800x500 with a different sha256. `e2e: ok` on the Mac
  (Podman) [V]. compose.yml takes `PORT`, `DATA_DIR`, `SIM_OUT` (defaults unchanged).
- TASK 14/15 status page and splash fields: the kiosk writes `/run/thinframe/status.html` before cog
  starts and opens cog on it; headline "joining <ssid>" / "no Wi-Fi configured" / "reaching <server>"
  / "cannot reach <server>" plus IP, refreshed on every failed attempt; the first server answer
  switches to the dashboard. Server: `splash_text`, `splash_bg` (#rrggbb), `splash_png` (base64 PNG
  up to 48 KiB) over REST and MCP `set_frame_config`; tfctl stores the logo at
  `/data/thinframe/splash.png` (wins over `/etc/thinframe/splash.png`) and SPLASH_* in the conf; the
  status page shows the logo. QEMU boot test now 16/16: status page "no Wi-Fi configured" (no
  profile, before DHCP) with the frame id; a PUT of splash_text + splash_png reached
  `/etc/thinframe.conf` and `/data/thinframe/splash.png` [V in QEMU]. On the panel: not seen [U].
- Found on the way: `/run/thinframe` was 0700 root (now 0755; status page is not secret). iwd profile
  names were escaped per byte (`My=20Net.psk`); iwd 3.0 `storage_get_network_file_path` keeps
  `[A-Za-z0-9 _-]` names as they are and hex-encodes the whole SSID otherwise (`=436166c3a9.psk`) [V,
  read in build/iwd-3.0/src/storage.c]. Fixed with round-trip tests. tfctl armv7 666,500 B.
- TASK 2 DEBUG: `DEBUG=1 image/build.sh rootfs` builds `board/pf1007l/debug.fragment` in
  `/br/output-debug` (Buildroot never uninstalls, so release builds stay clean) with version
  `…-debug`. Image `2026.10.20-e4518e92-debug`: evtest 33,928 B, strace 6.13; QEMU boot test 16/16;
  rootfs.ext4 still 199,229,440 B [V in QEMU]. i2c-tools was dropped by olddefconfig (needs
  BUSYBOX_SHOW_OTHERS); BusyBox's i2c* applets cover it, and br-run.sh now stops when a fragment
  option is dropped (tested by re-adding it). WebKit OOM-killed cc1plus again at 5 jobs
  (GeneratedSerializers.cpp); the job budget is now 4.5 GB each, 3 jobs on 16 GB.
- TASK 9 size audit (QEMU, image `2026.10.20-a9b1c71c`): target tree 169 MiB (was 174); largest
  libWPEWebKit 65 MiB, libicudata 31 MiB, mesa megadriver 11 MiB (33 hard links, one copy), DejaVu
  Sans + Mono (Serif and Condensed dropped: pages use sans-serif and monospace only). ext4 free
  14.9 MiB of 190. Idle `ps` (kernel threads aside): init, udevd, dbus-daemon, iwd, dhcpcd (+3
  privsep helpers), dropbear, udhcpd, getty, tfctl, WPENetworkProcess: the README list plus dbus,
  which iwd needs. 26 MiB used of 1002 at the login prompt [V in QEMU].
- TASK 9 `tfctl status`: IPs from /proc/net/fib_trie (no `ip` spawn; only iwctl is spawned), USB-C
  address sorted last so the heartbeat and status page report the LAN address. QEMU 17/17 incl.
  `tfctl status` → `ip : 10.0.2.15 192.168.7.2` [V in QEMU].
- TASK 9 hung page: the dashboard sends `POST /api/frames/<id>/alive` every 30 s from a JS timer
  (stops when the WebProcess hangs; cog's network process keeps the SSE socket open, so the socket
  is no signal). The server checks every 30 s per connected frame: a page that pinged during this
  agent connection and is quiet for PAGE_STALE (default 3 min) gets one `restart-kiosk` and a
  `page` event; another needs a fresh ping. `TestHungPageRestartsCogOnce` (-race, 5 runs) and
  tests/e2e.sh with PAGE_STALE=20s: no restart over 30 s of a live page, then `kill -STOP` on the
  sim's WPEWebProcess → "no ping for 2xs, restarting cog" [V in the sim container].

## 2026-10-20 release CI: ccache survives a timed-out build
- Release run 39392591191 (workflow_dispatch) was still in "build" after 4 h 20 min, against a
  340 min job limit. A cold WebKit build on the 4-core runner does not fit [V, run log].
- The release job now restores and saves `image/cache/ccache` (THINFRAME_CCACHE, bind-mounted as
  /ccache) with `if: always()`. The build step has a 320 min limit so the save still runs inside the
  355 min job. A timed-out run therefore leaves a warm cache for the next dispatch [U until a second run passes].
## 2026-10-20 M2 partial: the real image under `qemu-system-arm -M virt` (sim/qemu/run.sh)
- `QEMU=1 image/build.sh rootfs` builds the variant (qemu.fragment: Mesa swrast + linux-qemu.fragment
  with ARCH_VIRT, virtio-blk/net/gpu/input, PL011, PL031) into image/out/qemu/, own output tree.
- `SERVER_URL=http://10.0.2.2:18090 FRAME_TOKEN=m2-token sim/qemu/run.sh`: kernel 7.2.10 boots,
  `[drm] Initialized virtio_gpu 0.1.0 … on minor 0`, `tfctl: provision: 3 keys`, frame `sim-qemu`
  appears in `/api/frames` (first_seen 12:22:18Z) [V]. Needs `-global virtio-mmio.force-legacy=false`:
  on the legacy transport virtio-gpu and virtio-input never bind [V].
- cog loads the dashboard ("Loaded successfully") and then dies with SIGSEGV every time with
  `renderer=gles`, at rotation 3 and at rotation 0 [V]. With `renderer=modeset` it runs but
  warns "does not support rotation 3", and page flips fail with "Permission denied" [V]. The screendump
  stays "Display output is not active". QEMU here (Homebrew) has no virgl, so virtio-gpu only offers
  kms_swrast on card0. The likely cause is WPE's buffer export with no real render node [U]. This tells
  us nothing about lima, which has its own render node.
- Because of this, cog's renderer is now a setting: `COG_RENDERER` in thinframe.conf, default `gles`
  (the only one that rotates, cog 0.18.4 source [V]). `modeset` is the escape hatch if gles fails on
  the panel [U, check at TASK 6].
- M2 stays open: the screendump does not show the dashboard yet, and the A/B update inside virt is
  not run (it passes on `-M orangepi-pc`, see TASK 12).

## 2026-10-20 first full release run (39392591191, workflow_dispatch on main)
- `image` passed in about 5 h on ubuntu-latest: Buildroot build plus the QEMU boot test of the release image [V].
  `server-image` pushed ghcr.io/mikeo7/thinframe-server:v2026.10.20 and :latest [V].
- `publish` fails at `gh release create` with "HTTP 403: Resource not accessible by integration",
  although the job log shows `Contents: write`. A re-run gave the same 403, and the repo has no rulesets [V].
  The cause is not known: a repo or account setting, or the token creating the tag [U]. Owner to check.
  Until then, S8/S10 stay unchecked. Artifacts are on the run (`image`, 90 days).
- Cause of the 403 [V by contrast]: a probe workflow (draft PR #39, closed unmerged, cleaned up)
  created a draft release, a raw tag and a full release on the current main commit with the same
  token, and all three worked. The failing run built 3b35ae1, and by publish time main had changed
  ci.yml and release.yml (#35). GITHUB_TOKEN cannot create a ref on a commit whose workflow files
  differ from main (no `workflows` permission). Fix: a `tag` job creates the tag first, while
  GITHUB_SHA is still the tip of main; publish only creates the release on the existing tag.
  A tag made with GITHUB_TOKEN does not trigger the `push: tags` workflow, so no second run starts.
- 2026-10-20, later: release run 39493723923 on the #40 workflow. The `tag` job created v2026.10.20
  at 8d949b1 [V]. `image` then failed at once: the #40 edit had left the old version script glued
  onto `uses: actions/checkout@v7` (a YAML continuation line). actionlint does not flag that. Fixed,
  and CI's shell job now fails on any text continuing a `uses:` line (tested against the broken file).
  The tag stays at 8d949b1; the next run reuses it, since that commit differs only in workflows.

## 2026-10-20 M2: dashboard on virtio-gpu (two real bugs fixed)
- Debug aids in qemu.fragment (sim only, never shipped): gdb, unstripped binaries, 400M rootfs and
  a passwordless root login on the serial console (`nc -U sim/qemu/work/console.sock`).
- Bug 1, cog [V]: under gdb, cog's SIGSEGV is `pc 0x0` called via libffi from libwayland-server.
  `WAYLAND_DEBUG=server` shows the last requests as wl_shm `create_buffer` 1280x800, `attach`, `commit`:
  the web process renders in software and sends wl_shm buffers. cog 0.18.4's gles renderer sets
  only `export_fdo_egl_image`, so wpebackend-fdo 1.14.3 calls the NULL `export_shm_buffer`.
  Fix: image/buildroot/patches/cog/0001 (BR2_GLOBAL_PATCH_DIR, main image too). It uploads the shm
  pixels to the renderer's texture and draws them with the same rotation, and checks
  gbm_surface_lock_front_buffer() for NULL. On the frame this path is only taken if WebKit falls
  back to software [U]; without the patch that fallback would be a crash loop.
- Bug 2, tfctl [V]: cog then ran but logged "Cannot set mode (Permission denied)". DRM debugfs
  `clients` showed cog with master=n. cog, WPEWebProcess and WPENetworkProcess were all in tfctl's
  process group, so after a restart the old web process outlived cog while holding a dup of cog's
  DRM file (and with it master). The new cog opened the device without master. Fix: cog
  gets its own process group; on restart or exit the kiosk SIGKILLs the group and waits (max 5 s)
  until it is empty. Test `reap_group_kills_helpers_that_outlive_the_child`. The same race applies
  on the frame after every URL or rotation change [U].
- Result: screendump shows the dashboard (clock, date, weather and status "sim-qemu · 10.0.2.15"),
  rotated 270° as on the mounted frame: docs/img/m2-qemu-virt-dashboard.png [V].
- Also fixed: br-run.sh's `>>>` filter returned 1 for quiet targets (BRTARGET=cog-dirclean) and
  `set -e` ended the build silently.
- Still open for M2: the A/B update + rollback inside `-M virt`. This harness boots the kernel
  directly (no U-Boot, so no boot.scr slot logic). The same test passes on `-M orangepi-pc` (TASK 12).

## 2026-10-20 TASK 15 in QEMU: retro splash, status page, hand-off to cog
- `tfctl splash` gained SPLASH_STYLE=retro (default; `plain` keeps the old look): tfctl/src/retro.rs.
  `tfctl splash --png FILE [STATUS]` writes the picture without a display.
- Boot run under `-M virt` with a screendump every 2 s and the server down for the first 2 minutes
  (`KEEP_SHOTS=1 SHOT_INTERVAL_S=2 sim/qemu/run.sh`, frameserver started at T+126 s). 17 distinct frames:
  splash "starting" (T+17 s), status page "cannot reach http://10.0.2.2:18090" with frame id and IP on the
  splash backdrop (T+46..128 s, only the cursor changes across reloads), splash "loading" (T+142 s),
  dashboard (T+229 s): docs/img/boot-qemu-{1-splash,2-status,3-dashboard}.png [V] (QEMU, not the panel).
- Fixed on the way, each seen in the previous run's frames [V]:
  - The status page showed its HTML source: WebKit takes `file://` types from the shared-mime-info
    globs, which the image lacked. image/rootfs/usr/share/mime/globs2 lists the few types we use.
  - The splash went black before cog painted: the kiosk now keeps the DRM file open, drops master
    (DRM_IOCTL_DROP_MASTER) so cog can set its mode, and closes it once the CRTC shows another framebuffer.
  - A white flash before the first page: cog gets `--bg-color=#0b0d10`.
  - The splash was 180° off from cog: `Canvas::rotate` cases 1 and 3 swapped (cog rotation 3 puts the
    picture's top at the panel's right edge).
  - The status page flashed without its backdrop on every reload: `status.html` is now a still shell
    (backdrop drawn once to /run/thinframe/backdrop.png) with an iframe `status-line.html` that reloads.
- Still visible: about 2 s of the dark cog background before the first paint (T+36, T+159 s), and about 6 s
  of "Display output is not active" while the kiosk restarts cog (T+136 s): the splash can only take the
  CRTC back after the old cog has exited.
- `SHOW=1 sim/qemu/run.sh` opens the guest in a window on the Mac (cocoa); a click is a touch.

## 2026-10-20 TASK 16 one-press pairing and the seventies splash, end to end in QEMU
- Setup: frameserver with `FRAME_TOKEN=admin-secret` (auth on), fresh store; guest flashed with
  `FRAME_TOKEN=` (empty) by `sim/qemu/run.sh`. Taps go through a virtio multitouch screen (new in the
  harness) at the panel pixel where the button is drawn.
- Boot: provision wrote SERVER_URL and FRAME_ID only; the agent sent `POST /api/pair` 24 s after the
  kernel started and cog showed the PAIR page (docs/img/pair-qemu-1-button.jpg) [V].
- One tap at panel (256,252): server log `POST /pair/sim-qemu/accept`, then the agent's poll took the
  token, stored FRAME_TOKEN in /data/etc/thinframe.conf, connected the control socket with it and opened
  the dashboard (splash "tuning in to the dashboard ... at 10.0.2.15" in between):
  docs/img/pair-qemu-2-splash.jpg, docs/img/pair-qemu-3-dashboard.png [V].
- Reboot (`reboot` on the serial console): back on the dashboard with no new `POST /api/pair`; the
  socket reconnected with the stored token (frame events: ws connected 23:47:32) [V].
- A second `POST /api/pair` for sim-qemu answers 409 [V].
- "forget pairing" (`POST /admin/sim-qemu/unpair`): the server closed the socket, the reconnect got 401,
  the agent cleared its token and the PAIR page was back 3 s later; one more tap paired it again and
  the dashboard returned. Events: paired, forgotten, paired [V].
- Found on the way, both fixed:
  - cog ignores mouse clicks unless it draws a cursor, so the harness's virtio tablet could never press
    anything. The harness now has a virtio multitouch device; `run.sh tap` sends one finger (QMP `mtt`,
    release with tracking id -1). Touch is also what the frame's panel is.
  - cog scaled touch points to the rotated size but did not turn them: with rotation 3 a tap landed
    elsewhere. image/buildroot/patches/cog/0002 maps the physical point into the page the same way the
    renderer turns the picture. Verified by the tap on the drawn button above [V in QEMU]; the frame's
    touch controller may need its own axis check in TASK 6 [U].
  - `SHOW=1` with a cocoa window resized the guest's screen to the window (296x480). It now opens
    Screen Sharing on a localhost VNC server, which leaves the guest at 800x1280; the kernel command
    line also pins `video=Virtual-1:800x1280`.
- Seventies splash cost: `tfctl splash --png` (scene build, one frame, rotate, PNG encode) took 8.2 s
  under TCG; the same on the Mac takes 41 ms to build the scene and 5.7 ms per animation frame (release
  profile). The A33 figures are not measured yet [U]; the kiosk logs "20 frames, N ms each" once a
  splash has animated that long.

## 2026-10-21 TASK 17 Wi-Fi first, then pairing, end to end in QEMU
- Found first: the image had **no iwctl**. Buildroot builds iwd's client only with readline, so
  `tfctl wifi`, the RSSI in `tfctl status` and any on-screen setup could never have worked on the frame.
  pf1007l_defconfig now sets `BR2_PACKAGE_READLINE=y`; /usr/bin/iwctl is 148 KB [V in the QEMU build].
  Also, Buildroot 2025.02's S40iwd has no `restart`, so `tfctl wifi` now runs stop + start [V source].
- Setup: QEMU image with mac80211_hwsim (three radios), hostapd serving "Groovy Wifi" (WPA2,
  "far out man") and "Cafe" (open), udhcpd on each (qemu-overlay, S35simwifi). Serial log:
  `wlan1: AP-ENABLED`, `wlan2: AP-ENABLED` [V]. frameserver with auth on, fresh store; card flashed with
  neither Wi-Fi nor token.
- Real iwctl 3.0 output in the guest (`get-networks rssi-dbms`): colour escapes, rows like
  `      Groovy Wifi                       psk                 -3000   `. tfctl's parser matches it [V].
- Boot: cog opened `http://127.0.0.1:8091/wifi`; both networks listed, Groovy Wifi with a lock
  (docs/img/wifi-qemu-1-networks.jpg) [V]. No pairing request reached the server yet [V].
- Tap on Groovy Wifi: password page with the on-screen keyboard. Typed a wrong password, Join:
  iwd/hostapd refused it and the page came back to the keyboard with "That didn't work (Operation
  failed). Check the password." (docs/img/wifi-qemu-2-wrong-password.jpg) [V].
- Typed "far out man" (docs/img/wifi-qemu-3-password.jpg), Join: wlan0 `State connected`,
  `Connected network Groovy Wifi`; iwd wrote `/var/lib/iwd/Groovy Wifi.psk` (on /data) [V].
  Only then did the server see `POST /api/pair` (19:17:38) and the PAIR page appear, step marker
  "1 Wi-Fi · 2 pair" (docs/img/wifi-qemu-4-pair.jpg) [V].
- Tap PAIR: `POST /pair/sim-qemu/accept` 19:18:50, dashboard `GET /frame/sim-qemu` 19:19:06 [V].
- `reboot`: no Wi-Fi page and no pairing request; iwd rejoined Groovy Wifi by itself, the socket came
  back and the dashboard loaded (`GET /frame/sim-qemu` 19:24:50,
  docs/img/wifi-qemu-5-dashboard-after-reboot.jpg) [V].
- Seen on the way: under TCG a tap shows on screen seconds later (software GL), so taps queue up.
  libinput in the guest reports each tap at once and in the right place (`libinput debug-events`) [V].
  So the list page now reloads every 20 s once networks are shown (3 s while still looking), and the
  PAIR page's step marker got the same pill as the Wi-Fi page. Both changed after this run (unit
  tests only, not re-run in QEMU).
