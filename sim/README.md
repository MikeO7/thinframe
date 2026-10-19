# sim/ — iterate without the frame

| Tier | What | Cost | Use for |
|---|---|---|---|
| 0 | Desktop browser at `http://localhost:8080/frame/sim`, devtools device mode 1280x800, touch on. Safari/Epiphany are WebKit like the frame | none | dashboard layout, JS, data |
| 1 | `docker compose -f sim/compose.yml up`: a container with headless WPE (`cog --platform=headless`) rendering `/frame/sim` every N s to `sim/out/frame.png`, plus a native `tfctl agent` posting heartbeats and obeying config | one container | real WebKit rendering, agent/config/update loop, multi-frame admin |
| 2 | QEMU full system of the SD image (`image/build.sh test`) | slow, no panel | boot, OpenRC, networking, update install path |

Tier 1 is TASK M1. The sim frame shows up in `/admin` as `sim`, so the whole control loop is
exercised with zero hardware: change rotation or URL in admin, watch `frame.png` change.
