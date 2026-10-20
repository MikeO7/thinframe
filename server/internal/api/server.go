// Package api is the HTTP surface: dashboard pages, REST for frames, the control
// WebSocket, the SSE data plane, the admin page and the data adapters. MCP calls
// the same methods on *Server, so there is one code path for every change.
package api

import (
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"net/http"
	"strings"
	"sync"
	"time"

	"github.com/MikeO7/thinframe/server/internal/artifacts"
	"github.com/MikeO7/thinframe/server/internal/hub"
	"github.com/MikeO7/thinframe/server/internal/photos"
	"github.com/MikeO7/thinframe/server/internal/store"
	"github.com/MikeO7/thinframe/server/internal/widgets"
)

// Options configures a Server.
type Options struct {
	Token     string // empty = no auth (LAN dev only)
	Photos    *photos.Service
	Artifacts *artifacts.Dir
	Weather   WeatherSource  // may be nil
	Calendar  CalendarSource // may be nil
	Version   string
}

// Server holds the dependencies and implements every operation once.
type Server struct {
	Store *store.Store
	Hub   *hub.Hub
	opts  Options
	sse   *broker
	mu    sync.Mutex
	// offlineAfter marks a frame offline if no heartbeat for this long.
	offlineAfter time.Duration
	// dataEvery is how often an open SSE stream gets fresh weather and calendar data.
	dataEvery time.Duration
}

// New wires a Server.
func New(st *store.Store, opts Options) *Server {
	s := &Server{Store: st, Hub: hub.New(), opts: opts, sse: newBroker(), offlineAfter: 3 * time.Minute, dataEvery: 10 * time.Minute}
	s.Hub.OnClose = func(id string) {
		_, _ = st.Update(id, func(f *store.Frame) { store.AddEvent(f, "ws", "disconnected") })
	}
	return s
}

// Routes returns the full mux.
func (s *Server) Routes() http.Handler {
	m := http.NewServeMux()
	m.HandleFunc("GET /", s.handleIndex)
	m.HandleFunc("GET /healthz", func(w http.ResponseWriter, _ *http.Request) { _, _ = w.Write([]byte("ok")) })
	m.HandleFunc("GET /frame/{id}", s.handleFramePage)
	m.HandleFunc("GET /api/stream/{id}", s.handleSSE)
	m.HandleFunc("GET /api/data/photo/next", s.handlePhotoNext)
	m.HandleFunc("GET /api/data/weather", s.handleDataWeather)
	m.HandleFunc("GET /api/data/calendar", s.handleDataCalendar)
	m.HandleFunc("POST /api/frames/{id}/beacon", s.handleBeacon)
	// Protected by token when set.
	m.Handle("GET /api/frames", s.auth(http.HandlerFunc(s.handleList)))
	m.Handle("GET /api/frames/{id}", s.auth(http.HandlerFunc(s.handleGetFrame)))
	m.Handle("DELETE /api/frames/{id}", s.auth(http.HandlerFunc(s.handleDeleteFrame)))
	m.Handle("GET /api/frames/{id}/config", s.auth(http.HandlerFunc(s.handleGetConfig)))
	m.Handle("PUT /api/frames/{id}/config", s.auth(http.HandlerFunc(s.handlePutConfig)))
	m.Handle("PUT /api/frames/{id}/layout", s.auth(http.HandlerFunc(s.handlePutLayout)))
	m.Handle("POST /api/frames/{id}/heartbeat", s.auth(http.HandlerFunc(s.handleHeartbeat)))
	m.Handle("POST /api/frames/{id}/actions", s.auth(http.HandlerFunc(s.handlePostAction)))
	m.Handle("GET /api/frames/{id}/ws", s.auth(http.HandlerFunc(s.handleWS)))
	m.Handle("GET /api/release", s.auth(http.HandlerFunc(s.handleRelease)))
	if s.opts.Artifacts != nil {
		m.Handle("GET /api/artifacts/{name}", s.auth(s.opts.Artifacts.Handler()))
	}
	m.HandleFunc("GET /admin", s.handleAdmin)
	m.HandleFunc("POST /admin/{id}/config", s.handleAdminConfig)
	m.HandleFunc("POST /admin/{id}/layout", s.handleAdminLayout)
	m.HandleFunc("POST /admin/{id}/action", s.handleAdminAction)
	return logRequests(m)
}

