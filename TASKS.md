# Tasks

Ordered. Each has a gate. Do not start the next until the gate is checked and logged in
`docs/log.md`. Items marked (owner) need a human at the frame.

## 0. Repo hygiene
- [ ] `shellcheck` passes on every `.sh` in CI (`.github/workflows/ci.yml`).
- [ ] Pin `ALPINE_VERSION` and `ALPINE_MINIROOTFS_SHA256` in `image/build.sh` to the current
      stable armv7 minirootfs from https://alpinelinux.org/downloads/ . Record the date.

## 0b. tfctl builds
- [ ] `cargo build --release --target armv7-unknown-linux-musleabihf` (cargo-zigbuild or cross)
      produces a static binary. CI does it and fails if `tfctl` is over 1 MB.
- [ ] `tfctl status` runs on the Mac host build and prints the config line.
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

## 3. Assemble the card image
- [ ] `image/build.sh assemble` writes MBR (signature `50463037`), U-Boot at 8 KiB, FAT32
      `PFBOOT` at 4 MiB (256 MiB) with zImage, DTB, `extlinux/extlinux.conf`, `boot.scr`,
      `uEnv.txt`; ext4 `afroot` with the rootfs. Root by `PARTUUID=50463037-02`.
- [ ] First-boot service `/etc/local.d/10-growroot.start` grows p2 to the card and removes itself.
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
- [ ] `tfctl agent` (tfctl/src/agent.rs): config poll, heartbeat, actions, fallback URL. Runs as a
      thread inside `tfctl kiosk` when SERVER_URL is set.
- [ ] `tfctl set KEY VALUE` rewrites one key in /etc/thinframe.conf (used for first pairing).
- Gate: change rotation in /admin, frame re-renders within one poll interval; stop the container,
  frame shows the fallback page within 3 polls; start it, frame comes back.

## S. Server (any box with Docker)
- [ ] S1 `server/`: axum skeleton serving static/ at /frame/<id>, config GET/PUT, heartbeat POST,
      JSON files in /data. `cargo test` covers the config merge and one-shot action clearing.
- [ ] S2 `/admin`: plain HTML table of frames (id, ip, last seen, mem, temp) + edit form.
- [ ] S3 Dockerfile builds static musl for amd64 and arm64; `docker compose up` works; GH Action
      pushes ghcr.io/mikeo7/thinframe-server on tag.
- [ ] S4 optional FRAME_TOKEN bearer auth for off-LAN use.
- [ ] S5 `/api/data/*` adapters the dashboard needs (first: photos dir, clock, weather). Each one a
      small module; the page never calls third parties directly.
- Gate: two frames with different ids show different pages from one container.

## 9. Polish (optional, after everything above)
- [ ] Boot splash: `extlinux.conf` `quiet loglevel=3 vt.global_cursor_default=0`, blank
      console via `consoleblank`.
- [ ] Health check in `tfctl kiosk`: detect a hung WebProcess and restart cog.
- [ ] `tfctl status` (tfctl/src/status.rs): read /proc and /sys, no shelling out except iwctl.
- [ ] Size audit: `du -sh` of the root, `ps` list at idle. Remove anything not in the README budget.
- [ ] GitHub Actions builds tfctl and the image on a tag and attaches both to a release.
- [ ] Speaker, accelerometer rotation, light sensor: see upstream `docs/hardware.md`. Not now.
