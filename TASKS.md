# Tasks

Ordered. Each has a gate. Do not start the next until the gate is checked and logged in
`docs/log.md`. Items marked (owner) need a human at the frame.

## K. Kernel on Alpine (later; removes the Arch kernel dependency)
- [ ] K1 Build the ormarch panel patch + aic8800 modules against Alpine's `linux-lts` with `abuild`
      in a Docker armv7 container; publish as apk in a repo the image build pulls from.
- Gate: same frame boots on the Alpine kernel; weekly rebuild tracks Alpine kernel updates.

## 0. Repo hygiene
- [ ] `shellcheck` passes on every `.sh` in CI (`.github/workflows/ci.yml`).
- [ ] Pin `ALPINE_VERSION` and `ALPINE_MINIROOTFS_SHA256` in `image/build.sh` to the current
      stable armv7 minirootfs from https://alpinelinux.org/downloads/ . Record the date.

## 0b. tfctl builds
- [x] `cargo build --release --target armv7-unknown-linux-musleabihf` (cargo-zigbuild or cross)
      produces a static binary. CI does it and fails if `tfctl` is over 1 MB.
- [x] `tfctl status` runs on the Mac host build and prints the config line.
- Gate: CI green, binary size logged.

## 1. Build the boot inputs (Mac, no frame)
- [ ] `image/build.sh inputs` clones ormarch at `ORMARCH_COMMIT`, runs its `kernel/build.sh`
      and `wifi/build.sh`, and copies out: `u-boot-pf1007l-L.bin`, `boot.scr`, the 7.2.10
      kernel package, `sun8i-a33-pf1007l-full.dtb`, panel `.ko`, aic8800 `.ko` files and the
      `aic8800DC` firmware dir. Hashes written to `image/out/inputs.sha256`.
- Gate: `ls image/work/inputs` shows every file in the list above; hashes match a second run.

## 2. Alpine rootfs (Mac, Docker linux/arm/v7)
- [ ] `image/build.sh rootfs` unpacks the minirootfs, copies `image/rootfs/` over it, then
      `apk add` the package list in `image/packages.txt`, installs the kernel modules dir
      from the Arch package under `/lib/modules/7.2.10-1-armv7-ARCH/`, adds the extra `.ko`
      files to `.../updates/`, runs `depmod`, installs firmware, enables OpenRC services:
      `devfs dmesg mdev`→ replace mdev with `udev udev-trigger udev-settle` (eudev; libinput
      needs it), `hostname modules sysctl networking iwd dhcpcd chronyd sshd seatd thinframe`.
- [ ] Check: Alpine's `kmod` loads `.ko.zst` (Arch compresses modules). If not, decompress
      them during the build. [U]
- [ ] Check: Alpine armv7 has `cog` built with the `drm` platform. Run
      `apk info -L cog | grep -i drm` inside the container. If absent, see docs/decisions.md
      fallback (cage + cog wayland) and update `image/rootfs/etc/conf.d/thinframe`.
- [ ] User `frame` (uid 1000, groups wheel video input seat), passwordless doas, key-only SSH,
      host key from `image/secrets/` so the fingerprint never changes between builds.
- Gate: `image/build.sh test` boots the rootfs in QEMU (`-M orangepi-pc`, same trick ormarch
  uses) to an OpenRC login prompt and `ssh` works to the guest.

## 3. Assemble the card image (A/B layout, see decisions.md)
- [ ] `image/build.sh assemble`: MBR sig `50463037`, U-Boot at 8 KiB, p1 FAT32 `PFBOOT` 256 MiB
      (`zImage.a`, DTBs, `extlinux/extlinux.conf` with labels a/b, `boot.scr`, `uEnv.txt` with
      `slot=a tries=0`), p2 ext4 `rootA` 1 GiB, p3 ext4 `rootB` 1 GiB (empty), p4 ext4 `data` to end.
- [ ] Root mounts read-only; `/etc/thinframe.conf`, `/etc/ssh`, `/var/lib/iwd`, `/var/log`,
      `/var/lib/thinframe` are bind mounts from `data` (fstab in the overlay).
- [ ] First boot grows p4 (not the root) to the card.
- [ ] `image/build.sh bundle` also emits `system-<ver>.tar` (zImage, rootfs.ext4.zst, sha256,
      version.json) for the container to ship.
