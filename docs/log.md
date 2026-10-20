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
