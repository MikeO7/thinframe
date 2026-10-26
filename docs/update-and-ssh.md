# Getting in and changing things

New frame? Start at [flashing.md](flashing.md). This page is for after that.

## Paths in
1. Wi-Fi: `ssh frame@<frame name>` (hostname = FRAME_ID, router DNS) or the IP shown in `/admin`.
   Key: `~/.thinframe/frame_id_ed25519` on the Mac that ran `flash.sh` (or
   `image/secrets/frame_id_ed25519` for a local build). Dropbear, key only, no root login. [U]
2. USB-C data cable to the Mac: the kernel's built-in CDC gadget gives a USB Ethernet device
   (frame 192.168.7.2, the Mac gets 192.168.7.1 from the frame's udhcpd) and the kernel log
   on `ttyGS0`: `ssh frame@192.168.7.2`, or `screen /dev/tty.usbmodem* 115200` (kernel messages only,
   no login prompt: `/dev/console` is tty1). [U]
3. Pull the card: p1 (`PFBOOT`) is FAT32, readable on the Mac: `uEnv.txt` (slot, tries,
   `pf_mode`), `extlinux/slot-a.conf` / `slot-b.conf`, and `provision.txt` (dropped there by
   `flash.sh --settings-only`, applied and deleted on the next boot).

`frame` is an ordinary user: there is no sudo or doas on the image. Everything that changes the
frame goes through the server (config, actions, updates), `provision.txt`, or a new card.

## Changing the page
Edit `server/static/` or the layout in `/admin`; the frame follows on its control socket.
The frame's own `www/` is only the offline fallback.

## Changing config
`/etc/thinframe.conf` is a symlink to the data partition (`/data/etc/thinframe.conf`) and
written by `tfctl` (config pushed from the server, `provision.txt`, `tfctl set` as root). The
image default is `image/rootfs/etc/thinframe.conf`. The server token is not in it: it sits in
`/data/etc/thinframe.token`, root only, so `frame` can read the conf but not the secret.

## Wi-Fi
Put `WIFI_SSID`/`WIFI_PSK` in `provision.txt` on PFBOOT and reboot, or (as root) `tfctl wifi
"My SSID" "passphrase"`. Profiles live in `/var/lib/iwd/` (bound from `/data/iwd`).

## Debug
Without touching the frame:
- **report**: send the `report` action (`/admin`, `POST /api/frames/<id>/actions`, or the MCP
  tool `frame_action`). The frame posts one text file: tfctl versions and slot, `thinframe.conf`,
  `uEnv.txt`/`.bak`, mounts, `df`, `ps`, `dmesg` (every `tfctl boot` step line is in there), the
  Wi-Fi station, this boot's log and the previous boot's log. Read it in `/admin` (reports), at
  `GET /api/frames/<id>/reports/latest`, or with the MCP tool `get_report`. The server keeps the
  last 20 per frame under `DATA_DIR/reports/<id>/`.
- **debug mode**: `debug: true` in the frame's config (the checkbox in `/admin`, or
  `set_frame_config`). tfctl then logs every socket message both ways, every config key it
  writes and cog's command line, runs cog with `G_MESSAGES_DEBUG=all`, and posts a report on its
  first server contact after every (re)start. Persisted as `DEBUG="1"` in `thinframe.conf`, so it
  survives reboots until switched off. Chatty: the log is capped at 1 MiB in RAM, so leave it on
  only while hunting something.

Pushing a change to a connected frame: `scripts/dev-push.sh` builds tfctl for armv7, stages it
as `dev-<stamp>-<sha>` in the server's artifacts directory (the server reads
`versions.json` on every config request, no restart) and, with `SERVER_URL`, `FRAME_ID` and
`FRAME_TOKEN` set, sends `update-software`. The frame swaps the binary within seconds; a build
that crash-loops is rolled back by tfctl itself. A native server for that loop:
`cd server && PORT=8080 DATA_DIR=~/.thinframe/data ARTIFACTS_DIR=~/.thinframe/artifacts FRAME_TOKEN=... go run ./cmd/frameserver`.

A hung kernel or a wedged tfctl reboots itself: tfctl arms the A33's hardware watchdog (16 s)
and pets it every 5 s; init's respawn re-arms it. Nothing to do at the frame.

On the frame (ssh as `frame`):
- `tfctl status`: uptime, IP, memory, cog pid, URL.
- `/var/log/thinframe.log` (RAM): tfctl and cog output. A copy is kept on the card every 15 s:
  `/data/thinframe/log/current.log`, and `previous.log` from the boot before, so a reboot or a
  crash loop leaves its trace. `dmesg` for drivers (`dmesg | grep -E 'panel|dsi|silead|aic'`).
- Console-only rescue: in `extlinux/slot-a.conf` on PFBOOT, change `DEFAULT thinframe` to
  `DEFAULT min` (minimal DTB, kernel messages on the USB serial port).