- [ ] boot.scr: read `slot`/`tries` from `uEnv.txt`; if `tries>0` decrement and write back; if it hits
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
- [ ] Set system clock and TZ from the `hello` reply on every connect (no NTP on the frame).
- [ ] `brightness` and `schedule` in config: backlight via sysfs, cog SIGSTOP/SIGCONT, touch wakes
      for 60 s, `screen` action, `screen_state` in heartbeat. Evaluated locally so it survives outages.
- [x] `tfctl set KEY VALUE` rewrites one key in /etc/thinframe.conf (used for first pairing).
- Gate: change rotation in /admin, frame re-renders within 1 s; stop the container, frame shows the
  fallback page within 3 reconnect attempts; start it, frame comes back and acks queued actions.

## 12. System (OS) update, A/B
- [ ] Agent: on `system_version` change, stream `rootfs.ext4.zst` to the standby slot via `zstd -d | dd`,
      copy `zImage.<slot>`, verify both sha256, set `slot=<new> tries=3`, reboot.
- [ ] First heartbeat after boot writes `tries=0` (slot good) and reports `slot`, `system_version`.
- [ ] Rollback test in sim qemu mode: ship a bundle with a broken init, watch U-Boot flip back after 3 tries.
- Gate: sim qemu frame goes 2026.10.a → .b → rollback → .c with no manual steps.

## 14. Flashing must be three steps (docs/flashing.md is the spec)
- [ ] `scripts/flash.sh`: release download with sha256, card discovery with guards, typed confirmation,
      one sudo write + readback, `provision.txt` onto PFBOOT, eject. `--settings-only` rewrites
      provision.txt only. Reuse ormarch `image/tools/write-card.py` guard logic.
- [ ] `tfctl provision` on first boot (called from an OpenRC service before `thinframe`): read
      `/boot/provision.txt`, write iwd profile, `thinframe.conf` (SERVER_URL, FRAME_ID, FRAME_TOKEN,
      COG_ROTATION), `authorized_keys` from SSH_PUBKEY, hostname = FRAME_ID; delete the file; continue boot.
- [ ] On-screen status page (local file, shown by cog until the server answers): "connecting to Wi-Fi",
      "cannot reach http://…", with the IP once it has one. This is the only local page besides fallback.
- [ ] 14b Linux variant of flash.sh (lsblk instead of diskutil).
- Gate: a person who has not read the repo flashes a frame from docs/flashing.md alone, timed, no questions asked.

## 15. Boot experience (ours from the first lit pixel)
- [ ] Kernel cmdline: `quiet loglevel=0 vt.global_cursor_default=0 consoleblank=0 fbcon=map:9`
      so no console text ever appears on the panel; serial console stays on ttyGS0 for debugging.
- [ ] `tfctl splash`: open /dev/dri/card0 (dumb buffer, no GLES), fill background colour, blit a PNG
      from `/etc/thinframe/splash.png` (tiny PNG decoder, no crates) centred, draw one text line with
      an embedded bitmap font. Runs from OpenRC `boot` runlevel right after udev, before networking.
      Hands the DRM device over when cog starts.
- [ ] Local status page (`/var/lib/thinframe/www/status.html`) shows the same logo and live text:
      "joining <ssid>", "reaching <server>", IP, then hands over to the dashboard.
- [ ] Config fields `splash_text` and `splash_png` (base64 or artifact name) so the server/MCP can change
      the logo; tfctl writes them to the data partition.
- Gate: power on → dark ≤10 s → logo → status text → dashboard, no kernel or console text visible; photo in docs/img/.

## 11. Auto-update from the container (frame side)
- [ ] Agent compares `software_version` from config with `/etc/thinframe.version`; on change downloads
      `tfctl-armv7` and `overlay.tar.gz` from `/api/artifacts/`, checks sha256 from config, installs
      (`tfctl.prev` kept), applies overlay with the exclusions, writes the version, restarts service.
- [ ] Rollback: if tfctl exits twice within 60 s after an update, service start_pre swaps `tfctl.prev` back.
- Gate: bump a version in the container, rebuild, frame updates itself within one poll and heartbeat
  reports the new version; break the binary on purpose, frame rolls back.

