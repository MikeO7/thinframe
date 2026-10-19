# Rules for working on thinframe

Read `README.md`, then `TASKS.md`. Work the tasks in order. Do not skip a gate.

## Safety

- **Never write the eMMC.** No `dd`, `sfdisk`, `mkfs` or `fastboot flash` to `/dev/mmcblk0`
  on the frame, ever. We boot from SD only. The frame must stay recoverable by removing the card.
- Card writes happen only through `scripts/write-card-macos.sh`, which refuses any disk that is
  not a removable card of 4 to 64 GB with no mounted system volume.
- No `sudo` on the Mac except inside the card writer.
- Proprietary files (`gsl_m86_hd.fw`, Android dumps) never enter Git. `.gitignore` covers
  `image/vendor/`, `image/cache/`, `image/work/`, `image/out/`, `image/secrets/`.
- Secrets (SSH host key, Wi-Fi PSK) go in `image/secrets/` (mode 700), never in Git.

## Reuse, do not re-derive

The upstream project `pangoleen/ormarch-picture-frame` has pinned, hardware-tested builds of
U-Boot, kernel 7.2.10, the panel module, three DTBs and the AIC8800 modules. `image/build.sh`
clones it into `image/cache/ormarch` and runs its `kernel/build.sh` and `wifi/build.sh`. Do
not port those recipes into this repo unless a task says so. If one of them breaks, fix it
there or pin an older commit here.

## Tags

Every claim in `docs/` carries one: **[V]** read in source or measured on this frame,
**[D]** second-hand (upstream project, a wiki), **[U]** inference, test it. When you verify
something on hardware, change its tag and add a dated line to `docs/log.md`.

## Done means

A task is done when its gate line in `TASKS.md` is checked and `docs/log.md` has the evidence
(command + output, or a photo path). Nothing is "probably fine".

## Custom code is Rust, in `tfctl/`
No new shell scripts on the frame. If something needs logic, it is a subcommand of `tfctl`.
Minimal dependencies; justify every crate in `docs/decisions.md`. Binary must stay under 1 MB.
Mac-side build scripts may stay bash.

## Style

Shell on the Mac: `#!/usr/bin/env bash` with `set -euo pipefail`. On the frame only OpenRC
service files and `local.d` one-liners are shell. Run `shellcheck` before committing. Keep scripts under 150 lines; split otherwise.
Commit small. Branch `mikeo/<topic>`, PR, auto-merge.