// ---- auth -----------------------------------------------------------------

func (s *Server) auth(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if s.opts.Token == "" || tokenOK(r, s.opts.Token) {
			next.ServeHTTP(w, r)
			return
		}
		w.Header().Set("WWW-Authenticate", `Bearer realm="thinframe"`)
		http.Error(w, "unauthorized", http.StatusUnauthorized)
	})
}

func tokenOK(r *http.Request, want string) bool {
	h := r.Header.Get("Authorization")
	if t, ok := strings.CutPrefix(h, "Bearer "); ok && t == want {
		return true
	}
	// WebSocket clients that cannot set headers may pass ?token=; tfctl sets the header.
	return r.URL.Query().Get("token") == want
}

// TokenOK exposes the check for other packages (MCP).
func (s *Server) TokenOK(r *http.Request) bool { return s.opts.Token == "" || tokenOK(r, s.opts.Token) }

// ---- operations shared by REST, admin and MCP ------------------------------

// ErrInvalid is returned for bad input.
var ErrInvalid = errors.New("invalid input")

// SetConfig merges non-nil fields into a frame's config, bumps page_version when the
// page is affected, persists, and pushes to the frame if online.
func (s *Server) SetConfig(id string, patch map[string]any) (store.Frame, error) {
	var bad error
	f, err := s.Store.Update(id, func(f *store.Frame) {
		c := &f.Config
		for k, v := range patch {
			switch k {
			case "url":
				c.URL, _ = v.(string)
			case "rotation":
				n, ok := asInt(v)
				if !ok || n < 0 || n > 3 {
					bad = fmt.Errorf("%w: rotation must be 0..3", ErrInvalid)
					return
				}
				c.Rotation = n
			case "brightness":
				n, ok := asInt(v)
				if !ok || n < 0 || n > 100 {
					bad = fmt.Errorf("%w: brightness must be 0..100", ErrInvalid)
					return
				}
				c.Brightness = n
			case "screen_off":
				c.ScreenOff, _ = v.(string)
			case "screen_on":
				c.ScreenOn, _ = v.(string)
			case "tags":
				c.Tags = asStrings(v)
			case "name":
				f.Name, _ = v.(string)
			default:
				bad = fmt.Errorf("%w: unknown config field %q", ErrInvalid, k)
				return
			}
		}
		store.AddEvent(f, "config", fmt.Sprintf("set %v", keys(patch)))
	})
	if err != nil {
		return f, err
	}
	if bad != nil {
		return f, bad
	}
	s.pushConfig(f)
	return f, nil
}

// SetLayout validates and stores a layout, bumps page_version and tells the page to reload.
func (s *Server) SetLayout(id string, l store.Layout) (store.Frame, error) {
	if err := widgets.Validate(l); err != nil {
		return store.Frame{}, fmt.Errorf("%w: %v", ErrInvalid, err)
	}
	f, err := s.Store.Update(id, func(f *store.Frame) {
		f.Layout = l
		f.Config.PageVersion++
		store.AddEvent(f, "layout", fmt.Sprintf("%d widgets, page_version %d", len(l.Widgets), f.Config.PageVersion))
	})
	if err != nil {
		return f, err
	}
	s.pushConfig(f)
	s.sse.publish(id, map[string]any{"page_version": f.Config.PageVersion})
	return f, nil
}

// ValidActions lists what a frame understands.
var ValidActions = []string{"reboot", "restart-kiosk", "reload-page", "screen", "update-software", "update-system", "rollback", "screenshot"}

// Act queues an action and pushes it if the frame is online.
func (s *Server) Act(id, name string, args map[string]string) (store.Action, error) {
	ok := false
	for _, v := range ValidActions {
		ok = ok || v == name
	}
	if !ok {
		return store.Action{}, fmt.Errorf("%w: unknown action %q (valid: %s)", ErrInvalid, name, strings.Join(ValidActions, ", "))
	}
	a := store.Action{ID: newID(), Name: name, Args: normalizeArgs(name, args), CreatedAt: time.Now().UTC()}
	_, err := s.Store.Update(id, func(f *store.Frame) {
		f.Pending = append(f.Pending, a)
		store.AddEvent(f, "action", name)
	})
	if err != nil {
		return a, err
	}
	if name == "reload-page" {
		s.sse.publish(id, map[string]any{"reload": true})
	}
	s.Hub.Send(id, actionMsg(a))
	return a, nil
}

