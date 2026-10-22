# Getting in and changing things

New frame? Start at [flashing.md](flashing.md). This page is for after that.

## Paths in
1. Wi-Fi: `ssh frame@<frame name>` (hostname = FRAME_ID, router DNS) or the IP shown in `/admin`.
   Key: `~/.thinframe/frame_id_ed25519` on the Mac that ran `flash.sh` (or
   `image/secrets/frame_id_ed25519` for a local build). Dropbear, key only, no root login. [U]
2. USB-C data cable to the Mac: the kernel's built-in CDC gadget gives a USB Ethernet device
   (frame 192.168.7.2, the Mac gets 192.168.7.1 from the frame's udhcpd) and a serial console
   on `ttyGS0`: `ssh frame@192.168.7.2`, or `screen /dev/tty.usbmodem* 115200`. [U]
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
- `tfctl status`: uptime, IP, memory, cog pid, URL.
- `/var/log/thinframe.log` (RAM): tfctl and cog output. `dmesg` for drivers
  (`dmesg | grep -E 'panel|dsi|silead|aic'`).
- Console-only rescue: in `extlinux/slot-a.conf` on PFBOOT, change `DEFAULT thinframe` to
  `DEFAULT min` (minimal DTB, kernel messages on the USB serial port).
