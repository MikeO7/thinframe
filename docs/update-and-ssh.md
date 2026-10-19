# Getting in and changing things

New frame? Start at [flashing.md](flashing.md). This page is for after that.

## Paths in
1. Wi-Fi: `ssh frame@<frame name>` (hostname = FRAME_ID, router DNS) or the IP shown in `/admin`.
   Key: `~/.thinframe/frame_id_ed25519` on the Mac that ran `flash.sh`.
2. USB-C data cable to the Mac: the frame appears as a USB Ethernet device. Frame is
   192.168.7.2, Mac side gets 192.168.7.1 via the frame's dhcpd. `ssh frame@192.168.7.2`.
   Also a USB serial console: `screen /dev/tty.usbmodem* 115200`.
3. Pull the card: p1 is FAT32, readable on the Mac. Edit `extlinux.conf` or `uEnv.txt` there.

## Changing the page
- Fast loop, no Git: `scripts/deploy.sh frame@thinframe` copies `www/`, `image/rootfs/` and a
  freshly built `tfctl` to the frame over ssh/tar and restarts the service.
- Proper: push to `main`, then on the frame `doas tfctl update`. It fetches the main tarball, applies it, restarts.

## Changing config
`/etc/thinframe.conf` on the frame (source of truth: `image/rootfs/etc/thinframe.conf`):

```
FRAME_URL="file:///var/lib/thinframe/www/index.html"
COG_PLATFORM="drm"
COG_ROTATION="3"          # quarter turns; verify on hardware
COG_EXTRA_ARGS=""
```

`rc-service thinframe restart` after edits.

## Wi-Fi
`doas tfctl wifi "My SSID" "passphrase"` writes an iwd profile and reconnects. Profiles live in
`/var/lib/iwd/`. Remove one to forget it.

## Debug
- `tfctl status` : uptime, IP, memory, cog pid, URL.
- `rc-service thinframe status`, `tail /var/log/thinframe.log`
- `evtest` for touch, `dmesg | grep -E 'panel|silead|aic'` for drivers.
- Safe mode: rename `extlinux/extlinux.conf` on p1 to use the `-min` DTB entry to get a
  console only.