// ConfigFor is the config object as the frame sees it (URL resolved, versions filled in).
func (s *Server) ConfigFor(f store.Frame) map[string]any {
	c := f.Config
	if c.URL == "" {
		c.URL = "/frame/" + f.ID
	}
	out := map[string]any{
		"url": c.URL, "rotation": c.Rotation, "brightness": c.Brightness,
		"screen_off": c.ScreenOff, "screen_on": c.ScreenOn, "page_version": c.PageVersion,
		"tags": c.Tags, "name": f.Name,
	}
	if s.opts.Artifacts != nil {
		v := s.opts.Artifacts.Versions()
		out["software_version"], out["system_version"] = v.Software, v.System
		sums := map[string]string{}
		for logical, file := range v.Files {
			if sum, err := s.opts.Artifacts.Sum(file); err == nil {
				sums[logical] = file + ":" + sum
			}
		}
		out["artifacts"] = sums
	}
	return out
}

// Online reports a frame's liveness: socket open, or heartbeat within offlineAfter.
func (s *Server) Online(f store.Frame) bool {
	if s.Hub.Online(f.ID) {
		return true
	}
	return f.Last != nil && time.Since(f.Last.At) < s.offlineAfter
}

func (s *Server) pushConfig(f store.Frame) {
	s.Hub.Send(f.ID, hub.Msg{"type": "config", "config": s.ConfigFor(f)})
}

// actionMsg is the wire form. Args are sent as a map and also flattened to the
// top level, which is what tfctl reads (e.g. {"name":"screen","state":"off"}).
func actionMsg(a store.Action) hub.Msg {
	m := hub.Msg{"type": "action", "id": a.ID, "name": a.Name, "args": a.Args}
	for k, v := range a.Args {
		if _, taken := m[k]; !taken {
			m[k] = v
		}
	}
	return m
}

// normalizeArgs maps the generic "arg" key used by the admin form and MCP to the
// key each action expects.
func normalizeArgs(name string, args map[string]string) map[string]string {
	if v, ok := args["arg"]; ok {
		switch name {
		case "screen":
			args["state"] = v
		}
	}
	return args
}

// ---- helpers -----------------------------------------------------------------

func newID() string {
	b := make([]byte, 6)
	_, _ = rand.Read(b)
	return hex.EncodeToString(b)
}

func asInt(v any) (int, bool) {
	switch n := v.(type) {
	case int:
		return n, true
	case float64:
		return int(n), n == float64(int(n))
	case json.Number:
		i, err := n.Int64()
		return int(i), err == nil
	case string:
		var i int
		_, err := fmt.Sscanf(n, "%d", &i)
		return i, err == nil
	}
	return 0, false
}

func asStrings(v any) []string {
	switch t := v.(type) {
	case []string:
		return t
	case []any:
		out := make([]string, 0, len(t))
		for _, x := range t {
			if s, ok := x.(string); ok {
				out = append(out, s)
			}
		}
		return out
	case string:
		if t == "" {
			return nil
		}
		parts := strings.Split(t, ",")
		for i := range parts {
			parts[i] = strings.TrimSpace(parts[i])
		}
		return parts
	}
	return nil
}

func keys(m map[string]any) []string {
	out := make([]string, 0, len(m))
	for k := range m {
		out = append(out, k)
	}
	return out
}

func writeJSON(w http.ResponseWriter, code int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	_ = json.NewEncoder(w).Encode(v)
}

func writeErr(w http.ResponseWriter, err error) {
	switch {
	case errors.Is(err, store.ErrNotFound):
		http.Error(w, err.Error(), http.StatusNotFound)
	case errors.Is(err, ErrInvalid):
		http.Error(w, err.Error(), http.StatusBadRequest)
	default:
		http.Error(w, err.Error(), http.StatusInternalServerError)
	}
}

func logRequests(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if strings.HasPrefix(r.URL.Path, "/api/stream/") || strings.HasPrefix(r.URL.Path, "/api/data/") {
			next.ServeHTTP(w, r)
			return
		}
		t := time.Now()
		next.ServeHTTP(w, r)
		log.Printf("%s %s %s", r.Method, r.URL.Path, time.Since(t).Round(time.Millisecond))
	})
}
