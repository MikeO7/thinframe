# Tasks

Ordered. Each has a gate. Do not start the next until the gate is checked and logged in
`docs/log.md`. Items marked (owner) need a human at the frame.

## K. Kernel bumps
- [ ] K1 Move the kernel pin from the ormarch-tested 7.2.x tag to current mainline stable; re-apply
      the panel patch; verify on the frame. Then track stable in the weekly rebuild.

## 0. Repo hygiene
- [x] `shellcheck` passes on every `.sh` in CI (`.github/workflows/ci.yml`).
- [x] Pin `BUILDROOT_VERSION` in `image/build.sh` to the current LTS release and record the date.
      Record WPE/cog/kernel versions from the defconfig in `docs/log.md`.

## 0b. tfctl builds
- [x] `cargo build --release --target armv7-unknown-linux-musleabihf` (cargo-zigbuild or cross)
      produces a static binary. CI does it and fails if `tfctl` is over 1 MB.
- [x] `tfctl status` runs on the Mac host build and prints the config line.
- Gate: CI green, binary size logged.

## 1. Build the boot inputs (Mac, no frame)
- [x] `image/build.sh inputs` clones ormarch at `ORMARCH_COMMIT`, runs its `image/build.sh uboot`
      and copies out what Buildroot does not build itself: `u-boot-pf1007l-L.bin` (+ `.config`),
      `u-boot-qemu-opipc.bin` (QEMU test only), ormarch's `boot.cmd`, the board DTS and its
      min/pmic/full variants, and the v7.2.10 panel patch. Kernel, panel driver, DTBs and the
      aic8800 modules are built by Buildroot in TASK 2 from these sources (the Alpine-era plan
      of reusing the Arch kernel package and its `.ko` files no longer applies). Hashes written to
      `image/out/inputs.sha256`.
- Gate: `ls image/work/inputs` shows every file in the list above; hashes match a second run.

## 2. Buildroot image (Docker, cross-compile; no armv7 emulation needed)
- [x] `image/buildroot/` is a BR2_EXTERNAL tree: `external.desc`, `Config.in`, `external.mk`,
      `configs/pf1007l_defconfig`, `board/pf1007l/{linux.config,linux.fragment,patches/,dts/,
      rootfs-overlay -> ../../rootfs,post-image.sh,genimage.cfg,uboot/}`, `package/aic8800/`
      (kernel module package from the radxa source ormarch pinned), `package/tfctl/` (installs the
      prebuilt armv7 binary from tfctl/).
- [x] Defconfig: arm cortex-a7 neon-vfpv4 musl, BusyBox init, mainline kernel pinned to the ormarch
      7.2.x tag with the panel patch + DTS, mesa3d (lima, gbm, egl, gles), libdrm, libinput, eudev,
      wpewebkit (no wayland), cog with `BR2_PACKAGE_COG_PLATFORM_DRM`, iwd, dhcpcd, dropbear,
      e2fsprogs (resize2fs), zstd, ca-certificates, font (DejaVu or Noto), tzdata. No X, no
      wayland, no systemd (dbus only because iwd needs it).
- [x] `image/build.sh` runs Buildroot in a Docker container (`image/Dockerfile.br`) with `dl/` and
      `ccache` on bind-mounted volumes so rebuilds take minutes. First build: 1 to 3 h.
- [ ] Touch firmware: `PF1007L_TOUCH_FIRMWARE` copied into the overlay at build time, never committed.
- [x] DEBUG fragment adds evtest, libinput-tools, i2c-tools, strace.
      (`DEBUG=1 image/build.sh rootfs`, own output tree, version `-debug`: evtest + strace; libinput
      tools and BusyBox i2c* are in every image; i2c-tools itself needs BUSYBOX_SHOW_OTHERS.)
- Gate: `output/images/rootfs.ext4` under 200 MB; QEMU (vexpress or orangepi-pc board with the
  kernel built here) boots to a login prompt and dropbear answers.

## 3. Assemble the card image (A/B layout, see decisions.md)
- [x] `image/build.sh assemble`: MBR sig `50463037`, U-Boot at 8 KiB, p1 FAT32 `PFBOOT` 256 MiB
      (`zImage.a`, DTBs, `extlinux/extlinux.conf` with labels a/b, `boot.scr`, `uEnv.txt` with
      `slot=a tries=0`), p2 ext4 `rootA` 1 GiB, p3 ext4 `rootB` 1 GiB (empty), p4 ext4 `data` to end.
