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
    && rm -rf /var/lib/apt/lists/*
COPY --from=tfctl /src/target/release/tfctl /usr/bin/tfctl
COPY --from=driver /simframe /usr/bin/simframe
ENV SERVER_URL=http://frameserver:8080 FRAME_ID=sim SHOT_INTERVAL_S=5 OUT_DIR=/out
VOLUME /out
ENTRYPOINT ["/usr/bin/simframe"]
