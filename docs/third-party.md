# Third-party origins

Everything in this repository is built from pinned mainline sources by our own scripts. The
board-specific knowledge in three places started as files of
[pangoleen/ormarch-picture-frame](https://github.com/pangoleen/ormarch-picture-frame)
(commit `4207b5c2f6d6615bf6a18fb725cc7a77dd532065`, MIT), itself building on Wes Bos's
U-Boot work for this frame (`wesbos/photo-frame-dashboard`), and have since been rewritten
for this project:

| Here | Started as |
|---|---|
| `image/buildroot/board/pf1007l/dts/allwinner/sun8i-a33-pf1007l.dts` | `kernel/dts/sun8i-a33-pf1007l.dts` |
| `image/buildroot/board/pf1007l/patches/linux/0001-drm-panel-*.patch` | `kernel/patches/v7.2.10/0001-*.patch` |
| `image/uboot/patches/*.patch` | `image/uboot/0001-wes-pf1007l-uboot.patch`, `0002-pf1007l-linux-boot.patch` |
| `image/uboot/boot.cmd` | `image/uboot/boot.cmd` |
| `docs/hardware.md` | `docs/hardware.md`, `docs/experiments.md` (facts, tagged [D]) |

The DTS and the patches carry their own licences (GPL-2.0+ OR MIT for the device trees, the
kernel's and U-Boot's GPL-2.0+ for the code). The MIT notice of the project they came from:

```
MIT License

Copyright (c) 2026 Paolo Rosson

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
