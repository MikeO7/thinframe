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
//	PORT          default 41007 (tcp, the same number as DISCOVERY's udp port)
//	DATA_DIR      default /data
//	FRAME_TOKEN   bearer token for /api and /mcp, Basic password for /admin; empty disables auth (LAN dev only)
//	PHOTOS_DIR    folder photo source; unset = photo widget shows nothing
//	ARTIFACTS_DIR default /artifacts; tfctl-armv7, system-*.tar, versions.json
//	WEATHER_LAT, WEATHER_LON  enable the open-meteo weather adapter (current and 7-day forecast)
//	WEATHER_UNITS  f for °F (default °C)
//	CALENDAR_ICS_URL  enable the calendar widget from a public .ics feed (no recurrence expansion)
//	TZ            frames receive this time zone
//	PAGE_STALE    hung-page restart after this long without a page ping (default 3m, Go duration)
//	SERVICES_SSH  ssh destination whose `docker ps` feeds the source "services" (key auth only)
//	SERVICES_DOCKER docker socket path read instead (in a container: mount the host's socket); wins over SERVICES_SSH
//	SERVICES_EVERY  how often to poll (default 1m, Go duration)
//	DISCOVERY     udp address frames probe to find this server (default :41007, "off" disables;
//	              needs host networking in a container)
package main

import (
	"context"
	"errors"
	"fmt"
	"log"
	"net"
	"net/http"
	"os"
	"os/signal"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"time"

	"github.com/MikeO7/thinframe/server/internal/api"
	"github.com/MikeO7/thinframe/server/internal/artifacts"
	"github.com/MikeO7/thinframe/server/internal/discover"
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
	opts := api.Options{Token: os.Getenv("FRAME_TOKEN"), Version: Version, Artifacts: artifacts.New(env("ARTIFACTS_DIR", "/artifacts")),
		Reports: filepath.Join(dataDir, "reports")}
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
			opts.Fahrenheit = strings.HasPrefix(strings.ToLower(os.Getenv("WEATHER_UNITS")), "f")
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
	// TCP on the same number as discovery's UDP port: "thinframe lives on 41007".
	port := env("PORT", strconv.Itoa(discover.Port))
	hs := newHTTPServer(":"+port, handler(srv))
	go func() {
		log.Printf("frameserver %s listening on %s (data %s)", Version, hs.Addr, dataDir)
		if err := hs.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
			log.Fatalf("listen: %v", err)
		}
	}()
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	if addr := env("DISCOVERY", fmt.Sprintf(":%d", discover.Port)); addr != "off" {
		go func() {
			keys := func(id string) []string {
				var k []string
				if f, err := st.Get(id); err == nil && f.TokenHash != "" {
					k = append(k, f.TokenHash)
				}
				if opts.Token != "" {
					k = append(k, discover.HashToken(opts.Token))
				}
				return k
			}
			if err := discover.Serve(ctx, addr, port, keys); err != nil {
				log.Printf("discovery: %v (frames need SERVER_URL)", err)
			}
		}()
	}
	every, err := time.ParseDuration(env("SERVICES_EVERY", "1m"))
	if err != nil || every < 10*time.Second {
		every = time.Minute
	}
	if sock := os.Getenv("SERVICES_DOCKER"); sock != "" {
		go srv.PollDocker(ctx, sock, every)
		log.Printf("services: docker socket %s every %s", sock, every)
	} else if host := os.Getenv("SERVICES_SSH"); host != "" {
		go srv.PollServices(ctx, host, every)
		log.Printf("services: docker ps on %s every %s", host, every)
	}
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
