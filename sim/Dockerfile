# TASK M1. Debian/Alpine with cog + wpewebkit (headless platform), plus tfctl built for the host arch.
# Loop: tfctl agent & ; cog --platform=headless renders $SERVER_URL/frame/$FRAME_ID; screenshot to /out/frame.png.
# Note: cog headless has no screenshot flag; use WPEWebDriver or wpe's `--enable-automation` + a
# tiny script, or render via `wpewebkit` GTK-less minibrowser with offscreen capture. Pick the simplest
# that works and record it in docs/decisions.md.
FROM alpine:3.24
RUN apk add --no-cache cog wpewebkit
ENTRYPOINT ["/bin/sh", "-c", "echo 'TASK M1 not implemented'; exit 1"]