- [x] Root mounts read-only; `/etc/dropbear`, `/var/lib/iwd` are bind mounts from `data` (fstab in
      the overlay), `/etc/thinframe.conf` a symlink to `/data/etc/thinframe.conf`. Logs and the status page stay in RAM
      (decisions.md "BusyBox init"); the fallback `www/` is part of the root and of the overlay.
- [x] First boot grows p4 (not the root) to the card.
- [x] `image/build.sh rootfs` (post-image.sh) also emits `system-<ver>.tar` (zImage, rootfs.ext4.zst, sha256,
      version.json) for the container to ship.
- [x] boot.scr: read `slot`/`tries` from `uEnv.txt`; if `tries>0` decrement and write back; if it hits
      0, flip slot. (ormarch's boot.scr has the counter/state-file machinery; extend, don't rewrite.)
- Gate: image self-check in `build.sh` passes; `image/out/thinframe-sd.img.sha256` written.

## 4. First boot on the frame (owner)
- [ ] Write card with `scripts/write-card-macos.sh`. Power the frame from the Mac over USB-C.
- [ ] Within 60 s the Mac sees a USB serial device and a USB Ethernet interface (192.168.7.1
      host / 192.168.7.2 frame). `ssh frame@192.168.7.2` logs in.
- Gate: `uname -a` shows 7.2.10; `dmesg | grep -i -E 'panel|dsi'` shows the panel bound.

## 5. Display
- [ ] Console appears on the panel (DTB `full`, panel module loaded by `modules-load.d`).
- [ ] `tfctl kiosk` (tfctl/src/kiosk.rs): spawn cog, restart on exit with back-off, SIGTERM passthrough.
- [ ] `rc-service thinframe start` shows `www/index.html` full screen, landscape.
      Tune `COG_ROTATION` in `/etc/thinframe.conf` (expect 3, [U]).
- [ ] Memory: `free -m` before and after the page. Budget: <60 MB before, <300 MB after. Record it.
- Gate: photo of the page on the frame in `docs/img/`, `free -m` output in `docs/log.md`.

## 6. Touch
- [ ] `evtest` shows the Silead device. Firmware loaded (`dmesg | grep silead`).
- [ ] Tapping the four corners of `www/index.html` (it prints coordinates) matches. Fix with
      `LIBINPUT_CALIBRATION_MATRIX` in `image/rootfs/etc/udev/rules.d/90-touch.rules`.
      Upstream found a vertical flip is needed relative to the rotated output. [D]
- Gate: all four corners within 10 px.

## 7. Wi-Fi
- [ ] `tfctl wifi "SSID" "psk"` (tfctl/src/wifi.rs) writes `/var/lib/iwd/<SSID>.psk` and iwd connects.
      Modules `aic8800_bsp aic8800_fdrv` load from `modules-load.d` in that order. [D]
- [ ] Reboot with no USB data cable: ssh by the DHCP-assigned name/IP works (no avahi: the router
      learns the hostname from dhcpcd).
- Gate: three reboots in a row reconnect without help.

## 8. Update path
- [ ] `tfctl update` (tfctl/src/update.rs) fetches the main tarball with busybox wget, applies
      `image/rootfs/` and `www/`, replaces `/usr/bin/tfctl` from the latest GitHub release asset
      if newer, restarts the service. Logs to `/var/log/tf-update.log`.
- [ ] `scripts/deploy.sh host` pushes the working tree (not Git) for fast iteration.
- Gate: edit `www/index.html` on the Mac, `deploy.sh`, page changes within 5 s.

## 10. Thin client (frame side)
- [x] `tfctl agent` (tfctl/src/agent.rs): WebSocket client (RFC 6455 over TcpStream, text frames,
      ping/pong, masking), hello/heartbeat/ack out, config/action in; reconnect with backoff; HTTP
      GET /config + 60 s poll fallback when the socket is down; fallback URL when the server is gone
      for 3 attempts. Runs as a thread inside `tfctl kiosk` when SERVER_URL is set.
- [x] Unit tests for the frame codec (mask/unmask, fragment, close) and the message types.
- [x] Set system clock and TZ from the `hello` reply on every connect (no NTP on the frame).
      (agent.rs `on_hello`: `date -s @N` when off by >2 s, TZ copied from zoneinfo. Hardware unverified.)
