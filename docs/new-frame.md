# Setting up a frame

From a boxed PF1007L to a frame on your dashboard: one command on the Mac, one card, one tap.
After that everything (updates, settings, fixes, reboots) comes from the server.

## Once per Mac

1. **Server.** In this repository:

   ```bash
   scripts/server-mac.sh install
   ```

   It builds `frameserver`, makes an admin token, and runs it as a login service on port 41007
   (restarts on its own; `scripts/server-mac.sh status` lists the frames). Frames find it on the
   network by themselves **[V]** (2026-10-22: the frame moved to the Mac's new address on its own).
   Keep the Mac awake while frames use it.

2. **Touch firmware.** The touch controller needs the vendor's table, which cannot be shipped.
   It is the same file for every PF1007L. The flasher looks for `~/.thinframe/gsl_m86_hd.fw` and
   puts it on each card; the frame checks its SHA-256 (`e11ffa8a…`) before using it.
   Get it once from any PF1007L's Android boot image (`image/vendor/README.md` upstream,
   `extract_gsl_fw.py`), or copy it from a Mac that has it. Without it everything works but taps.

## For each frame

1. Put a microSD card (8 to 64 GB) in the Mac and run:

   ```bash
   scripts/flash.sh
   ```

   | Step | What happens |
   |---|---|
   | Settings | Wi-Fi network and password come from this Mac (Keychain). The server is found on the network. You type a **name** for the frame (`kitchen`, lowercased). |
   | Image | The latest release, checked against its SHA-256 (or `IMAGE=path/to/thinframe-sd.img` for a local build). |
   | Card | Waits for the card, shows its size and name, asks once before erasing. |
   | Write | One Mac password prompt, progress bars for writing and verifying, then settings and touch firmware go on the card and it ejects. |

2. Put the card in the frame **before** powering it on, then power on. With no card, or a card
   inserted late, the frame starts its original Android instead; power-cycle with the card in.

3. About a minute later the frame shows a **PAIR** button. Tap it once. The frame appears on the
   admin page (`http://<mac>:41007/admin`, password = the token) and shows its dashboard.

That is all. The frame's own storage (Android) is never written: take the card out and it is
the original frame again.

## If something is off

| You see | Cause and fix |
|---|---|
| The original Android screen | The card went in after power-on, or is not seated. Power-cycle with the card in. |
| Black screen for more than 2 minutes | `scripts/server-mac.sh status`: the frame should be listed. If not, plug the frame's USB-C into the Mac: it reaches the server over the cable without Wi-Fi, and fixes can be pushed. |
| "Reaching the server" | The Mac is asleep or the server is stopped: `scripts/server-mac.sh status`. |
| Wi-Fi list on the screen | The saved network did not join (wrong password, out of range). Pick a network on screen. |
| Taps do nothing | No touch firmware was on the card. Copy `gsl_m86_hd.fw` to `~/.thinframe/` and rewrite the settings: `scripts/flash.sh --settings-only`. |

A second PF1007L has not been tried yet **[U]**: the bootloader carries this unit's memory
settings (docs/hardware.md). If a new unit does not start from the card, take the card out
(Android comes back) and note it in docs/log.md.
