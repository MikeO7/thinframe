# Multi-arch (amd64/arm64) image. Static musl binary, scratch base, ~5 MB.
FROM --platform=$BUILDPLATFORM rust:1-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY . .
# TASK S1: cross-compile for $TARGETPLATFORM (use cargo-zigbuild or rust:alpine per-arch build)
RUN cargo build --release

FROM scratch
COPY --from=build /src/target/release/frameserver /frameserver
COPY static /static
VOLUME /data
EXPOSE 8080
ENTRYPOINT ["/frameserver"]
