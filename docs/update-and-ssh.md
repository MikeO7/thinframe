# Getting in and changing things

## Paths in
1. Wi-Fi: `ssh frame@alpineframe` (router DNS from the DHCP hostname) or the IP. Key from `image/secrets/frame_id_ed25519`.
2. USB-C data cable to the Mac: the frame appears as a USB Ethernet device. Frame is
   192.168.7.2, Mac side gets 192.168.7.1 via the frame's dhcpd. `ssh frame@192.168.7.2`.
   Also a USB serial console: `screen /dev/tty.usbmodem* 115200`.
3. Pull the card: p1 is FAT32, readable on the Mac. Edit `extlinux.conf` or `uEnv.txt` there.

## Changing the page
- Fast loop, no Git: `scripts/deploy.sh frame@alpineframe` copies `www/`, `image/rootfs/` and a
  freshly built `afctl` to the frame over ssh/tar and restarts the service.
- Proper: push to `main`, then on the frame `doas afctl update`. It fetches the main tarball, applies it, restarts.

## Changing config
`/etc/alpineframe.conf` on the frame (source of truth: `image/rootfs/etc/alpineframe.conf`):

```
FRAME_URL="file:///var/lib/alpineframe/www/index.html"
COG_PLATFORM="drm"
COG_ROTATION="3"          # quarter turns; verify on hardware
COG_EXTRA_ARGS=""
```

`rc-service alpineframe restart` after edits.

## Wi-Fi
`doas afctl wifi "My SSID" "passphrase"` writes an iwd profile and reconnects. Profiles live in
`/var/lib/iwd/`. Remove one to forget it.

## Debug
- `afctl status` : uptime, IP, memory, cog pid, URL.
- `rc-service alpineframe status`, `tail /var/log/alpineframe.log`
- `evtest` for touch, `dmesg | grep -E 'panel|silead|aic'` for drivers.
- Safe mode: rename `extlinux/extlinux.conf` on p1 to use the `-min` DTB entry to get a
  console only.