- [x] `brightness` and `schedule` in config: backlight via sysfs, cog SIGSTOP/SIGCONT, touch wakes
      for 60 s, `screen` action, `screen_state` in heartbeat. Evaluated locally so it survives outages.
      (screen.rs + kiosk.rs `tick_schedule`; unit-tested windows, e2e acks `screen`. Backlight unverified.)
- [x] `tfctl set KEY VALUE` rewrites one key in /etc/thinframe.conf (used for first pairing).
- Gate: change rotation in /admin, frame re-renders within 1 s; stop the container, frame shows the
  fallback page within 3 reconnect attempts; start it, frame comes back and acks queued actions.

## 12. System (OS) update, A/B
- [x] Agent: on `system_version` change, stream `rootfs.ext4.zst` to the standby slot via `zstd -d | dd`,
      copy `zImage.<slot>`, verify both sha256, set `slot=<new> tries=3`, reboot.
      (tfctl/src/sysupdate.rs + slot.rs; `dd` only to the disk with MBR signature 50463037. Hardware unverified.)
- [x] First heartbeat after boot writes `tries=0` (slot good) and reports `slot`, `system_version`.
      (On every agent connect: `confirm_slot`, event `slot-confirmed`. `/etc/thinframe.system-version` must be in the image.)
- [x] Rollback test in sim qemu mode: ship a bundle with a broken init, watch U-Boot flip back after 3 tries.
- Gate: sim qemu frame goes 2026.10.a → .b → rollback → .c with no manual steps.

## 14. Flashing must be three steps (docs/flashing.md is the spec)
- [ ] `scripts/flash.sh`: release download with sha256, card discovery with guards, typed confirmation,
      one sudo write + readback, `provision.txt` onto PFBOOT, eject. `--settings-only` rewrites
      provision.txt only. Reuse ormarch `image/tools/write-card.py` guard logic.
- [x] `tfctl provision` on first boot (called from an OpenRC service before `thinframe`): read
      `/boot/provision.txt`, write iwd profile, `thinframe.conf` (SERVER_URL, FRAME_ID, FRAME_TOKEN,
      COG_ROTATION), `authorized_keys` from SSH_PUBKEY, hostname = FRAME_ID; delete the file; continue boot.
      (tfctl/src/provision.rs, `--boot-dir`; keys go to /home/frame/.ssh and /data/ssh/authorized_keys, the
      image bind-mounts /data/ssh over /etc/dropbear. Init must mount PFBOOT rw around the call.)
- [ ] On-screen status page (local file, shown by cog until the server answers): "connecting to Wi-Fi",
      "cannot reach http://…", with the IP once it has one. This is the only local page besides fallback.
      (tfctl/src/statuspage.rs: the kiosk opens cog on it before the agent starts, "joining <ssid>" /
      "no Wi-Fi configured" / "reaching <server>" / "cannot reach <server>" + IP, refreshed on every
      failed attempt; the first server answer switches to the dashboard. QEMU: written as "reaching".
      Pending: seen on the panel.)
- [x] 14b Linux variant of flash.sh (lsblk instead of diskutil). (`tests/flash-test.sh` covers discovery with a fake lsblk; the write path is untested.)
- Gate: a person who has not read the repo flashes a frame from docs/flashing.md alone, timed, no questions asked.

## 15. Boot experience (ours from the first lit pixel)
- [ ] Kernel cmdline: `quiet loglevel=0 vt.global_cursor_default=0 consoleblank=0 fbcon=map:9`
      so no console text ever appears on the panel; serial console stays on ttyGS0 for debugging.
- [ ] `tfctl splash`: open /dev/dri/card0 (dumb buffer, no GLES), fill background colour, blit a PNG
      from `/etc/thinframe/splash.png` (tiny PNG decoder, no crates) centred, draw one text line with
      an embedded bitmap font. Runs from OpenRC `boot` runlevel right after udev, before networking.
      Hands the DRM device over when cog starts.
      (Done: background + centred text line, SPLASH_BG/SPLASH_TEXT, rotation, kiosk draws it before cog and
      holds "cog missing" if cog is absent; PNG decoder (tfctl/src/png.rs, own inflate, 8-bit non-interlaced) blends
      `/etc/thinframe/splash.png` above the text; tfctl/src/{drm,splash,font,png}.rs. Pending: ship a logo, hardware check.)
