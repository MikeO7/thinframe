# Multi-arch (amd64/arm64). Static Go binary, scratch base, a few MB.
FROM --platform=$BUILDPLATFORM golang:1.27-alpine AS build
ARG TARGETOS TARGETARCH VERSION=dev
WORKDIR /src
COPY go.mod go.sum* ./
RUN go mod download
COPY . .
RUN CGO_ENABLED=0 GOOS=$TARGETOS GOARCH=$TARGETARCH go build -trimpath -ldflags="-s -w -X main.Version=$VERSION" -o /frameserver ./cmd/frameserver

# TASK S6: a second stage cross-builds tfctl for armv7 (cargo-zigbuild) and tars image/rootfs/
# into /artifacts with sha256 sidecars; system-<ver>.tar is copied in from image/out by release.yml.

FROM scratch
COPY --from=build /frameserver /frameserver
VOLUME /data
EXPOSE 8080
ENTRYPOINT ["/frameserver"]
