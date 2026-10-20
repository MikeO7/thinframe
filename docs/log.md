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

## 2026-10-20
- Cloud session, no Docker daemon, no hardware. Re-ran every check that can run here, all green:
  `shellcheck -x -e SC1091,SC2034,SC2317 image/build.sh scripts/*.sh image/rootfs/etc/local.d/*` clean;
  tfctl `cargo fmt --check`, `clippy -D warnings`, `cargo test` (59 passed); server `go vet`, `gofmt -l`, `go test ./...` ok (Go 1.27 auto-fetched); sim `go vet` + `go build` ok.
- Not doable here, so not started (per CLAUDE.md, gates not loosened): TASK 1/2/3 and M1/M2 need a Docker daemon (Buildroot container, QEMU), sections 4-8, 13, K1 need the frame, E needs an explicit owner go.
- Repo visibility: could not be changed from this session (proxy blocks settings writes). Owner must do it in GitHub Settings.
