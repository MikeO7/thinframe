# Rules for working on thinframe

Read `README.md`, then `TASKS.md`. Work the tasks in order. Do not skip a gate.

## Where to start, and what you can do without the frame
Everything in sections 0, 0b, S, M1 and 1 to 3 runs on the Mac or in Docker with no hardware.
Recommended order for a fresh start: S0, S1, S2 (the Go server with real tests), then 0b and
TASK 10 (`tfctl agent` talking to that server, natively on the Mac), then M1 (the sim container),
then 1 to 3 (the image). Sections 4 to 8 and 13 need the owner at the frame; batch them.

Mac prerequisites: Xcode CLT, Homebrew `git curl python3 shellcheck qemu cargo-zigbuild`, Docker
(any; Buildroot cross-compiles inside a linux/arm64 or amd64 container, no armv7 emulation needed),
Rust via rustup (cargo is at `~/.rustup/toolchains/stable-*/bin`, not on PATH), Go 1.27.

## When stuck
Prefer reading the upstream project `pangoleen/ormarch-picture-frame` (clone it outside this repo)
over inventing. Its `docs/experiments.md` has 2000 lines of what was tried on this exact board.
If a gate cannot pass, write why in `docs/log.md` and stop; do not loosen the gate.

## Safety

- **Never write the eMMC.** No `dd`, `sfdisk`, `mkfs` or `fastboot flash` to `/dev/mmcblk0`
  on the frame, ever. We boot from SD only. The frame must stay recoverable by removing the card.
- Card writes happen only through `scripts/flash.sh`, which refuses any disk that is not a
  removable card of 4 to 64 GB with no mounted system volume.
- No `sudo` on the Mac except inside the card writer.
- Proprietary files (`gsl_m86_hd.fw`, Android dumps) never enter Git. `.gitignore` covers
  `image/vendor/`, `image/cache/`, `image/work/`, `image/out/`, `image/secrets/`.
- Secrets (SSH host key, Wi-Fi PSK) go in `image/secrets/` (mode 700), never in Git.

## Boot inputs are vendored, not fetched

`image/inputs/` holds the hardware-tested board sources we did not write: the board DTS and its
three variants, the v7.2.10 panel patch, the two U-Boot patches and `boot.cmd`, all from
`pangoleen/ormarch-picture-frame` at one commit (MIT, `image/inputs/README.md` has the table).
U-Boot is built from pinned mainline by `image/uboot-build.sh`; the kernel, DTBs and the AIC8800
modules by Buildroot. No build step clones another project. Change a vendored file only with a
reason in `docs/decisions.md`, and keep `image/inputs/README.md` true.

## Tags

Every claim in `docs/` carries one: **[V]** read in source or measured on this frame,
**[D]** second-hand (upstream project, a wiki), **[U]** inference, test it. When you verify
something on hardware, change its tag and add a dated line to `docs/log.md`.

## Done means

A task is done when its gate line in `TASKS.md` is checked and `docs/log.md` has the evidence
(command + output, or a photo path). Nothing is "probably fine".

## Languages
Frame: Rust only, in `tfctl/`. Container: Go only, in `server/`. Nothing else, anywhere.
No new shell scripts on the frame. If something needs logic, it is a subcommand of `tfctl`.
Minimal dependencies; justify every crate in `docs/decisions.md`. Binary under 2 MB (CI enforces
it); the README budgets are targets to measure against, not gates.
Mac-side build scripts may stay bash.

## Style

Shell on the Mac: `#!/usr/bin/env bash` with `set -euo pipefail`. On the frame only the BusyBox
init script `S90thinframe` is shell. Run `shellcheck` before committing. Keep scripts under 150 lines; split otherwise.
Commit small. Branch `mikeo/<topic>`, PR, auto-merge.
