# Flashing a frame

Three steps. No SSH, no editing files, no building. About ten minutes, mostly waiting.

## You need
- The frame (PF1007L family: Humblestead / EFERCRO / Anna Bella 10.1", see upstream device index).
- A microSD card, 4 to 64 GB. It becomes the frame's disk. The frame's internal storage is never touched.
- A Mac with a card reader.
- `frameserver` running on your network: natively, or in a container with host networking so the
  frame's discovery broadcast (UDP 41007) reaches it. The root `compose.yml` is bridged: with it,
  press `e` below and type the server URL. Its token (`FRAME_TOKEN`) is the `/admin` password;
  the card does not need it.

## 1. Flash the card

```bash
curl -fsSL https://raw.githubusercontent.com/MikeO7/thinframe/main/scripts/flash.sh | bash
```

It shows what it will use and asks only for what it cannot know:

| Setting | Where it comes from |
|---|---|
| Wi-Fi network and password | This Mac's current network and its Keychain (typed if not found). `e` edits. |
| Server | Found by the frame on the network **[V]** (2026-10-22). `e` sets a fixed URL. |
| Frame name | Asked every time (`kitchen`; lowercased, spaces become dashes). |

Then it downloads the latest image (or uses `IMAGE=<file>`), waits for the card, asks once
before erasing it, writes and verifies it with progress bars, puts your settings and the touch
firmware (`~/.thinframe/gsl_m86_hd.fw`, if present) on it, and ejects it.

Running the server on the same Mac, and the touch firmware: [docs/new-frame.md](new-frame.md).

It refuses anything that is not a removable card between 4 and 64 GB, and it never asks for your
password except for the one `sudo` the write itself needs.

## 2. Put the card in, plug the frame in

Power off the frame. Card in the TF slot, label facing the back. Power on. Leave it alone for
about a minute. First boot: the thinframe logo within about 4 seconds (splash at 3.8 s **[V]**
2026-10-27, later boots; the first boot also grows the card's data partition **[U]**), then a
6-digit pairing code.

## 3. Pair it

Open `http://nas.local:41007/admin`. The frame is listed under "Waiting to pair": type the code
from its screen and press pair (or tell Claude the code). Within a few seconds the frame opens its
dashboard and is listed under the name you gave, with its IP, version and temperature.
If the server has `FRAME_TOKEN` set, the browser asks for a login first: any user name, the
token as password **[V]** (server tests `TestAdminAuth`).

That is all. From here on the frame updates itself from the server, including the operating
system. You never flash it again unless the card dies.

## If something goes wrong

| Symptom | Do |
|---|---|
| Dark screen after 2 minutes | Pull the card and put it back. If still dark, re-run `flash.sh`; it remembers your Wi-Fi and server. |
| Logo but no dashboard | The frame cannot reach the server. Check the server URL you typed is reachable from the Wi-Fi network. The frame shows the reason on screen. |
| Not in `/admin` | Wrong Wi-Fi password, or not paired yet (type the code from the frame on `/admin`). To rewrite only the settings (10 seconds, no full flash): `curl -fsSL <the flash.sh URL above> \| bash -s -- --settings-only`. |
| Black screen on the wall brick, boots on a computer or dock | A card flashed before 2026-10-26 waits for a USB host: its console was the USB serial port, which takes no text until a computer connects. Re-run `flash.sh` with a newer image. Until then a powered USB hub between the brick and the frame starts it. **[U]** cause read in the kernel source 2026-10-26, see `docs/log.md`. |
| Want the old Android back | Power off, remove the card, power on. The original firmware is untouched. |
| Need a shell | `ssh frame@<ip from admin>`. The key is at `~/.thinframe/frame_id_ed25519` on the Mac that ran `flash.sh`. |

## How it works (for the curious)

`flash.sh` writes the release image, then drops one file, `provision.txt`, onto the small FAT
partition that your Mac can see. On first boot `tfctl` reads it, writes the Wi-Fi profile, server
URL, token and frame name onto the persistent data partition, deletes `provision.txt`, and starts.
You can also create that file by hand: the format is in `image/boot/provision.txt.example`.
