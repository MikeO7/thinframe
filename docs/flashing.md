# Flashing a frame

Three steps. No SSH, no editing files, no building. About ten minutes, mostly waiting.

## You need
- The frame (PF1007L family: Humblestead / EFERCRO / Anna Bella 10.1", see upstream device index).
- A microSD card, 8 to 64 GB. It becomes the frame's disk. The frame's internal storage is never touched.
- A Mac with a card reader.
- `frameserver` running somewhere on your network (`docker compose up -d`). Note its URL and token.

## 1. Flash the card

```bash
curl -fsSL https://raw.githubusercontent.com/MikeO7/thinframe/main/scripts/flash.sh | bash
```

The script asks four questions and nothing else:

| Question | Example |
|---|---|
| Wi-Fi network | `HomeWifi` |
| Wi-Fi password | (hidden) |
| Server | `http://nas.local:8080` |
| Frame name | `kitchen` |

Then it downloads the latest image, shows you the card it found, waits for you to type the
card's name to confirm, writes it, verifies it, writes your answers onto the card, and ejects it.

It refuses anything that is not a removable card between 4 and 64 GB, and it never asks for your
password except for the one `sudo` the write itself needs.

## 2. Put the card in, plug the frame in

Power off the frame. Card in the TF slot, label facing the back. Power on. Leave it alone for
about a minute. First boot: the screen stays dark for 20 to 30 seconds, then shows the thinframe
logo, then the dashboard.

## 3. Check it arrived

Open `http://nas.local:8080/admin`. The frame is listed under the name you gave, with its IP,
version and temperature. Or ask Claude: "is the kitchen frame online?"

That is all. From here on the frame updates itself from the server, including the operating
system. You never flash it again unless the card dies.

## If something goes wrong

| Symptom | Do |
|---|---|
| Dark screen after 2 minutes | Pull the card and put it back. If still dark, re-run `flash.sh`; it offers to keep your answers. |
| Logo but no dashboard | The frame cannot reach the server. Check the server URL you typed is reachable from the Wi-Fi network. The frame shows the reason on screen. |
| Not in `/admin` | Wrong Wi-Fi password or token. Re-run `flash.sh`, choose "rewrite settings only" (10 seconds, no full flash). |
| Want the old Android back | Power off, remove the card, power on. The original firmware is untouched. |
| Need a shell | `ssh frame@<ip from admin>`. The key is at `~/.thinframe/frame_id_ed25519` on the Mac that ran `flash.sh`. |

## How it works (for the curious)

`flash.sh` writes the release image, then drops one file, `provision.txt`, onto the small FAT
partition that your Mac can see. On first boot `tfctl` reads it, writes the Wi-Fi profile, server
URL, token and frame name onto the persistent data partition, deletes `provision.txt`, and starts.
You can also create that file by hand: the format is in `image/boot/provision.txt.example`.
