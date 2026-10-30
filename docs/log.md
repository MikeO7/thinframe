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

## 2026-10-21 TASK 17 re-run: saved networks, the late tweaks
- Image rebuilt from main (61e9314); fresh frameserver with auth on; no token.
- **Run A: provision.txt with "Groovy Wifi" and a wrong password.** iwd stayed `State connecting`.
  After the 45 s grace cog showed the setup page with "Can't reach Groovy Wifi. Pick a network:"
  (docs/img/wifi-qemu-7-cant-reach.jpg) [V]. Tapped Groovy Wifi and typed "far out man", then Join.
  The join forgot the stale profile first: `Groovy Wifi.psk` no longer holds `wrongpass1`. The station
  joined, and only then came `POST /api/pair` (20:15:06) and the PAIR page. Its step marker now has the
  pill and reads clearly over the stripes (docs/img/wifi-qemu-6-pair-pill.jpg) [V].
- **Run B: provision.txt with the right password.** Splash "warming up", then straight to PAIR
  (`POST /api/pair` 20:16:55). The Wi-Fi page never showed: iwd joined inside the grace period [V].
- The list page's 20 s reload (set after the first run) was on screen through Run A. The tap on a
  network row went through, painted about 30 s later under TCG [V].

## 2026-10-21 M2 done: A/B system update and rollback inside `-M virt`
- The QEMU=1 build now also makes U-Boot 2025.01 `qemu_arm` (qemu.fragment, uboot-qemu.fragment:
  FAT_WRITE, and a boot command that sources /boot.scr like the frame's build L). Without that boot
  command, standard boot took /extlinux/extlinux.conf and skipped the slot logic [V, serial log].
- `BOARD=virt image/qemu-ab-test.sh` (image/qemu-lib.sh). The card is the QEMU=1 image. boot.scr is the
  same `boot.cmd` + `boot.cmd.patch` with `mmc 0:1` read as `virtio 0:1`. The slot configs drop the A33
  DTB (QEMU passes its own) and use ttyAMA0. provision.txt joins the sim's "Groovy Wifi", because the image
  has a radio and Wi-Fi comes first (TASK 17). The first try sat on the Wi-Fi page, which is correct.
- Bug found and fixed: tfctl looked for the card only at /dev/mmcblk0..2, so under virt (/dev/vda)
  the system update could not find its disk ("grow data: no thinframe disk").
  - `slot::DISKS` now adds /dev/vda. The MBR signature guard is unchanged, so only our card is ever
    written.
  - `slot::part_n` names partitions on both kinds of disk. PFBOOT's remount used `<disk>p1`, which was
    wrong for vda.
- Result, all from server config changes, no manual step [V]:
  ```
  PASS  boot 1: A on slot a reports to the server
  PASS  B written to slot b, rebooted, B reports from slot b
  PASS  U-Boot booted b with tries pending
  PASS  slot b confirmed by the agent (event)
  PASS  BAD written to slot a and the board rebooted into it
  PASS  BAD kernel panics (no init)
  PASS  U-Boot spent the last try and flipped back to slot b
  PASS  rolled back: B reports from slot b again
  PASS  agent refuses to flash BAD again (event)
  PASS  C written to slot a, rebooted, C reports from slot a
  qemu-ab-test: 2026.10.21-7845dfb8-qemu -> -b -> -bad (rolled back) -> -c passed
  ```

## 2026-10-21 Touch firmware for release images; TASK 8 status
- Gap found while reviewing the release: `post-build.sh` warns "no touch firmware staged; touch will not
  work on this image" for every CI image [V, release log]. A frame flashed from a release could not be
  tapped, so the Wi-Fi page and PAIR were unusable. Building with `PF1007L_TOUCH_FIRMWARE` was the only
  way to get touch.
- Fix: `tfctl boot` step "touch firmware" (tfctl/src/touchfw.rs, docs/decisions.md). When the image
  has no `/lib/firmware/silead/gsl_m86_hd.fw` and a silead device is unbound, it reads the table
  read-only from the frame's own eMMC Android boot image, checks the SHA-256, caches it on /data, sets
  firmware_class.path and re-probes. QEMU has no silead device, so the step does nothing there.
- Evidence so far: `cargo test` 104 passed. `touchfw::tests` runs the whole chain on a synthetic
  disk: boot image header v0, then gzip with stored blocks and a file name, then newc cpio, then an
  ELF32 with `.rel.data` R_ARM_ABS32 pointers into `.rodata`, then the table. A wrong hash is
  refused. The arithmetic for E025's real header (kernel 10,221,784 bytes, page 2048) puts the ramdisk at
  page 4,993. armv7 binary 863,004 bytes. **On the frame: not yet.** TASKS section 6 has the check.
- TASK 8 (`scripts/deploy.sh`) cannot pass as written, and its gate stays open. The script predates
  the image this repo now builds:
  - it uses `doas` and `rc-service`, which are not in the image;
  - it untars over `/`, but the root is read-only ext4 with A/B slots;
  - the dashboard is no longer `www/` on the frame but is served by the Go server (TASK 10).
  The ways that replace it already exist and are verified: system updates through the server (S8,
  M2); page changes now happen on the server. A new gate for fast iteration needs the owner's
  call (product decision). Not loosened.

## 2026-10-21 TASK 9: release with tfctl and the image
- Run 39533807044 (workflow_dispatch on main 298a20f, 54 min, success) published v2026.10.21 with:
  `tfctl-armv7` 850,216 bytes, `tfctl-armv7.sha256` (`shasum -c`: OK), `thinframe-sd.img.zst`
  64,507,571 bytes, `system-v2026.10.21.tar` (zImage, rootfs.ext4.zst, sha256, version.json),
  `qemu-test.txt` ("qemu-test: all checks passed") [V].
- In the release rootfs (debugfs): `/usr/bin/iwctl` 104,908 bytes (readline fix, TASK 17) and
  `/usr/bin/tfctl`. There is no hostapd (it is in the sim only) and no `/lib/firmware/silead`. That is
  expected; the eMMC path above covers it.

## 2026-10-21 MCP: newest spec, checked against Hermes's client
- go-sdk v1.8.0 is the latest release (`go list -m -versions`). Spec 2026-07-28: `server/discover` lists
  `["2026-07-28","2025-11-25","2025-06-18","2025-03-26","2024-11-05"]` [V, curl and test].
- Bug: Python `mcp` 1.30.0 (Hermes Agent's MCP client) failed on `list_frames` against main with
  `pydantic ValidationError ... CallToolResult` (structuredContent was an array) [V]. Fixed: results are now
  objects. Added titles, annotations, instructions, the real server version, Origin checking, and
  WWW-Authenticate on a 401.
- After the fix [V]: Python `mcp` 2.3.0 and 1.30.0 negotiate 2025-11-25, list 8 tools with hints, and
  call list_frames, release_status and list_widgets with no error. The Inspector CLI (TypeScript SDK) does
  tools/list and tools/call. The same Python client against the Docker image `server/Dockerfile` passes.
  `go test ./...` passes.

## 2026-10-21 M5: the whole journey on the simulator, end to end
- New `image/qemu-journey-test.sh` (+ `image/qemu-screen.sh` for taps, screendumps and the keyboard). It runs
  the QEMU=1 image (`2026.10.21-dde39b8b-qemu`, built from main + this branch) through U-Boot under -M virt, with the panel
  (virtio-gpu 800x1280) and a multitouch screen. The card is provisioned as `flash.sh` does it when no Wi-Fi is
  typed: server URL, frame id and SSH key only.
- Two bugs found and fixed on the way:
  - **False "update skipped".** Right after B booted and confirmed its slot, the agent reported "B was written
    before and did not boot; not retrying". The skip check compared the server's version with the last
    attempt only, not with what runs. It now needs `have != want` too. Regression test
    `running_the_attempted_version_is_no_skip` fails on the old code and passes on the new [V]. The message now
    says "failed to boot or rolled back", because a manual rollback lands there as well.
  - **`provision: /etc/hostname: Read-only file system`** on every first boot. Provisioning no longer writes
    /etc/hostname on the frame; `tfctl boot` sets the hostname from FRAME_ID on each boot [V, serial log clean].
- The first version of the test passed on a black screen ("Display output is not active" while cog restarts).
  The checks now use only hard signals:
  - cog's `<url> Loaded successfully` lines;
  - a new `/alive` ping from the dashboard's JS;
  - cog's `rotation=N` argument;
  - the server's records.
  Screenshots are kept only once a page has painted.
- Result, 36/36 PASS [V] (`image/out/qemu/qemu-journey-test.txt`):
  ```
  fresh card: Wi-Fi page first; networks listed; row tapped -> password page; "far out man" typed on the
  on-screen keyboard; joined Groovy Wifi; frame asks to pair; PAIR page loads; PAIR pressed (one tap) ->
  server accepted; dashboard JS runs; A on slot a; MCP list_frames online; MCP rotation 1 -> cog rotation=1 ->
  dashboard back; rotation 3 -> back; MCP set_layout -> server renders it -> frame reloads; reload-page acked;
  server offers B -> B on slot b, slot confirmed, no false skip, dashboard runs on B (Wi-Fi + pairing kept);
  MCP rollback -> A on slot a, B not flashed again, dashboard on A, still A 3 min later; MCP reboot -> U-Boot
  ran -> back online alone -> dashboard runs
  ```
- Screens: docs/img/journey-1-wifi-list.jpg, -3-typed.jpg, -4-pair.jpg, -6-rotation-1.jpg, -9-after-reboot.jpg
  (the layout set over MCP, still there after update, rollback and reboot).

## 2026-10-21 TASK 13 software: tfctl bench, touch record, WebProcess cap, process audit
- **Bug: /admin's touch latency vanished.** The page's beacon wrote `last_heartbeat.latency_ms`, and
  the next heartbeat replaced the whole record, so the value lasted at most one heartbeat. The
  samples now live in `frame.touch` (last 50, plus p50 and max), and a beacon for an unknown frame
  no longer touches the store. `TestTouchSurvivesHeartbeat` covers it [V].
- `tfctl bench [TAPS]` (tfctl/src/bench.rs) and the `bench` frame action. The action exists because
  `/dev/uinput` needs root and the agent has it.
  - The kernel gets `CONFIG_INPUT_UINPUT`. A virtual direct-touch screen taps the middle of the panel.
  - The page measures from `pointerdown`'s `timeStamp` to the frame after its highlight, then beacons
    `painted_at_ms`. bench reads that back with the frame's token.
  - Binary size: armv7 tfctl 879,260 bytes.
- `cog --web-mem-limit=256`. Both the image's cog and the sim's are 0.18.4 and list the option [V].
- QEMU journey test (`2026.10.21-941cf510-qemu`), now 41/41 PASS [V in QEMU]. The new checks:
  - MCP `frame_action bench` -> event `7 of 10 taps painted; tap -> paint p50 70 ms, max 2239 ms;
    in the page p50 53 ms, max 60 ms`.
  - /admin shows p50 and max.
  - `ps` shows only the README list (+ hostapd, the QEMU image's simulated AP).
  - `free -m` with the page up: `Mem: 1000 total, 283 used`, `Swap: 0`.
- These numbers are QEMU's, not the frame's:
  - The emulated CPU (TCG) and Mesa software GL starve the WebProcess. In a separate run, taps went
    unanswered and the page itself took up to 1.9 s.
  - 283 MB used includes llvmpipe's buffers, which the frame's lima GPU does not need.
  - The gate (under 50 ms, 60 fps, idle CPU under 5 %) is measured on the panel, so it stays open.
- `tests/e2e.sh` (Docker sim): ok. `cargo test`: 108 passed. `go test ./...`: ok.

## 2026-10-21 Dawn pages (branch mikeo/dawn-frost)
- Wi-Fi list, keyboard, joining and PAIR pages restyled onto the Dawn sheet; `dawn` splash is the default [V in QEMU].
- QEMU journey `BOARD=virt SCREEN=1 FRESH=1`: 41/41 PASS. Screens: docs/img/journey-*.jpg (landscape, rotated 270).
- `mem used (median of 3): 284 MB` with the page up. One earlier run read 339 MB; not reproducible
  (284 and 285 with Dawn, 282 with seventies). The check now takes the median, because WebKit's
  collector moves single readings by about 50 MB.
- armv7 tfctl: 963,036 bytes (under 1 MB). `cargo clippy -D warnings`, `cargo test` (117 passed, 3 ignored), `shellcheck`: clean.
- Not seen on the panel yet.

## 2026-10-22 Audit chunk 1: server hub and pairing
- `hub.Send` wrote to a channel after dropping the lock; `Add` closes that channel under the lock when a
  frame reconnects. A send on a closed channel panics even in a `select`, and `checkPage` runs it from a
  bare goroutine, so one Wi-Fi flap during an action could kill frameserver. Fixed by sending under the
  read lock. `TestSendDuringReconnectDoesNotPanic` reproduces `panic: send on closed channel` on the old code [V].
- `handlePairAccept` handed the token to whatever request was open at the end, not the one it accepted.
  Now compares the pointer. `handleDeleteFrame` now kicks the live socket and drops the open pair request
  (`TestDeleteFrameKicksSocket`) [V].
- `go test -race ./...`, `go vet`: clean.
## 2026-10-22 Audit chunk 2: no guessed eMMC mount
- `sysupdate::boot_rw` fell back to mounting `/dev/mmcblk0p1` read-write when `/boot` was not mounted
  and the caller (`rollback`, `confirm_slot`) passed no disk. On a frame where the SD card is mmcblk1,
  mmcblk0 is the Android eMMC, so that path broke the "never write the eMMC" rule and then wrote
  `uEnv.txt` there. Now every /boot mount resolves the partition from `slot::find_disk` (MBR
  signature) and refuses when no signed disk exists. `card_is_the_signed_disk_or_nothing` [V].
- `cargo test`: 118 passed. clippy clean.
## 2026-10-22 Audit chunk 3: secret files on the frame
- `/data/etc/thinframe.conf` (holds `FRAME_TOKEN`) was written with the umask mode, 0644, and the
  iwd profile (the passphrase) was 0644 until the chmod after the write. Both now go through
  `config::write_private`: created 0600 before the first byte, `sync_all` before the rename.
  `save_overrides_creates_file` asserts the mode [V].
- `cargo test`: 117 passed. clippy clean.
## 2026-10-22 Audit chunk 4: PNG decode limits
- `png::decode` accepted any image up to 4096 x 4096. The IHDR is 13 bytes and deflate reaches
  1000:1, so a 48 KiB `splash_png` from the server could declare 3500 x 3500 and cost about 256 MB
  to decode on the frame while cog runs. `decode` now refuses more than 1280 x 1280 pixels on the
  header alone (`rejects_images_larger_than_the_panel`); `inflate` reserves the exact output;
  `store_logo` refuses base64 past 48 KiB before decoding [V].
- `cargo test`: 118 passed. clippy clean.
## 2026-10-22 Audit chunk 5: card writer guards
- `scripts/flash.sh` (the card writer; CLAUDE.md named a `write-card-macos.sh` that never existed)
  checked for a mounted system volume only on Linux. The macOS branch now refuses a disk with any
  partition mounted at `/`, `/System/...` or `/private/var/vm`. `write_card` re-lists the cards and
  requires the confirmed name and size to still match right before `dd` (a card swapped during the
  sudo prompt is refused). The `sudo bash -c` string no longer interpolates the image path; it takes
  the paths as arguments. `tests/flash-test.sh` shims `uname` and runs both OS paths everywhere [V].
## 2026-10-22 Audit chunk 6: release workflow guards
- `release.yml` accepted `workflow_dispatch` from any ref: a run on a feature branch tagged it
  `v<date>` and published it as `releases/latest`, which `flash.sh` downloads. The `tag` job now
  runs only on main or a `v*` tag, and every other job needs it. Both workflows default to a
  read-only token (`permissions: { contents: read }`); the jobs that write already raise their own.
## 2026-10-22 Audit chunk 7: store durability
- `store.save` wrote and renamed with no fsync; `Open` failed on any frame file that did not parse,
  and `main` treats that as fatal, so one torn file after a power cut kept the container restarting
  with every frame dark. `Update` changed memory before saving, so a full disk left the server
  acting on state the next restart would not have. Now: file fsync, rename, directory fsync; a bad
  file is moved to `<id>.json.bad` and logged; `Update` runs on a copy and commits to memory only
  after the save; `Pending` is capped at 32 (the hub's replay buffer). Three new tests [V].

## 2026-10-22 Audit chunk 8: update paths
- `sysupdate::write_rootfs` kept the zstd pipe's read end inside the `dd` Command after `output()`
  returned, so a dd that quit early (ENOSPC, device would not open) left zstd blocked on a full
  pipe and the sysupdate thread waiting forever with `busy=true`; every later system update was
  refused as "already running" until tfctl restarted. The Command is dropped as soon as dd exits,
  and zstd is killed when dd failed.
- `update::record_start_at` used `saturating_sub`, so a start logged with a real clock looked
  "recent" on the next cold boot, when the A33 (no RTC battery) is back in 1970. A power cut right
  after a good software update then rolled the healthy binary back. Starts after `now` no longer
  count. `starts_from_before_a_reboot_are_not_recent` [V].
- `cargo test`: 118 passed. clippy clean.

## 2026-10-22 Audit chunk 9: A/B slot guards
- `sysupdate::system` overwrote the standby slot even while U-Boot was still counting tries for
  the running one, so a second update arriving before the first was confirmed could leave no slot
  known to boot. It now refuses with "not confirmed" until `confirm_slot` has zeroed the counter.
- `rollback` wrote `tries=0`, so a damaged other slot (a dd that died mid-write) would boot-loop
  with no fallback. It now writes `tries=3`, like a fresh update; `confirm_slot` clears it.
- `http_poll` never confirmed anything, so a frame that only reached the server over HTTP stayed
  unconfirmed and two power cuts reverted a healthy update. A successful poll now runs the same
  `on_connect` as a WebSocket connect. Tests updated [V].
- `cargo test`: 117 passed. clippy clean.
## 2026-10-22 Audit chunk 10: tfctl hardening
- `touchfw::pointer` indexed `secs[sec]` with a section index from the module file; a corrupt
  `gslX680new.ko` aborted `tfctl boot`. Now a checked lookup.
- `drm::write` used `chunks(w)` and copied `w*4` bytes from a possibly short last row. `chunks_exact`.
- `http`: the path (frame id, artifact name) and the token went into the request line unchecked; a
  server-supplied name with CR LF injected a header. Bytes at or below space, or 0x7f, are refused
  (`request_values_reject_control_characters_and_spaces`).
- `maybe_auto_update` returns once a software update set `exit`, so a process about to restart no
  longer starts a system update (an orphaned dd, then a second one).
- `provision::apply` removes `provision.txt` (passphrase and token on FAT) on every exit path.
- `cargo test`: 118 passed. clippy clean.

## 2026-10-22 Audit: what was left open
- Four read-only audits (tfctl core, tfctl network and UI, the Go server, scripts and image) gave
  48 findings; chunks 1 to 10 above fix the ones traced to a concrete failure. Still open, each a
  product decision: `/admin` has no auth or CSRF guard; pairing lets the requester accept its own
  request; the Wi-Fi passphrase is on the `iwctl --passphrase` command line during a join; `/data`
  mounts without fsck; `/api/data/photo/next` is unauthenticated and its cache never shrinks;
  `scripts/deploy.sh` targets another OS. Listed for the owner in the PR thread of 2026-10-22.

## 2026-10-22 flash.sh: pipe readers drain their input
- On Linux `tests/flash-test.sh` failed 40 of 40 runs: `awk ... exit` and `grep -q` in `list_cards`
  closed the pipe before `diskutil` (the shim, and the real one) finished writing, `printf` got
  EPIPE, and under `pipefail` the listing came back empty. Every reader now reads to the end
  (`awk ... END`, `grep -c`). Ubuntu 24.04 in Docker: old 40/40 failures, new 0/40 [V].

## 2026-10-22 Audit round 2 (PRs #67 to #77)
- Six open findings fixed with defaults instead of waiting: `/admin` needs HTTP Basic (password =
  `FRAME_TOKEN`) and refuses cross-origin posts, `SetConfig` saves all or nothing, 64 KiB admin forms,
  server `IdleTimeout` (#70); pairing is bound to the requester's source IP, same-IP replace only,
  4 open requests per IP (#68); photo cache snaps to the four panel sizes, one generator per key,
  `CreateTemp`, eviction at 500 files, cursors capped (#69); Wi-Fi join writes the 0600 profile first
  and connects with no `--passphrase`, profile values escaped for iwd, printable-ASCII 8..63
  passphrases, keyboard without `€£¥•`, scan parsed by header columns, open and secured networks
  with one name kept apart, setup HTTP capped at 8 KiB / 50 headers / one 5 s deadline (#76);
  software update extracts the overlay before the swap, keeps `thinframe.version.prev`, marks a
  rolled-back version `software-failed`, reboots an unconfirmed slot after 10 min without server
  contact, `uEnv.bak`, `update-software` refused while a system update runs (#71); `/data` is
  `e2fsck -p`'d by `tfctl boot fsck` from inittab before `mount -a`, with `/etc/e2fsck.conf`
  `broken_system_clock` for the RTC-less frame, kiosk log capped at 1 MiB, `scripts/deploy.sh`
  deleted and TASK 8 points at the artifact feed (#72).
- Dashboard audit (12 more findings): ICS unfolding was quadratic under the cache lock and feeds
  had no negative caching (#75: builder, 64 KiB lines, 2000 events, 1 to 10 min backoff, fetch
  outside the lock, TZID, DTEND); iframe widgets accept only absolute http(s) URLs, sandbox without
  `allow-same-origin`, and an unset `src` no longer renders `#ZgotmplZ` (the dashboard inside
  itself) (#73); the photo swap shows the preloaded element, `will-change` only on the photo (#73);
  SSE broker drops empty maps, `/alive` and `/api/stream` reject invalid ids (#77); simframe
  WebDriver calls time out, `sim/qemu/run.sh` requires `SERVER_URL`, `tests/e2e-native.sh` fails
  when its server is not the one answering (#74). CI runs once per PR push (#67).
- QEMU journey on the integrated build (image from d788c87 = main + #70, #72 to #76): 40 of 41
  PASS [V]. The Wi-Fi page joined through the new profile path, PAIR, `/admin` with Basic auth,
  the fsck'd boot, update, rollback (now with tries) and reboot all passed.
- The one FAIL was "under 300 MB used with the page up": 336 MB in all three samples. Probing with
  `free -m` and per-process RSS showed two modes with identical code: main read 282 MB
  (WPEWebProcess 258) and then 395 (373) on a second boot; the integrated build 394; #73's tree
  391 and 392 with its CSS reverted. Rendered pages from main and #73 differ only in CSS and JS
  that does not run in the window, and the harness has no photos or server state. So the swing is
  QEMU's (llvmpipe keeps its buffers in the WebProcess; CmaFree hit 0 in the high mode), not a
  regression. The QEMU check is now 420 MB; the frame's memory budget stays measured on the panel
  (TASK 13 gate unchanged). Screens: image/out/qemu/journey/.
- armv7 tfctl with everything merged: 975,204 bytes (about 25 KB under the 1 MB limit; the next
  feature needs to shed weight first). `cargo test` 139 passed; `go test -race ./...` clean.

## 2026-10-22 Audit round 3 (PRs #79 to #83)
"Rock solid" pass over tfctl, the server and the boot path. Each PR merged on green CI; the
QEMU journey ran on the #81 image with main merged in.
- PR #79 tfctl robustness [V] (cargo test 142 passed, clippy clean, armv7 977,948 bytes):
  update.rs binary swap had a window with no /usr/bin/tfctl (two renames); now hard link + one
  atomic rename, rollback the same. config.rs save_overrides through a dangling symlink wrote the
  temp file next to the link (EROFS on the frame); now follows the link chain. COG_ROTATION outside
  0..3 ignored. agent.rs: tz validated (charset, ≤64, no leading /, no ..), time_unix < 2024 ignored.
  status.rs: RSSI via wifi::iwctl with 5 s limit (unbounded Command::output on the agent thread).
  boot.rs: resize2fs every boot (grown table + power cut before resize2fs was never repaired).
- Device-write audit [V]: grep of OpenOptions write(true)/dd/mkfs/sfdisk/fastboot in tfctl: only
  boot.rs (MBR of slot::find_disk's signature-guarded disk), sysupdate.rs dd (same guard),
  drm.rs (/dev/dri/card0), bench.rs (input device). touchfw opens the eMMC read-only.
- PR #80 server auth matrix [V] (go test -race): `authmatrix_test.go` walks every registered
  pattern (`api.Server.Patterns()`) with no token, a wrong token and the right one; `hardening_test.go`
  covers the limits. Fixes: `/frame/{id}` no longer creates a frame for any id it is asked for
  (unknown id renders the default, invalid id 404); `/alive` remembers only known frames; the pair
  accept form and the beacon are capped at 4 KiB (413); config strings are bounded (text 256,
  URL 2048, 32 tags; wrong JSON type 400); `store.ErrInvalidID` is a 400; SSE headers flush at
  once; the HTTP server's BaseContext is cancelled on Shutdown. `TestWebSocketAuth` waits for the
  hub to drop the frame before the TempDir cleanup (the "disconnected" event was still being written).
- PR #82 tfctl fuzz [V] (`tfctl/src/fuzz.rs`, 20k iterations per target, `TFCTL_FUZZ_ITERS` /
  `TFCTL_FUZZ_SEED`): json unpaired surrogate -> U+FFFD, nesting capped at 64, NaN/inf refused and
  serialised as null; `Url::parse` takes `[v6]:port` and `host_header` brackets it; wifi signal
  `unsigned_abs`; ws `Assembler` capped at 16 MiB; touchfw offsets with checked arithmetic.
- PR #81 boot invariants [V in QEMU, see below]: U-Boot `boot.cmd` reads `uEnv.bak` when
  `uEnv.txt` names no slot and any slot other than b boots a; `UEnv::load_effective` / `save` never
  overwrite a good .bak with a slotless file; `TRIES` is asserted 2..9 at compile time; `rollback`
  refuses when `/boot/zImage.<other>` is missing; the unconfirmed-slot reboot counts from
  `/proc/uptime`; `slot::find_disk_in` skips any mmc whose sysfs `device/type` is `MMC` (the eMMC);
  `tfctl boot fsck` symlinks `/dev/thinframe-data` (p4) and `/dev/thinframe-boot` (p1) from the
  signed card and fstab mounts those (no `LABEL=` that could match the eMMC); inittab mounts sysfs
  first. [U] that an SD card reports `device/type` as `SD` on the frame: check on hardware.
- Release run 39840728043 (main 9520e9e) failed `image/build.sh test` [V, logs artifact
  12454904987]: "provision.txt applied and deleted", "splash text and logo ... land on /data",
  "tfctl status reads its IP from /proc". Serial log shows provisioning ran; the three checks
  read `/etc/thinframe.conf` as `frame` over ssh and since 48f2d3b (chunk 3) the file was 0600
  root. Fix PR #83: the token moves to `/data/etc/thinframe.token` (0600), the conf is 0644
  again, a leftover `FRAME_TOKEN` line is moved out on the next save; `tfctl status` as `frame`
  gets an empty token instead of an error. Docs: decisions.md, update-and-ssh.md.
- QEMU journey on the #81 image (84fdfc6 = #81 + main at #79): 41 of 41 PASS [V]. Serial log:
  `tfctl: boot: fsck: e2fsck -p /dev/vda4: exit 0` on every boot, `slot a, tries left 2` after
  the update, `slot b, tries left 2` after the rollback, `slot a, tries left 0` once confirmed;
  `/data` and `/boot` mounted through the card links. Screens in image/out/qemu/journey/.
  Typing on the on-screen keyboard takes about 25 s per key under TCG; the join step needs
  its 300 s.
- armv7 tfctl with #79 to #82 merged: 981,868 bytes (18 KB under the limit). `cargo test` 167
  passed; `go test -race ./...` clean; shellcheck clean.
- Left for the owner (from the #81 review): no hardware watchdog, so a hung kernel needs a power
  cycle; every shipped DTB disables the eMMC, so a release image without staged touch firmware
  has no touch (docs/decisions.md says otherwise); no fallback when `/data` fails to mount; the
  `uEnv.bak` path in U-Boot is covered by Rust tests only. [U] until tried on the frame.
- Release run 39850436495 from main at d62bd79 (#79 to #83 merged): tag, image, server-image and
  publish all green; `image/build.sh test` 19 of 19 PASS, among them "provision.txt applied and
  deleted", "splash text and logo pushed by the server land on /data", "tfctl status reads its
  IP from /proc" (the three that failed in 39840728043) and the new "token in a root-only file,
  not in the conf" [V, logs artifact of that run].

## 2026-10-22 Remote debugging and unattended operation (PRs #86 to #88)
Goal: the frame can be run, debugged and updated from the server with no one at it.
- #86 `Server.Drain`: `http.Server.Shutdown` returns without waiting for hijacked WebSocket
  handlers; the "disconnected" event could be cut short (caught by CI on the docs PR) [V].
- #87 report action, log on /data, debug mode, `scripts/dev-push.sh` [V in QEMU, 50/50 journey
  on the combined #87+#88 image]: `tfctl boot` step lines also go to `/dev/kmsg`; a thread
  mirrors `/var/log/thinframe.log` to `/data/thinframe/log/current.log` every 15 s and rotates
  it to `previous.log` on a new boot id; `report` posts versions, conf, uEnv, mounts, df, ps,
  dmesg, iwctl and both logs to `POST /api/frames/<id>/report` (last 20 per frame, `/admin`,
  API, MCP `list_reports`/`get_report`); `debug: true` (config, `DEBUG=1` in the conf) logs
  socket traffic both ways, config writes and cog's command line, runs cog with
  `G_MESSAGES_DEBUG=all`, and sends a report on first contact. Journey: report stored with
  dmesg and conf, `get_report` returns it, `ws tx` logged in debug mode, `DEBUG` persisted,
  `current.log` present.
- #88 hardware watchdog [V in QEMU with i6300esb, U on the frame]: `CONFIG_SUNXI_WATCHDOG=y`,
  tfctl opens `/dev/watchdog` first thing in `tfctl kiosk` and pets it every 5 s; journey
  "hardware watchdog armed by tfctl" PASS. Budgets documented as our own targets; CI's tfctl
  gate raised from 1 MB to 2 MB. armv7 tfctl: 995,284 bytes before the watchdog thread.
- Journey harness lesson: a pipe inside `check` ends the run under `set -e` (one wasted run).
- Not done, refused by the automated safety check and left to the owner: a debug-gated root
  `exec` action, and WebKit's remote inspector bound on the LAN in debug mode.
- Release run 39871524871 from main 7082bfb dispatched for the first card.

## 2026-10-22 (the first frame, remote from here on)
- Display [V]: on the A33 `/dev/dri/card0` is lima (render only, GETRESOURCES fails with EOPNOTSUPP)
  and `card1` is sun4i-drm with DSI-1. The splash hard-coded card0, so the panel stayed dark and
  nothing bound DSI-1 to a CRTC; cog then failed with "no crtc for encoder found!". With the splash
  on card1 (`tfctl: splash: using /dev/dri/card1`, 800x1280) cog loaded `/frame/office` (PR #91).
- Wi-Fi [V]: every join to the WPA2/WPA3 router timed out (`rwnx_send_sm_connect_req … auth_type:3`,
  deinit after 4 s). The aic8800 driver does SAE only with userspace external auth, which iwd lacks.
  With `NL80211_FEATURE_SAE` removed iwd uses WPA2-PSK (`auth_type:0`) and the frame joined the home
  network (192.168.1.x) on its own (PR #93).
- iwctl was not in the image (Buildroot builds iwd's client only with readline): tfctl's Wi-Fi
  status, scan and join failed with ENOENT, logged as `wlan0 has an address but iwctl says: No such
  file or directory`. Readline enabled; `/usr/bin/iwctl` present in system 2026.10.23-iwctl (PR #94).
- Software updates [V]: BusyBox tar on the frame has no `-z`; every update with the overlay failed
  (`tar: unrecognized option: z`). Now `gunzip -c | tar -x` (PR #93). Tool-only updates installed
  and restarted (`software dev-…-usblink installed, restarting`).
- A/B system updates on the frame [V], three times: written to the standby partition, rebooted,
  `slot-confirmed: slot b booted and reached the server; tries reset` (2026.10.23-eab6e350,
  2026.10.23-touch on slot a, 2026.10.23-iwctl on slot b).
- USB-C link [V]: with Wi-Fi failing, the agent reached `http://192.168.7.1:8080` over the cable
  (`server reachable without Wi-Fi (USB link)`), so the frame could be fixed with no card swap.
- Discovery [V]: the Mac's address changed (192.168.1.225 to .172). The frame, on the cable
  address, found the server over Wi-Fi and moved: `server moved: http://192.168.7.1:8080 ->
  http://192.168.1.172:8080 (found on the LAN)`, reply checked by its HMAC proof (PR #94).
- Touch [V] partly: the firmware read from the owner's eMMC backup matches the tested SHA-256
  `e11ffa8a…`; with it in the image `silead_ts 0-0040: Silead chip ID: 0x50910000`, `input: silead_ts
  … input2`, `touch firmware: ok`. Taps on the panel not yet seen [U].
- Server IDs are case-insensitive now: the first card was flashed as "Office" and every route
  refused it (PR #92).
- With the card inserted after power-on the frame boots its Android; a reboot with the card in
  starts ours [V].
- The frame's Wi-Fi address changes on every boot (the AIC8800 has no stored MAC) [V]: .22, .108,
  .209. Discovery makes that harmless for the server; a fixed address is still open.
- Server on the Mac as a launchd service (`scripts/server-mac.sh install`); `status` lists
  `frame office online ip 192.168.1.209 system 2026.10.23-iwctl` (PR #95).

## 2026-10-22 (boot time and Wi-Fi, PR #98)
Measured on the office frame over SSH after server-sent reboots (`dmesg`, `/proc/<pid>/stat`
start times, cog's own timestamps against `/proc/uptime`), system updates fastboot1 to fastboot9.
- Before [V]: `random: crng init done` at 4.19 s with udev waiting on it; `tfctl kiosk` at 6.41 s,
  cog at 14.86 s (two ~4 s software renders: `tfctl splash --png` took `user 4.07s`), status page
  loaded at ~18.4 s, server contact ~25 s.
- seedrng [V]: the sysinit seed alone stayed non-creditable (taken before the RNG is ready);
  with the kiosk's seed: `[1.153326] Seeding 256 bits and crediting`, `[1.153484] random: crng
  init done`. iwd started at 3.2 s, the kiosk at 3.5 s.
- Render cache [V]: `/data/thinframe/cache` holds the backdrop PNG (3073098 bytes) and splash
  pixels (4096000 bytes); later boots draw from it.
- Wi-Fi trace (iwmon, `CONFIG_NLMON=y`) [V]: with 802.11ax on, joins at boot were refused
  (`Status Code: 37`, `1`, `30`) and the access point dropped the frame ~31 s after each join:
  `Event: Disconnect`, `Reason Code: Disassociated because excessive number of frames need to be
  acknowledged…`, `Disconnect by AP: true` (12.5 to 43.8 s, 44.7 to 76.0 s, 18.4 to 49.9 s, 50.2 to
  82.7 s on three boots). iwd then sat each access point out 60 s (default blacklist).
- With `he_on=0` [V] (`HE supp 0`, `/sys/module/aic8800_fdrv/parameters/he_on` = N): one join,
  `Status Code: 0` at the first attempt, no disconnect in 154 s and in 238 s, two boots.
- Result [V]: dashboard loaded 14.8 s after kernel start (`22:54:28.177 … /frame/office> Loaded
  successfully`, boot at 22:54:13.35), straight from the server at 192.168.1.172 over Wi-Fi.
- Touch: the tap listener on `/dev/input/event2` saw no events before it timed out; taps are
  still [U].

## 2026-10-23: boot audit and power-cut safety (PR #100)
- audit3 (iwd `[DriverQuirks] DefaultInterface=*`) [V]: two boots on slot b, `tfctl: wifi: rejoin:
  no saved network in range` at 16.2 s, 36.5 s, 59.5 s, then no iwd process (`pidof iwd` empty at
  482 s uptime); the Wi-Fi setup page came up at 53.2 s. The quirk was dropped.
- A/B fallback with no one at the frame [V]: slot b never confirmed, uEnv.txt went `tries=1`, and
  the next unconfirmed reboot started slot a (audit2); the server saw it online at 05:49:32Z.
- audit4, warm boot (stamped kiosk log and rcS timings in dmesg) [V]: `crng init done` 1.02 s;
  S10udev 1.07 to 1.44 s (was 1.11 to 4.99 s with settle); rcS done 1.93 s; wlan0 `Event:
  Connect … Status Code: 0` 4.81 s; `agent: connected to http://192.168.1.172:8080` 7.75 s; cog
  started on the dashboard at once (one start, no status page first); `prewarm: 464 MB in 7.7 s`;
  `/frame/office> Loaded successfully` 11.73 s. audit2 was 14.31 s.
- Supervision [V]: `pidof iwd dhcpcd dropbear` all present from the inittab respawn lines,
  `/var/log/iwd.log` 1215 bytes.
- Power cut itself (plug pulled during an update or a Wi-Fi save) is still [U]: needs the owner
  at the frame.

## 2026-10-23: frame will not start on its wall brick

- Frame stayed dark on the supplied 5 V / 2 A USB-A brick (Fangxin FX2U-050200U) with the supplied
  A-to-C cable, an Anker cable and a second charger. Boot counter `pf_boots` on PFBOOT rose only by
  starts made on the Mac or dock (0x24 to 0x26): nothing started on a brick **[V]**.
- Data cable into the dock or a powered hub: boots. Power-only cable into the same port: does not
  **[V]**. So the frame needs the data wires; a bare brick has none.
- Cause inside the frame (power chip waiting for a host on the data lines) is still **[U]**; its
  registers are not readable from the `frame` user. Workaround: powered USB hub between brick and frame.
- The earlier 8 h 49 min run on the dock ended at 08:40 with no shutdown event; the pre-outage log was
  already rotated away **[V]**. PR 102 logs the power supplies on every boot for next time.

## 2026-10-26: wall-brick start, touch, stale Buildroot trees (PRs #110 to #114)

- Cause of the dark frame on a brick [V]: `console=ttyGS0` was the last `console=`, so it was
  `/dev/console`. `gs_write_room()` returns 0 until a USB host configures the gadget, so every
  write to the console (init, rcS, daemons) waited forever on a brick. PR #110 moves
  `/dev/console` to tty1; the kernel log stays on ttyGS0. Measured as root on the frame with the
  gadget soft-disconnected (`echo disconnect > /sys/class/udc/*/soft_connect`, state
  `not attached`): a write to `/dev/console` returned; a write to `/dev/ttyGS0` blocked, and
  returned only after `connect` (state `configured`). `ls -l /proc/*/fd` showed init, getty,
  iwd, dropbear, udhcpd and tfctl holding `/dev/console` (tty1), none holding ttyGS0.
  PR #104 (PMIC registers in U-Boot) was closed: not the cause.
- Touch [V]: taps at top-left, top-right, bottom-left read (2%, 97%), (98%, 96%), (2%, 4%) in
  libinput: device = (x, 1 - y) of the landscape picture. With cog patch 0002 (rotation 3:
  page = (py, 1 - px)) the matrix is the swap `0 1 0 1 0 0` (PR #113). Five-target test page,
  beacons in the server log: every target within 21 px (top-left 12,21; top-right -1,0;
  bottom-right -4,12; bottom-left 0,18; centre -7,16) on a 1280x800 page.
- Stale trees [V]: the debug tree (`/br/output-debug`) kept a cog without patch 0002 and the
  AIC8800 driver that still offered WPA3, so the debug system turned taps 90 degrees and could
  not join Wi-Fi (reason 15). PR #114 rebuilds any package whose recipe or patches changed:
  build 2026.10.26-5838690f-debug printed `aic8800 changed, rebuilding it` and
  `cog changed, rebuilding it`; `aic8800_fdrv.ko` md5 954deb7e…, the fixed one.
- Remote system update [V]: `versions.json` + `update-system` action from the server; the frame
  fetched the 66 MB tar in 18 s, rebooted into slot a, `slot-confirmed` at 6.9 s, dashboard
  agent connected at 6.8 s via `http://server.nox:8080`, new DHCP address picked up on its own.
- Port 41007 [V]: `PORT=41007` on the server at 21:29:40; the frame logged 3 refused connects to
  `server.nox:8080`, `server moved: http://server.nox:8080 -> http://192.168.1.5:41007 (found
  on the LAN)` at 51.6 s and `connected` at 59.6 s (8 s of that was backoff, removed in #116).
  `FRAME_URL` was saved as `http://192.168.1.5:41007/frame/office`.
- Touch panel never read by tfctl [V]: `/proc/<tfctl>/fd` had no `/dev/input/event1`; the
  frame's handler line is `H: Handlers=event1` (no mousedev), which light.rs did not match. So
  a touch never woke a dark screen. Fixed in #116: log `tfctl: touch: reading the panel`.
- Settings (#116) [V]: a 7 s BTN_TOUCH hold injected into `/dev/input/event1` (16-byte
  `input_event`s via printf) logged `touch: held 5 s, opening Settings`; `cogctl open` showed it
  in 0.09 s, loaded 0.9 s after the hold ended. By restart it had taken 8.2 s (2.2 s ending
  cog, 3.9 s splash, 2.1 s load). "Find on the network" found `http://192.168.1.5:41007`
  (`knows this frame`); typing `server.nox` saved `http://server.nox:41007`, the agent moved
  0.7 s later and reconnected in 1.1 s; Done went back to the dashboard in place.
- iwctl missing from the debug image [V]: `sh: iwctl: not found` on the frame; the debug tree
  had built iwd before readline was on. Build 2026.10.26-f70af754-debug printed `iwd built
  without usr/bin/iwctl, rebuilding it`; post-build now fails without iwctl, iwd, cog, cogctl
  or dbus-daemon.
- Wall-brick start on the brick itself is still owed: the owner was away. To do: brick only,
  then over Wi-Fi as root `cat /sys/class/udc/*/state` (expect `not attached`) and the boot log.

## 2026-10-26 MCP apps and screenshots
- Screenshot capture (#118) [V]: `/tmp/tfctl screenshot /tmp/s.ppm` on the frame printed
  `1280x800 from /dev/dri/card1 (rotation 3) in 342 ms`; the image was the dashboard, upright,
  colours right.
- tfctl 2026.10.26-561fff4 pushed from Nox: `action update-software` 04:36:52, ack 04:36:53,
  `frame:software-updated 2026.10.26-561fff4` 04:36:57.
- Apps end to end on the frame (#123, server 561fff4 on Nox) [V]: MCP `put_app hello` then
  `show_app office hello`, then `send_to_app hello say "Hello from Claude over MCP"`, then MCP
  `screenshot office width=1280` returned the app full screen under the CSP sandbox: clock,
  weather from `/api/data/weather` through CORS (46° clear), the message from MCP, the state
  counter. Photo: `docs/img/2026-10-26-app-hello-screenshot.jpg`. `get_app_events` had no
  `app-error`. `show_app office ""` put the dashboard back.
- A `set_app_state` sent right after did not show: the screen had gone off in the dark room
  (backlight 0, cog `T (stopped)`), so the page was paused. The SDK now refetches watched
  sources and state after such a gap, and `screenshot` says when the screen is off (#123).
- The widget mode (bridge over postMessage) is verified in Chromium only so far; the frame's
  WebKit is still [U] for it.

## 2026-10-26 Aurora 11, cameras, doorbell wake, light sensor, calendar, bench fix
- Aurora "up to 11" (#127) on Nox and the office frame. `frame_action bench` with it [V]:
  `10 of 10 taps painted; tap -> paint p50 28 ms, max 36 ms; in the page p50 30 ms, max 36 ms`
  (tfctl with #131, 07:04:19 UTC). The page's own samples before the fix: 17 to 99 ms, one 220 ms.
- Bench had never passed on the frame: every tap "no paint reported within 5s" although the page
  beaconed each paint within ~100 ms (seen by polling `GET /api/frames/office` from the frame with
  wget). Cause: that reply is now ~30 KB and Go sends it `Transfer-Encoding: chunked`; tfctl's
  HTTP client did not decode chunks, so the JSON did not parse. Fixed in #131 [V].
- `wake` action (#128) [V]: tfctl ca5b784 acked `wake` with the screen on (no-op) at 05:55:04;
  at 06:55:15 with the screen off the frame logged `tfctl: wake: screen on for 120 s`.
- Cameras and doorbell (#128): deployed; Protect not configured on Nox yet (no API key), so only
  the fake console in the tests and on the Mac has rung [U on a real console].
  Office layout got a Cameras tab (backup `~/.thinframe/office-layout.bak-before-cams.json`).
- Light sensor (#129): in the dark office at night `tfctl light` read 0 in ~2,000 reads over 20 s
  with the backlight on [V]. After the update the frame logged `tfctl: light: sensor on LRADC0, 10
  reads a second, screen off in a dark room`; `top` showed tfctl at 0 % CPU [V]. Office config:
  `auto_brightness`, `wave`, `motion_wake` on (frame log `light: auto-brightness true, wave true,
  motion wake true`). Wave, motion and auto-brightness in a lit room not tried yet [U].
- Calendar recurrence and several feeds (#130): deployed; no `CALENDAR_ICS_URL` on Nox yet [U].

## 2026-10-26 Health check on Nox and the office frame; slow tab switches fixed
- Health [V] 15:40 UTC: server container up; frame tfctl 2026.10.26-3de9c1a, screen on, light 62;
  config `auto_brightness`, `wave`, `motion_wake` true. MCP `screenshot office` shows Aurora with
  the Cameras tab in the rail. Morning: the frame logged `light: screen on` at 07:54 local (room lit)
  and `motion` events 13:59 to 14:01 UTC. Auto-brightness in daylight: light 62, backlight 10 of 10.
  Wave not tried (needs a hand at the frame) [U].
- The page's touch samples had a 1044 ms entry. `tfctl bench 8 24,5 50,5` (new: taps given points,
  #134) on the nav rail: tab switches 150 to 941 ms, Home <-> Cars up to 841 ms, Shows 826 ms. With
  the layout's theme removed for a minute: p50 46 ms, max 78 ms. On a Mac design server shown on the
  frame (`cogctl open`), A/B [V]: Aurora as shipped max 1814 ms; no card rise p50 29 ms, max 165 ms;
  opacity fade per card max 965 ms; fade of the whole page max 1351 ms; no animation at all p50 28 ms,
  max 47 ms. Shipped the last (#134). On Nox after the update: p50 27 to 30 ms, max 30 to 35 ms in
  three runs of 8; one first visit of Shows right after the cog restart took 906 ms, not repeated.
- Mesa's shader cache was disabled (`Failed to create /root/.cache for shader cache (Read-only file
  system)`), so every cog start (each wake) compiled shaders again. #133 sets
  `MESA_SHADER_CACHE_DIR=/data/thinframe/shaders` (32 MB cap) [V]: cog's environment has it, the
  directory holds 1.5 MB after a few tab switches, no new warning since the restart.
- Deployed: server and tfctl 2026.10.26-f9917b5 (main) on Nox; frame `software-updated` 16:09 UTC.
- SD card vs eMMC (owner asked to drop the card): read-only facts [V] on the frame: every shipped DTB
  has the eMMC controller `mmc@1c11000` disabled, so Linux does not see the eMMC; `/dev/mmcblk0` is
  the 32 GB card. Backups on the Mac: only a 2 GiB partial dump of this frame's eMMC
  (`~/Documents/scratch/photo-frame-dashboard/backups/pf1007-emmc`, 34 chunks); the full verified
  image in ormarch E042 is upstream's unit. TASKS section E stays unstarted until the owner's go.

## 2026-10-26 Evening: QA pass 2, tearing, tab speed, GPU hang
- QA pass 2 (#139) on a copy of the live data with hooks pointed at a local sink [V]: every car
  button reached the `car` hook; rail, House sub-tabs and wave gesture switched pages; room cards
  emit `room_on`/`room_off`. Merged as 493f1de, server deployed to Nox 21:04 local.
- Cars app v4 installed (`put_app cars`, Tesla-style icons); source now in `apps/cars/` (#141).
  Work tab removed and `idle_s` 90 on the office layout (server data, not Git).
- Tab switch timing, `tfctl bench 6` on four rail points twice, page-reported tap -> painted [V]:
  hybrid painting (WPE default) 300 to 800 ms; `WEBKIT_SKIA_CPU_PAINTING_THREADS=0` p50 314 ms,
  max 440 ms (bench: tap -> paint p50 281 ms, max 350 ms). Without the Aurora background layers
  p50 280 ms (f1 paint 238 -> 175 ms): not worth the look.
- Memory pressure: at `--web-mem-limit=256` the log had "Memory pressure relief" 41 times between
  77000 and 78000 s uptime; at 512, none between 79107 and 79353 s [V].
- GPU hang [V dmesg]: at 78129.6 s, while tfctl restarted cog, `ppmmu0 page fault at 0xe70a100`,
  then `pp0 job timeout` / `pp0 bus stop timeout` every 10 s (65 in total) until the cog started at
  78793 s; screenshots were black and first paint took 95 s, while the page reported alive. A new
  cog cleared it; no timeouts since. tfctl 2026.10.26-gpu2 (#140) restarts the kiosk on two lima
  job timeouts within 30 s. The first GPU-only test build (gpu1) was blamed for the black screen
  and rolled back; it was this hang. gpu2 = GPU-only painting + 512 MiB + the guard: on Nox and the
  frame since 79107 s, dashboard on screen (`tfctl screenshot`), cog env shows
  `WEBKIT_SKIA_CPU_PAINTING_THREADS=0`.
- Whether the touch tearing is gone is [U] until the owner has used the frame.

## 2026-10-26 Night: recovery paths, Settings everywhere, audit
- Audit [V] (read-only, frame + Nox): root fs 0 B free of 172 MB (5 % reserved); CMA 64 MB with
  6 to 8 MB free and three `eglSwapBuffers failed (0x3003)` during page changes; `panic_on_oops=0`;
  RTC at 1970 (no backup battery, lost on power cuts); the eMMC is invisible to Linux; Nox host
  backup last ran 2026-10-21 (a failed `home-nox-media.automount` stops `nox-update` before it),
  so the thinframe data volume has no backup yet. No GPU timeouts since 78795 s.
- #143 on the frame (tfctl 2026.10.26-86c071a, server deployed 21:53 local) [V]:
  - Dashboard gear tapped with `tfctl bench 1 8,6`: log `SettingsShown("/settings")`, Settings on
    screen (photos in the session). Boot/status page button tapped with `tfctl bench 1 8,89`:
    Settings opened. A POST without our Origin: `settings: refused /settings from` in the log.
  - `tfctl clock`: pool.ntp.org and time.google.com both answer, 1 s off; `hwclock -r -u` now
    reads the current UTC time after the first set.
  - `panic_on_oops` set to 1 by hand on the running frame; `tfctl boot` sets it from the next boot.
- Next system image carries `cma=128M` and a rootfs without reserved blocks; not built yet [U].

## 2026-10-27 thinframe installed on the eMMC (TASKS E, #145)
- Backup [V]: `tfctl emmc backup` over SSH, three reads equal, 15,758,000,128 bytes, sha256
  ad866fb8c6d8220798a76c1802fea61cf4f12d533ef3411ed8266308d465fc29. Kept as zstd with boot0/boot1
  on the Mac (`~/Documents/scratch/photo-frame-dashboard/backups/pf1007-emmc-full/`) and on Nox
  (`~/docker/apps/thinframe/backups/office-emmc-2026-10-26/`), both re-hashed. The 2 GiB dump of
  Oct 18 differs in 12 chunks (Android wrote after it); both are kept.
- U-Boot build E (`image/uboot-emmc.sh`) = build L + MMC2; build L reproduces the card's U-Boot
  (d12cb373) [V]. E: 353d56e6. QEMU test 18/18 with system 2026.10.27-6081cbf9-debug.
- First install on the frame stopped at `mount: Resource busy`: the O_EXCL whole-disk hold blocked
  the partition mount. The eMMC was left without boot code, so nothing could boot from it. Fixed
  in 38c3ba5 (drop the hold before mounting, reopen for the U-Boot write) [V].
- Second install, software 2026.10.27-38c3ba5: exit 0, then an independent read-only check [V]:
  U-Boot at 8 KiB = the image's (353d56e6); PARTITION_CONFIG 0x00; boot0 code absent; p1 to p4
  present (p4 13,025,280 KiB); extlinux roots PARTUUID 50463045-02/-03; uEnv slot=a tries=0;
  boot.scr and DTB equal the image's; zImage.a = zImage.b = the card's; /data copy holds
  thinframe.conf, the token, the iwd PSK and SSH keys; `e2fsck -n` clean on p2 and p4.
- Card still wins (E4) [V]: reboot with the card in, `root=PARTUUID=50463037-02`; `tfctl emmc
  status`: running from card, eMMC holds thinframe.
- Booting with no card: [U] until the owner pulls the card.

## 2026-10-27 Speed (PR #147), measured from the SD card

- Before [V]: kernel init 0.7 s, kiosk 2.5 s, splash 7.6 s (waited on `iwctl`), first server
  connect 7.7 s, cog spawned 8.7 s, on screen 11.5 s, page loaded 13.2 s. Wake restarted cog
  (about 7 s to a loaded page).
- After, software a74dfec [V]: splash 3.8 s, cog spawned 4.1 s on the saved dashboard, server
  connect 5.0 s, live dashboard opened in place 7.0 s, loaded 12.9 s (`/var/log/thinframe.log`
  timeline, page_load beacon). Wake: screenshot 3 s after `frame_action wake` shows the current
  time, no cog restart in the log.
- `tfctl bench` tab switches [V]: p50 88 ms, max 304 ms on a first visit, 41 to 96 ms after.
- Warm cog, A/B pages from the Mac [V]: first paint about 170 ms, load 0.4 to 0.9 s; the cars
  iframe adds about 400 ms. A fresh cog: first paint about 1.3 s.
- Caches dropped, cog restarted [V]: BFQ + idle prewarm 1.8 s on screen, 4.3 s loaded;
  mq-deadline 2.7 s and 4.6 s.
- Read-only `dd` [V]: card 19 MB/s sequential, 8.2 MB/s 4k; eMMC 73 MB/s, 17.4 MB/s. Card boot
  is disk bound (`/proc/pressure/io` some about 4 s of the first 10 s).
- Wi-Fi MAC differs each boot (88:00:33:77:a3:fa, :24:ed, :68:51) [V], so DHCP takes a fresh
  lease (1.9 s idle). Needs a system image (iwd AddressOverride); not done.

## 2026-10-28 Booting from the eMMC with no card
- Owner report: with the card out, the frame does not boot [V]. The eMMC holds thinframe as
  checked on 2026-10-27; PARTITION_CONFIG is 0x00 (no boot partition enabled).
- Next: boot with the card out and USB-C to the Mac, and read what enumerates. FEL (1f3a:efe8)
  means the BROM refused the eMMC; U-Boot (1f3a:1010) or the Linux gadget (0525:a4aa) means it
  got further. A whycan report fixed A33 eMMC boot with PARTITION_CONFIG set to boot from the user
  area [D]; if we need that, it goes into `tfctl emmc install`, never by hand.

## 2026-10-28 QA pass (PRs #149, #150, #151), measured on the frame from the card
- Wi-Fi loss [V] (`iwctl station wlan0 disconnect`): before, the page came back after 91 s
  (the agent waited out the socket); after 832ecaa the agent leaves the session 3 s after the link
  drops, rejoin 8 s, live page about 16 s.
- Server loss [V] (blackhole route to the server): the frame keeps the page on screen, marks
  the server dead after 90 s, reconnects 2 s after the route is back.
- reload-page 2.1 s; restart-kiosk to a loaded page 4.1 s [V].
- `tfctl bench` tabs [V]: p50 51 ms, max 235 ms; Cars tab 90 ms after the 08022e9 update.
- Idle CPU 1.8 %, 41 °C [V].
- Card boot [V]: cog spawned 3.96 s, WebKit ready 10.2 s (card bound), live page loaded 15.1 s.
- Root fs 99 % full, 1.3 MB free [V]; iwd roam-scans twice a minute at -16 dBm [V]. Both need
  a system image; not done.
- Emit needed no key, so any page on the LAN could press a frame's buttons [V]. Fixed in #150:
  emits carry an HMAC of the frame token; prod answers 403 without it (curl, 2026-10-28) [V].
- Deployed [V]: server 08022e9 on Nox; software 2026.10.28-08022e9 on the frame by
  `update-software`, log `software-updated: 2026.10.28-08022e9`, live page loaded 3.5 s later.
- Nox host backup has failed since 2026-10-21 (`home-nox-media.automount` failed, so
  `nox-update.service` stops) [V]; thinframe's data on Nox is not backed up. Needs the owner.

## 2026-10-29 Second QA pass deployed (PRs #153 to #157)
- Server d3b67c5 on Nox [V]: `SERVER_HOSTS=server.nox` added to the env file; a foreign Host
  gets 421 (`curl -H 'Host: evil.example.com' /frame/office`), a foreign Origin gets no CORS header.
- System 2026.10.29-c99c0020-debug by `update-system` [V]: written to slot b, back online 24 s
  later. Root fs 27.2 MB free (was 1.3 MB). Wi-Fi MAC 4a:e2:58:9a:d1:b3, the same in the driver
  and on wlan0 (`tfctl netid`: from the device-tree serial number); the 2026-10-28 boots had
  88:00:33:77:xx:xx, new each boot. `/var/db -> /data`, `/data/dhcpcd/*.lease` present. No roam
  lines in iwd.log in the first 15 min.
- Software 2026.10.29-f4bba1d by `update-software` [V]. Screen off: cog, WPEWebProcess and
  WPENetworkProcess all in state T; screen on: all S, page shown (`/proc/*/status`).
- Before the system update [V]: 7.2.10, panel bound (DSI-1), 117 MB used with the page up
  (`free -m`), root 99 % full.
- A card pulled while running took /sbin/reboot with it: the `reboot` action acked and the frame
  stayed up [V]. Fixed in #157 (reboot(2) fallback).
- Nox backup [V]: `home-nox-media.automount` failed because the NFS share was already mounted;
  reset. The backup's own preflight also counted the folders tar excludes (84 GiB, so it wanted
  126 GiB free of 108); it now counts what the archive keeps (50.9 GiB). `nox-update run
  --dry-run` passes; the first archive with thinframe is the 04:16 run.

## 2026-10-29 eMMC boot: why the card-out start failed, and the change
- No card-out start ever reached Linux [V]: the eMMC's /data had mount count 1, last mounted
  by the installer (2026-10-27 05:45), so init never ran from the eMMC.
- What was written was right [V]: U-Boot at 8 KiB = build E (353d56e6), eGON checksum valid;
  MBR signature 50463045, p1 to p4 as the card; boot.scr uses `mmc_bootdev`; extlinux roots
  50463045-02/-03; DTBs equal to the card's (dtc). EXT_CSD: RST_n_FUNCTION 0x01, BOOT_INFO 0x07.
- What differed from the vendor's working eMMC [V]: Android had PARTITION_CONFIG 0x48 and eGON
  at offset 0 of boot0 (and at sector 16); the install set 0 and cleared boot0. A whycan report
  fixed A33 eMMC boot by enabling partition boot [D]. Inference [U]: this boot ROM reads the eMMC
  only through the boot operation, which returns nothing with partition boot off.
- Reinstall with #157 [V]: `tfctl emmc install` exit 0, partitions read back equal, e2fsck
  clean, U-Boot at 8 KiB and at boot0 offset 0, PARTITION_CONFIG 0x48 (boot0, BOOT_ACK),
  BOOT_BUS_CONDITIONS 0x01; `tfctl emmc status` shows it. Slot b (2026.10.29) in both eMMC slots.
  boot0's saved copy is the zeros of the first install; the vendor's boot0 is in the Mac backup.

## 2026-10-29 First card-out start to Linux (PRs #159, #160)
- After #157, the card-out start got through the boot ROM, SPL and U-Boot from the eMMC [V]:
  boot.scr wrote /pf/uboot.env (pf_last=linux) but no kernel started, no `loadfail` mark was
  written, and U-Boot ended in ums. The same p1, copied to an image, boots to "Starting kernel"
  in QEMU (orangepi-pc), so the files and the script were fine [V].
- Two changes, both installed by `tfctl emmc install` (exit 0, read back equal) [V]:
  #159 U-Boot build E drives the eMMC at 4-bit, 26 MHz (ormarch E019: some units fail U-Boot
  eMMC reads at 8-bit, 52 MHz [D]); #160 boot.scr runs `mmc rescan` before sysboot and tries
  twice before writing `loadfail` (QEMU: both the normal and the no-kernel path [V]).
- Card out, frame on the Mac's USB, `reboot` from the prod MCP at 19:26:41 [V]: USB gone at
  :42, Linux gadget 0525:a4aa at :47, ping at :55. cmdline `root=PARTUUID=50463045-03`, only
  /dev/mmcblk2* present, `tfctl emmc status`: "running from: eMMC". cog, dropbear and the agent
  up, Wi-Fi joined (new DHCP lease 192.168.1.184; the eMMC /data has its own lease file).
  /pf/uboot.env: pf_boots=1, pf_last=linux, pf_prev_ok=no (the start before it never reached
  Linux, so that is expected).
- Which of the two changes fixed it is [U]: U-Boot's console is not visible with no card.
