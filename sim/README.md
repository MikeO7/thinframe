# sim/ — iterate without the frame

| Tier | What | Cost | Use for |
|---|---|---|---|
| 0 | Desktop browser at `http://localhost:8080/frame/sim`, devtools device mode 1280x800, touch on. Safari/Epiphany are WebKit like the frame | none | dashboard layout, JS, data |
| 1 | `docker compose --profile sim up --build` from the repo root: Debian trixie with cog 0.18 headless + WPEWebDriver, the real `tfctl agent`, and a Go driver that follows the server's config and writes `sim/out/frame.png` every 5 s | one extra container | real WebKit rendering, WebSocket control loop, config pushes, actions and acks, multi-frame admin |
| 2 | `sim/qemu/run.sh`: the real Buildroot image in `qemu-system-arm -M virt` with virtio-gpu (DRM), virtio touchscreen and net; screenshots are QEMU framebuffer dumps | slow (32-bit TCG, no KVM on Apple Silicon) | exact OS behaviour: boot, splash, provisioning, A/B update and rollback, cog on DRM |

The sim frame shows up in `/admin` as `sim`. Change the layout or URL in admin (or via MCP) and
watch `sim/out/frame.png` change on the next shot. Rotation changes the viewport (1280x800 vs
800x1280). Actions that need hardware (screen, reboot, updates) are acked with an error.

How screenshots work: WPEWebDriver launches `cog --automation --platform=headless` and the driver
calls the WebDriver screenshot endpoint. No X, no Wayland, no VNC.