- [ ] Local status page (`/run/thinframe/status.html`) shows the same logo and live text:
      "joining <ssid>", "reaching <server>", IP, then hands over to the dashboard.
      (Software done with the item in 14; the page shows the splash logo. Pending: seen on the panel.)
- [x] Config fields `splash_text` and `splash_png` (base64 or artifact name) so the server/MCP can change
      the logo; tfctl writes them to the data partition.
      (Plus `splash_bg`. Server validates #rrggbb and a PNG of at most 48 KiB; REST and MCP
      `set_frame_config`; tfctl stores `/data/thinframe/splash.png` and SPLASH_* in the conf, drawn
      from the next kiosk start. QEMU: pushed through the server and found on /data.)
- Gate: power on → dark ≤10 s → logo → status text → dashboard, no kernel or console text visible; photo in docs/img/.

## 16. One-press pairing (no token typing)
- [x] Server: `POST /api/pair`, `POST /pair/{id}/accept` (the button), `GET /api/pair/{id}?nonce=`
      (token once), per-frame token hash in the store, scoped auth, "forget pairing" on /admin.
      (server/internal/api/pair.go, tests pair_test.go.)
- [x] tfctl: without FRAME_TOKEN the agent pairs first and shows the PAIR page; a 401 clears the token.
      (tfctl/src/pair.rs.) flash.sh: the token prompt may be left empty.
- [x] Gate: in QEMU, a frame flashed without a token shows PAIR, one tap pairs it, the dashboard loads,
  a reboot goes straight to the dashboard, a second pairing request is refused, "forget pairing"
  brings the button back. Evidence in docs/log.md (2026-10-20, docs/img/pair-qemu-*.png).
- [ ] On the frame: the same with a finger on the real panel (touch axes, TASK 6).

## 17. Wi-Fi first, on the frame's screen
- [x] tfctl: a frame with a radio and no connection (nothing configured, or the configured network
      not up within 45 s) shows a touch page before pairing: networks in range (iwctl scan), a
      password page with an on-screen keyboard, joining, connected. Served on 127.0.0.1:8091 by the
      agent, only while it is on screen. A wrong password comes back to the keyboard with the error.
      (tfctl/src/wifisetup.rs, radio in wifi.rs.) flash.sh: the Wi-Fi prompt may be left empty.
- [x] Sim: the QEMU image has mac80211_hwsim radios, "Groovy Wifi" (WPA2) and "Cafe" (open)
      (qemu-overlay/etc/init.d/S35simwifi).
- [x] Gate: in QEMU, a frame flashed with neither Wi-Fi nor token shows the Wi-Fi page, a wrong
      password is refused, the right one joins, then PAIR shows, one tap pairs, the dashboard loads,
      and a reboot goes straight to the dashboard. Evidence in docs/log.md (2026-10-21,
      docs/img/wifi-qemu-*.jpg).
- [x] Image: iwctl exists (`BR2_PACKAGE_READLINE=y`; it was missing, see log 2026-10-21; seen in the
      QEMU build, same defconfig as the frame).
- [ ] On the frame: the same with the AIC8800 and a real router (TASK 7).

## 11. Auto-update from the container (frame side)
- [x] Agent compares `software_version` from config with `/etc/thinframe.version`; on change downloads
      `tfctl-armv7` and `overlay.tar.gz` from `/api/artifacts/`, checks sha256 from config, installs
      (`tfctl.prev` kept), applies overlay with the exclusions, writes the version, restarts service.
      (tfctl/src/update.rs; restart = tfctl stops cog and exits 0, the inittab respawn runs the new binary.)
- [x] Rollback: if tfctl exits twice within 60 s after an update, service start_pre swaps `tfctl.prev` back.
      (Done inside tfctl itself: `update::record_start` at kiosk start, starts log in /data/thinframe/starts,
      event `software-rollback` sent on the next connect. No shell in start_pre needed.)
- Gate: bump a version in the container, rebuild, frame updates itself within one poll and heartbeat
  reports the new version; break the binary on purpose, frame rolls back.

## S. Server (any box with Docker)
- [x] S0 CI job for `server/`: gofmt, `go vet`, `go build`, `go test ./...` (multi-arch image: release.yml on tags).
- [x] S1 `server/`: stdlib ServeMux serving static/ at /frame/{id}, config GET/PUT, heartbeat POST,
      JSON files in /data (internal/store). `go test` covers the config merge and one-shot action clearing.