## S. Server (any box with Docker)
- [ ] S0 CI job for `server/`: `go vet`, `go test ./...`, `staticcheck`, multi-arch Docker build.
- [ ] S1 `server/`: stdlib ServeMux serving static/ at /frame/{id}, config GET/PUT, heartbeat POST,
      JSON files in /data (internal/store). `go test` covers the config merge and one-shot action clearing.
- [ ] S1b `/api/frames/{id}/ws`: one goroutine per frame, hub keyed by id, push `config` on PUT and
      `action` on request, resend unacked actions on reconnect, mark offline on socket close. Admin,
      REST and MCP all go through the hub so a change reaches the frame in under a second.
- [ ] S1c `/api/stream/{id}` SSE for the dashboard page's live data (separate from the control socket).
- [ ] S2 `/admin`: plain HTML table of frames (id, ip, last seen, mem, temp) + edit form.
- [ ] S3 Dockerfile builds static musl for amd64 and arm64; `docker compose up` works; GH Action
      pushes ghcr.io/mikeo7/thinframe-server on tag.
- [ ] S4 optional FRAME_TOKEN bearer auth for off-LAN use.
- [ ] S7 MCP: `/mcp` streamable HTTP with bearer auth, tools from decisions.md table, each calling the
      same handler as REST. Gate: from Claude Code, "rotate frame sim to 1" changes the sim screenshot.
- [ ] S8 Weekly `release.yml`: rebuild image on Alpine stable + pinned kernel, QEMU boot test, publish
      container with the new `system-<date>.tar` embedded. Dependabot for Go (server), Cargo (tfctl) and GitHub Actions.
- [ ] S9 Frames auto-register on first heartbeat with a valid token (no pre-creation in admin).
- [ ] S10 `release.yml` publishes `thinframe-sd.img.zst` + `.sha256` as a GitHub release asset on tag,
      so flash.sh never needs a local build.
- [ ] S6 Artifacts: Dockerfile stage cross-builds `tfctl` for armv7 and tars `image/rootfs/`;
      server exposes them at `/api/artifacts/` with sha256 and sets `software_version` in config.
- [ ] S5 Widgets + layout: `layout.json` per frame, `html/template` renderer, widget library
      (clock, photo, weather, calendar, text, iframe) in `internal/widgets/`, `/admin` layout form,
      MCP `set_layout`/`list_widgets`. Gate: two frames, two layouts, one container.
- [ ] S5b Photo adapter: `PHOTOS_DIR` scan, resize to 1280x800 and 800x1280, EXIF strip, cache,
      `/api/data/photo/next`. Interface `PhotoSource` so Immich/Google/iCloud can be added later.
- [ ] S5c `hello` reply carries `time_unix` and `tz`; weather and calendar adapters behind
      `/api/data/*`; the page never calls third parties directly.
- Gate: two frames with different ids show different pages from one container.

## M. Simulator (second container, dev only)
- [ ] M1 `sim/` `SIM_MODE=wpe`: headless WPE + native tfctl agent as frame `sim`, screenshots to
      `sim/out/frame.png`. Gate: change rotation in /admin, next screenshot is rotated.
- [ ] M2 `SIM_MODE=qemu`: boots `image/out/thinframe-sd.img` under qemu-system-arm with the orangepi-pc
      board and a DTB that has no panel; serial on the container log; user-mode net forwards 22.
      Gate: `ssh -p 2222 frame@localhost` into the emulated frame, heartbeat visible in /admin.
- [ ] M3 `docker compose --profile sim up` from the repo root starts both containers; README one-liner.
- [ ] M4 `tests/e2e.sh`: start both, wait for heartbeat, set config via MCP, assert screenshot changed.

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
- [ ] Health check in `tfctl kiosk`: detect a hung WebProcess and restart cog.
- [ ] `tfctl status` (tfctl/src/status.rs): read /proc and /sys, no shelling out except iwctl.
- [ ] Size audit: `du -sh` of the root, `ps` list at idle. Remove anything not in the README budget.
- [ ] GitHub Actions builds tfctl and the image on a tag and attaches both to a release.
- [ ] Speaker, accelerometer rotation, light sensor: see upstream `docs/hardware.md`. Not now.
