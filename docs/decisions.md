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
Alpine default. `thinframe` is a plain `supervise-daemon` service so a crashed cog restarts.

## SSH key only, user `frame`, `doas` not `sudo`
Smaller, Alpine default. Host key generated once into `image/secrets/` so re-flashing a card
does not change the fingerprint.

## Content as a local file by default
`file:///var/lib/thinframe/www/index.html`. No web server needed on the frame. Set
`FRAME_URL` in `/etc/conf.d/thinframe` to point at anything else (Home Assistant, a
dashboard). The page itself can fetch whatever it wants.

## One Rust binary for everything custom
Shell scripts were the first draft. They pull in coreutils behaviour, fork a dozen processes
per invocation and are hard to test. `tfctl` is a single static musl binary (opt-level z, LTO,
panic=abort, stripped; target under 1 MB) with hand-rolled arg parsing and no crates unless a
task proves one necessary. It is the service entry point (spawns and supervises cog), the
updater, the Wi-Fi helper and the status tool. The rest of the system is stock Alpine: busybox,
OpenRC, eudev, seatd, iwd, dhcpcd, sshd.

## Nothing extra in the image
No avahi (dbus + daemon for a name the router already knows), no chrony (busybox ntpd), no git
(tfctl downloads a tarball), no htop/curl/rsync in the release image. Debug tools are in
`image/packages-debug.txt` and only go into a DEBUG build.

## Where dashboard performance actually comes from
`tfctl` is not on the hot path: it launches cog and sleeps. Frame rate and responsiveness are
decided by WPE WebKit (C++, GLES on lima) and by the page. Rules for `www/`:
- No framework. Plain HTML/CSS/JS, one file if possible.
- Animate only `transform` and `opacity` (GPU composited). No layout thrash, no box-shadow animation.
- Few DOM nodes. Update text, do not rebuild trees.
- Images pre-sized to the panel (800x1280 or 1280x800). No client-side scaling of 4K photos.
- Fetch data on a timer, render diffs. Keep JS heap small; the WebProcess has ~200 MB to live in.

If HTML ever becomes the bottleneck, the escape hatch is a Rust renderer drawing straight to
DRM/GBM with GLES (no browser at all). That is a different project; note it, do not start it.

## Thin client: brains in a container, frame is a display
The frame stays exactly what it was (Alpine + cog on DRM). What changes is where the page and
the decisions live: `frameserver`, a Rust binary in a scratch Docker image, on whatever box has
spare CPU. The frame's `tfctl agent` polls it for config and posts heartbeats. Consequences:

- Dashboard changes are a container rebuild. The SD card is written once.
- Heavy work (image resizing, API calls, caching, scheduling) runs on the server. The frame's 1 GB
  holds only WebKit and the rendered page.
- Many frames, one server, per-frame config by id.
- Offline: the frame falls back to a local page; the dashboard should also cache itself.

Rejected: streaming pixels (VNC/RDP/WebRTC). Bandwidth and latency are worse than sending HTML
and letting the frame's GPU composite it, and it needs a heavier client than cog.

Transport is plain HTTP polling on the LAN. WebSocket push is a later refinement, not needed
at a 15 s poll.