- [x] S1b `/api/frames/{id}/ws`: one goroutine per frame, hub keyed by id, push `config` on PUT and
      `action` on request, resend unacked actions on reconnect, mark offline on socket close. Admin,
      REST and MCP all go through the hub so a change reaches the frame in under a second.
- [x] S1c `/api/stream/{id}` SSE for the dashboard page's live data (separate from the control socket).
- [x] S2 `/admin`: plain HTML table of frames (id, ip, last seen, mem, temp) + edit form.
- [x] S3 Dockerfile builds a static Go binary for amd64/arm64 (scratch, 11 MB); `docker compose up`
      works; release.yml pushes ghcr.io/mikeo7/thinframe-server on tags.
- [x] S4 FRAME_TOKEN bearer auth on /api and /mcp (frame page and admin stay LAN-open).
- [x] S7 MCP: `/mcp` streamable HTTP with bearer auth, tools from decisions.md table, each calling the
      same handler as REST. Gate: from Claude Code, "rotate frame sim to 1" changes the sim screenshot.
- [x] S8 Weekly `release.yml`: rebuild the Buildroot image on the pinned LTS branch + pinned kernel, QEMU boot test, publish
      container with the new `system-<date>.tar` embedded. Dependabot for Go (server), Cargo (tfctl) and GitHub Actions.
      (Cron Mondays 04:17 UTC; run 39494363118 green: release v2026.10.20 carries qemu-test.txt and
      system-v2026.10.20.tar, ghcr.io/mikeo7/thinframe-server:v2026.10.20 pushed. .github/dependabot.yml
      covers gomod server+sim, cargo, github-actions, docker.)
- [x] S9 Frames auto-register on first heartbeat with a valid token (no pre-creation in admin).
- [x] S10 `release.yml` publishes `thinframe-sd.img.zst` + `.sha256` as a GitHub release asset on tag,
      so flash.sh never needs a local build. (v2026.10.20: thinframe-sd.img.zst 64 MB + .sha256.)
- [x] S6 Artifacts: `scripts/make-artifacts.sh` (run by release.yml after cross-building tfctl) stages
      `tfctl-armv7`, `overlay.tar.gz`, `versions.json` into `server/artifacts/`, which the Dockerfile copies to
      `/artifacts`; server exposes them at `/api/artifacts/` with sha256 and sets `software_version` in config.
      (tests/e2e-native.sh checks the sidecar hash and the config field; Go test `TestConfigCarriesArtifactVersions`
      checks config versions/sums and token-gated file + sidecar serving. Server side is done; the Dockerfile has no
      tfctl cross-build stage, release.yml cross-builds before `docker build`, and that tag build is untested.)
- [x] S5 Widgets + layout: `layout.json` per frame, `html/template` renderer, widget library
      (clock, photo, weather, calendar, text, iframe) in `internal/widgets/`, `/admin` layout form,
      MCP `set_layout`/`list_widgets`. Gate: two frames, two layouts, one container.
- [x] S5b Photo adapter: `PHOTOS_DIR` scan, resize to 1280x800 and 800x1280, EXIF strip, cache,
      `/api/data/photo/next`. Interface `PhotoSource` so Immich/Google/iCloud can be added later.
- [x] S5c `hello` reply carries `time_unix` and `tz`; weather and calendar adapters (open-meteo, public .ics via
      `CALENDAR_ICS_URL`, no recurrence expansion) feed widgets over SSE (on connect + every 10 min) and
      `GET /api/data/weather|calendar`; the page never calls third parties directly. Covered by
      `internal/api/data_test.go` with httptest fakes (log 2026-10-20).
- Gate: two frames with different ids show different pages from one container.

## M. Simulator (second container, dev only)
- [x] M1 `sim/` fast mode: Debian trixie cog 0.18.4 headless + WPEWebDriver + real tfctl agent as
      frame `sim`, screenshots to `sim/out/frame.png`. Gate passed 2026-10-18: rotation 0 via REST
      gave a 375x600 portrait shot, rotation 3 gave 800x500 landscape; a layout PUT changed the shot.
