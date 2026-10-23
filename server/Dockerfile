# Multi-arch (amd64/arm64). Static Go binary, scratch base, a few MB.
FROM --platform=$BUILDPLATFORM golang:1.27-alpine AS build
ARG TARGETOS TARGETARCH VERSION=dev
WORKDIR /src
COPY go.mod go.sum* ./
RUN go mod download
COPY . .
RUN CGO_ENABLED=0 GOOS=$TARGETOS GOARCH=$TARGETARCH go build -trimpath -ldflags="-s -w -X main.Version=$VERSION" -o /frameserver ./cmd/frameserver

# TASK S6: release.yml runs scripts/make-artifacts.sh into server/artifacts/ (tfctl-armv7,
# system-<ver>.tar, versions.json) before the build; the dir is empty in dev.

FROM scratch
COPY --from=build /frameserver /frameserver
COPY artifacts/ /artifacts/
VOLUME /data
EXPOSE 8080
ENTRYPOINT ["/frameserver"]
