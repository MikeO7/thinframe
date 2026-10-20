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