- [ ] M2 `sim/qemu/`: the REAL Buildroot image under `qemu-system-arm -M virt` (same rootfs.ext4,
      same tfctl, same cog on DRM): a `pf1007l_qemu_defconfig` that reuses the main defconfig with
      the virt kernel fragment (virtio-gpu for /dev/dri/card0, virtio-input touchscreen, virtio-net,
      virtio-blk) and mesa swrast for GLES; `sim/qemu/run.sh` boots the A/B SD image with user-mode
      net (`hostfwd` 22 and the frameserver reachable), screenshots via the QEMU monitor `screendump`
      every N s into `sim/out/frame.png`, touch via QEMU `input-send-event`.
      Gate: frame `sim-qemu` appears in /admin from the real image; screendump shows the dashboard;
      an A/B system update + rollback runs end-to-end inside QEMU. Slow (TCG), for correctness only.
      2026-10-20: frame registers and the screendump shows the dashboard (two bugs fixed, docs/log.md);
      A/B inside -M virt still open (direct kernel boot, no U-Boot).
- [x] M3 `docker compose --profile sim up --build` from the repo root starts both containers (README).
- [x] M4 `tests/e2e.sh`: start both, wait for heartbeat, set config via MCP, assert screenshot changed.
      (`tests/e2e.sh` with compose.yml; CI job `e2e-sim`. `tests/e2e-native.sh` covers the no-Docker path.)

## 13. Smoothness (measure, then tune)
- [ ] Confirm GPU compositing in WPE on lima (`WEBKIT_SHOW_FPS=1`); fix before anything else.
- [ ] cpufreq governor experiment: performance vs ondemand, record touch latency and idle temp.
- [ ] libinput: no gestures/tap timeouts; page uses pointer events and `touch-action: none`.
- [ ] No swap/zram; WebProcess memory cap env; `ps` audit shows only the allowed process list.
- [ ] `tfctl bench`: replay a tap, record touch→first-paint; beacon from page; shown in /admin.
- Gate: touch→response under 50 ms, 60 fps in a 10 s swipe test, idle CPU under 5 %, logged in docs/log.md.

## E. Install to internal storage (later; needs an explicit go from the owner)
- [ ] E1 `tfctl backup-emmc`: stream the whole eMMC over the control socket or USB to the server
      (`/data/backups/<frame>-emmc.img.zst`), verify sha256. Refuses to continue without it.
- [ ] E2 `tfctl install-emmc`: write U-Boot to the eMMC boot offset, create the same 4-partition
      layout, copy the active slot and data partition, update `extlinux`/`uEnv` for mmcblk0 vs mmcblk1
      naming, verify, print "remove the card and reboot".
- [ ] E3 FEL recovery doc: sunxi-tools on the Mac, `sunxi-fel` + the SD image to boot without a card,
      and how to restore the backup to return to Android.
- [ ] E4 Boot order check: with a card inserted the card still wins (boot ROM order), so a rescue card
      always works.
- Gate: frame boots thinframe with no card; rescue card boots; backup restores Android on a test unit.

## 9. Polish (optional, after everything above)
- [ ] Boot splash: `extlinux.conf` `quiet loglevel=3 vt.global_cursor_default=0`, blank
      console via `consoleblank`.
- [x] Health check in `tfctl kiosk`: detect a hung WebProcess and restart cog.
      (Done server-side: the page pings `/alive` from a JS timer; a page that pinged during this agent
      connection and is quiet for PAGE_STALE (3 min) gets one `restart-kiosk`. server/internal/api/
      watchdog.go; tests/e2e.sh freezes the sim WebProcess with SIGSTOP. On the panel: not seen.)
- [x] `tfctl status` (tfctl/src/status.rs): read /proc and /sys, no shelling out except iwctl.
      (IPs from /proc/net/fib_trie, USB-C address last; QEMU check `tfctl status` shows 10.0.2.15.)
- [x] Size audit: `du -sh` of the root, `ps` list at idle. Remove anything not in the README budget.
      (docs/log.md 2026-10-20: root 169 MiB, crond/seedrng/syslogd/klogd removed, DejaVu Serif and
      Condensed dropped, idle `ps` = README list + dbus for iwd. Re-check on the frame with cog up.)
- [ ] GitHub Actions builds tfctl and the image on a tag and attaches both to a release.
- [ ] Speaker, accelerometer rotation, light sensor: see upstream `docs/hardware.md`. Not now.
