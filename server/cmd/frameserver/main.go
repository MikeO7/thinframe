// frameserver: everything smart lives here, in a container on real hardware.
//
// The frame is a Wi-Fi touch screen. It does two things:
//  1. cog loads  GET /frame/{id}            (the dashboard page for that frame)
//  2. tfctl keeps one WebSocket open at /api/frames/{id}/ws (control plane)
//
// Routes are in internal/api. State is JSON files under DATA_DIR. No database.
//
// Environment:
//
//	PORT          default 8080
//	DATA_DIR      default /data
//	FRAME_TOKEN   bearer token for /api and /mcp, Basic password for /admin; empty disables auth (LAN dev only)
//	PHOTOS_DIR    folder photo source; unset = photo widget shows nothing
//	ARTIFACTS_DIR default /artifacts; tfctl-armv7, overlay.tar.gz, system-*.tar, versions.json
//	WEATHER_LAT, WEATHER_LON  enable the open-meteo weather adapter
//	CALENDAR_ICS_URL  enable the calendar widget from a public .ics feed (no recurrence expansion)
//	TZ            frames receive this time zone
//	PAGE_STALE    hung-page restart after this long without a page ping (default 3m, Go duration)
package main

import (
	"context"
	"errors"
	"log"
	"net"
	"net/http"
	"os"
	"os/signal"
	"path/filepath"
	"strconv"
	"syscall"
	"time"

	"github.com/MikeO7/thinframe/server/internal/api"
	"github.com/MikeO7/thinframe/server/internal/artifacts"
	"github.com/MikeO7/thinframe/server/internal/mcp"
	"github.com/MikeO7/thinframe/server/internal/photos"
	"github.com/MikeO7/thinframe/server/internal/store"
)

// Version is set by -ldflags "-X main.Version=...".
var Version = "dev"

func env(k, def string) string {
	if v := os.Getenv(k); v != "" {
		return v
	}
	return def
}

func main() {
	log.SetFlags(log.Ltime)
	dataDir := env("DATA_DIR", "/data")
	st, err := store.Open(dataDir)
	if err != nil {
		log.Fatalf("store: %v", err)
	}
	opts := api.Options{Token: os.Getenv("FRAME_TOKEN"), Version: Version, Artifacts: artifacts.New(env("ARTIFACTS_DIR", "/artifacts"))}
	if dir := os.Getenv("PHOTOS_DIR"); dir != "" {
		ph, err := photos.New(photos.Folder{Root: dir}, filepath.Join(dataDir, "cache"))
		if err != nil {
			log.Fatalf("photos: %v", err)
		}
		opts.Photos = ph
		log.Printf("photos: %s", dir)
	}
	if u := os.Getenv("CALENDAR_ICS_URL"); u != "" {
		loc, err := time.LoadLocation(env("TZ", "UTC"))
		if err != nil {
			loc = time.UTC
		}
		opts.Calendar = &api.ICS{URL: u, Loc: loc}
		log.Printf("calendar: ics feed enabled")
	}
	if lat, lon := os.Getenv("WEATHER_LAT"), os.Getenv("WEATHER_LON"); lat != "" && lon != "" {
		la, e1 := strconv.ParseFloat(lat, 64)
		lo, e2 := strconv.ParseFloat(lon, 64)
		if e1 == nil && e2 == nil {
			opts.Weather = &api.OpenMeteo{Lat: la, Lon: lo}
			log.Printf("weather: open-meteo %.3f,%.3f", la, lo)
		}
	}
	if opts.Token == "" {
		log.Printf("WARNING: FRAME_TOKEN empty, /api, /admin and /mcp are open to the network")
	}
	if d, err := time.ParseDuration(os.Getenv("PAGE_STALE")); err == nil && d > 0 {
		opts.PageStale = d
	}
	srv := api.New(st, opts)
	hs := newHTTPServer(":"+env("PORT", "8080"), handler(srv))
	go func() {
		log.Printf("frameserver %s listening on %s (data %s)", Version, hs.Addr, dataDir)
		if err := hs.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
			log.Fatalf("listen: %v", err)
		}
	}()
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	<-ctx.Done()
	shutdown, cancel := context.WithTimeout(context.Background(), shutdownTimeout)
	defer cancel()
	if err := hs.Shutdown(shutdown); err != nil {
		log.Printf("shutdown: %v", err)
	}
	if err := srv.Drain(shutdown); err != nil {
		log.Printf("shutdown: control sockets: %v", err)
	}
}

// shutdownTimeout bounds a graceful stop.
const shutdownTimeout = 5 * time.Second

// handler is everything the server answers: MCP at /mcp, the rest from api.
func handler(srv *api.Server) http.Handler {
	mux := http.NewServeMux()
	mux.Handle("/mcp", mcp.Handler(srv, Version))
	mux.Handle("/", srv.Routes())
	return mux
}

// newHTTPServer wraps h. Every request context derives from one base context that Shutdown
// cancels, so long-lived SSE streams and control sockets end at once instead of holding
// Shutdown until its deadline (Shutdown alone waits for them and never closes hijacked sockets).
func newHTTPServer(addr string, h http.Handler) *http.Server {
	base, cancel := context.WithCancel(context.Background())
	hs := &http.Server{Addr: addr, Handler: h, ReadHeaderTimeout: 10 * time.Second, IdleTimeout: 120 * time.Second, // no WriteTimeout: SSE
		BaseContext: func(net.Listener) context.Context { return base }}
	hs.RegisterOnShutdown(cancel)
	return hs
}
