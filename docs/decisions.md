# Decisions

## Alpine, not Arch Linux ARM
Alpine armv7 minirootfs is ~3 MB; the finished root should be under 200 MB. musl + busybox
+ OpenRC start in a few seconds and idle at ~30 MB. Arch ARM gave us systemd and a desktop we
do not need. Alpine is still a real package manager, so `cog`, `iwd`, `openssh` are one
`apk add` away.

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

## Reuse the ormarch kernel
Mainline needs two things this board does not have upstream: the JD9365DA panel init table and
the AIC8800 SDIO driver. ormarch already builds both as modules against Arch's `linux-armv7`
7.2.10 and has a DTB that booted. A kernel is distro-neutral: we unpack the Arch package for
`/boot/zImage` and `/lib/modules/...` into the Alpine root. Later, if wanted, build the same
sources with Alpine's `abuild` and drop the Arch dependency (not a task now).

## OpenRC services instead of systemd units
Alpine default. `alpineframe` is a plain `supervise-daemon` service so a crashed cog restarts.

## SSH key only, user `frame`, `doas` not `sudo`
Smaller, Alpine default. Host key generated once into `image/secrets/` so re-flashing a card
does not change the fingerprint.

## Content as a local file by default
`file:///var/lib/alpineframe/www/index.html`. No web server needed on the frame. Set
`FRAME_URL` in `/etc/conf.d/alpineframe` to point at anything else (Home Assistant, a
dashboard). The page itself can fetch whatever it wants.

## One Rust binary for everything custom
Shell scripts were the first draft. They pull in coreutils behaviour, fork a dozen processes
per invocation and are hard to test. `afctl` is a single static musl binary (opt-level z, LTO,
panic=abort, stripped; target under 1 MB) with hand-rolled arg parsing and no crates unless a
task proves one necessary. It is the service entry point (spawns and supervises cog), the
updater, the Wi-Fi helper and the status tool. The rest of the system is stock Alpine: busybox,
OpenRC, eudev, seatd, iwd, dhcpcd, sshd.

## Nothing extra in the image
No avahi (dbus + daemon for a name the router already knows), no chrony (busybox ntpd), no git
(afctl downloads a tarball), no htop/curl/rsync in the release image. Debug tools are in
`image/packages-debug.txt` and only go into a DEBUG build.
