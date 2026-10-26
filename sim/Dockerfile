# simframe: WPE WebKit (cog headless) + WPEWebDriver + the real tfctl agent + a Go driver that
# follows the server's config and writes /out/frame.png. Runs on amd64 or arm64; no hardware.
FROM rust:1-slim-trixie AS tfctl
WORKDIR /src
COPY tfctl/ .
RUN cargo build --release && strip target/release/tfctl

FROM golang:1.27-trixie AS driver
WORKDIR /src
COPY sim/go.mod ./
COPY sim/cmd ./cmd
RUN CGO_ENABLED=0 go build -trimpath -ldflags="-s -w" -o /simframe ./cmd/simframe

FROM debian:trixie-slim
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends \
      cog wpewebkit-driver fonts-dejavu-core ca-certificates iproute2 \
      libgles2 libegl1 libgl1-mesa-dri libegl-mesa0 libgbm1 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=tfctl /src/target/release/tfctl /usr/bin/tfctl
COPY --from=driver /simframe /usr/bin/simframe
# No bubblewrap sandbox: hosts that restrict user namespaces (Ubuntu 24.04 runners) kill the
# WebProcess. Dev-only container loading our own server; the frame image keeps the sandbox.
ENV SERVER_URL=http://frameserver:41007 FRAME_ID=sim SHOT_INTERVAL_S=5 OUT_DIR=/out \
    LIBGL_ALWAYS_SOFTWARE=1 EGL_PLATFORM=surfaceless WEBKIT_DISABLE_DMABUF_RENDERER=1 GALLIUM_DRIVER=llvmpipe \
    WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1
VOLUME /out
ENTRYPOINT ["/usr/bin/simframe"]
