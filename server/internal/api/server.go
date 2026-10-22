// Package api is the HTTP surface: dashboard pages, REST for frames, the control
// WebSocket, the SSE data plane, the admin page and the data adapters. MCP calls
// the same methods on *Server, so there is one code path for every change.
package api

import (
	"bytes"
	"crypto/rand"
	"crypto/subtle"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"net/http"
	"regexp"
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
	PageStale time.Duration // hung-page timeout; 0 = 3 minutes
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
	// page: last dashboard ping per frame; pageStale without one restarts cog, checked every pageCheck.
	page      pageWatch
	pageStale time.Duration
	pageCheck time.Duration
	// pair: open one-press pairing requests (pair.go), guarded by mu.
	pair pairings
	// patterns: what Routes registered.
	patterns []string
}

// New wires a Server.
func New(st *store.Store, opts Options) *Server {
	s := &Server{Store: st, Hub: hub.New(), opts: opts, sse: newBroker(), offlineAfter: 3 * time.Minute, dataEvery: 10 * time.Minute,
		page: pageWatch{last: map[string]time.Time{}}, pair: pairings{open: map[string]*pairReq{}}, pageStale: 3 * time.Minute, pageCheck: 30 * time.Second}
	if opts.PageStale > 0 {
		s.pageStale = opts.PageStale
	}
	s.Hub.OnClose = func(id string) {
		_, _ = st.Update(id, func(f *store.Frame) { store.AddEvent(f, "ws", "disconnected") })
	}
	return s
}

// Routes returns the full mux. Every pattern it registers is recorded for Patterns.
func (s *Server) Routes() http.Handler {
	m := http.NewServeMux()
	s.patterns = nil
	handle := func(pattern string, h http.Handler) {
		m.Handle(pattern, h)
		s.patterns = append(s.patterns, pattern)
	}
	fn := func(pattern string, h http.HandlerFunc) { handle(pattern, h) }
	fn("GET /", s.handleIndex)
	fn("GET /healthz", func(w http.ResponseWriter, _ *http.Request) { _, _ = w.Write([]byte("ok")) })
	fn("GET /frame/{id}", s.handleFramePage)
	// Page plane: the dashboard has no token (cog loads a plain URL, sendBeacon cannot set
	// headers), so these are public like the page itself.
	fn("GET /api/stream/{id}", s.handleSSE)
	fn("GET /api/data/photo/next", s.handlePhotoNext)
	fn("GET /api/data/weather", s.handleDataWeather)
	fn("GET /api/data/calendar", s.handleDataCalendar)
	fn("POST /api/frames/{id}/beacon", s.handleBeacon)
	fn("POST /api/frames/{id}/alive", s.handleAlive)
	// Pairing: a frame without a token asks for one (pair.go).
	fn("POST /api/pair", s.handlePairRequest)
	fn("GET /api/pair/{id}", s.handlePairPoll)
	fn("POST /pair/{id}/accept", s.handlePairAccept)
	// Protected by token when set. "frame" routes also take that frame's paired token,
	// "anyFrame" routes any paired frame's token; the rest need FRAME_TOKEN.
	handle("GET /api/frames", s.auth(http.HandlerFunc(s.handleList)))
	handle("GET /api/frames/{id}", s.authFrame(http.HandlerFunc(s.handleGetFrame)))
	handle("DELETE /api/frames/{id}", s.auth(http.HandlerFunc(s.handleDeleteFrame)))
	handle("GET /api/frames/{id}/config", s.authFrame(http.HandlerFunc(s.handleGetConfig)))
	handle("PUT /api/frames/{id}/config", s.auth(http.HandlerFunc(s.handlePutConfig)))
	handle("PUT /api/frames/{id}/layout", s.auth(http.HandlerFunc(s.handlePutLayout)))
	handle("POST /api/frames/{id}/heartbeat", s.authFrame(http.HandlerFunc(s.handleHeartbeat)))
	handle("POST /api/frames/{id}/actions", s.auth(http.HandlerFunc(s.handlePostAction)))
	handle("GET /api/frames/{id}/ws", s.authFrame(http.HandlerFunc(s.handleWS)))
	handle("GET /api/release", s.authAnyFrame(http.HandlerFunc(s.handleRelease)))
	if s.opts.Artifacts != nil {
		handle("GET /api/artifacts/{name}", s.authAnyFrame(s.opts.Artifacts.Handler()))
	}
	// Admin: HTTP Basic (password = FRAME_TOKEN) when set; POSTs also refuse cross-origin forms.
	cop := http.NewCrossOriginProtection()
	handle("GET /admin", s.authAdmin(http.HandlerFunc(s.handleAdmin)))
	handle("POST /admin/{id}/config", cop.Handler(s.authAdmin(adminForm(s.handleAdminConfig))))
	handle("POST /admin/{id}/layout", cop.Handler(s.authAdmin(adminForm(s.handleAdminLayout))))
	handle("POST /admin/{id}/action", cop.Handler(s.authAdmin(adminForm(s.handleAdminAction))))
	handle("POST /admin/{id}/unpair", cop.Handler(s.authAdmin(adminForm(s.handleAdminUnpair))))
	return logRequests(m)
}

// Patterns lists the mux patterns the last Routes call registered (the route x auth test
// checks that every one of them has a row).
func (s *Server) Patterns() []string { return append([]string(nil), s.patterns...) }

// ---- auth -----------------------------------------------------------------

func (s *Server) auth(next http.Handler) http.Handler {
	return s.authWith(next, func(*http.Request) bool { return false })
}

// authFrame also accepts the paired token of the frame named in the path.
func (s *Server) authFrame(next http.Handler) http.Handler {
	return s.authWith(next, func(r *http.Request) bool { return s.frameTokenOK(r, r.PathValue("id")) })
}

// authAnyFrame also accepts any paired frame's token (release info, update artifacts).
func (s *Server) authAnyFrame(next http.Handler) http.Handler {
	return s.authWith(next, func(r *http.Request) bool { return s.frameTokenOK(r, "") })
}

func (s *Server) authWith(next http.Handler, frameOK func(*http.Request) bool) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if s.opts.Token == "" || tokenOK(r, s.opts.Token) || frameOK(r) {
			next.ServeHTTP(w, r)
			return
		}
		w.Header().Set("WWW-Authenticate", `Bearer realm="thinframe"`)
		http.Error(w, "unauthorized", http.StatusUnauthorized)
	})
}

// authAdmin guards the browser admin page with HTTP Basic: any user name, password = token.
func (s *Server) authAdmin(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if _, pw, ok := r.BasicAuth(); s.opts.Token == "" || ok && subtle.ConstantTimeCompare([]byte(pw), []byte(s.opts.Token)) == 1 {
			next.ServeHTTP(w, r)
			return
		}
		w.Header().Set("WWW-Authenticate", `Basic realm="thinframe"`)
		http.Error(w, "unauthorized", http.StatusUnauthorized)
	})
}

// MaxAdminForm bounds an admin form body.
const MaxAdminForm = 64 << 10

// adminForm parses the form under MaxAdminForm before the handler reads it.
func adminForm(next http.HandlerFunc) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		r.Body = http.MaxBytesReader(w, r.Body, MaxAdminForm)
		if err := r.ParseForm(); err != nil {
			code := http.StatusBadRequest
			if _, big := errors.AsType[*http.MaxBytesError](err); big {
				code = http.StatusRequestEntityTooLarge
			}
			http.Error(w, err.Error(), code)
			return
		}
		next(w, r)
	})
}

// bearer returns the request's token: the Authorization header, else ?token= (WebSocket
// clients that cannot set headers; tfctl sets the header).
func bearer(r *http.Request) (string, bool) {
	if t, ok := strings.CutPrefix(r.Header.Get("Authorization"), "Bearer "); ok && t != "" {
		return t, true
	}
	t := r.URL.Query().Get("token")
	return t, t != ""
}

func tokenOK(r *http.Request, want string) bool {
	t, ok := bearer(r)
	return ok && subtle.ConstantTimeCompare([]byte(t), []byte(want)) == 1
}

// TokenOK exposes the check for other packages (MCP).
func (s *Server) TokenOK(r *http.Request) bool { return s.opts.Token == "" || tokenOK(r, s.opts.Token) }

// ---- operations shared by REST, admin and MCP ------------------------------

// ErrInvalid is returned for bad input.
var ErrInvalid = errors.New("invalid input")

// MaxSplashPNG bounds the logo pushed in every config message (decoded bytes).
const MaxSplashPNG = 48 << 10

var rgbRe = regexp.MustCompile(`^#[0-9a-fA-F]{6}$`)

// checkSplashPNG accepts "" (no logo) or standard base64 of a PNG up to MaxSplashPNG bytes.
func checkSplashPNG(s string) error {
	if s == "" {
		return nil
	}
	b, err := base64.StdEncoding.DecodeString(s)
	switch {
	case err != nil:
		return fmt.Errorf("%w: splash_png is not base64: %v", ErrInvalid, err)
	case len(b) > MaxSplashPNG:
		return fmt.Errorf("%w: splash_png is %d bytes, limit %d", ErrInvalid, len(b), MaxSplashPNG)
	case !bytes.HasPrefix(b, []byte("\x89PNG\r\n\x1a\n")):
		return fmt.Errorf("%w: splash_png is not a PNG", ErrInvalid)
	}
	return nil
}

// SetConfig merges non-nil fields into a frame's config, bumps page_version when the
// page is affected, persists, and pushes to the frame if online.
func (s *Server) SetConfig(id string, patch map[string]any) (store.Frame, error) {
	var bad error
	f, err := s.Store.Update(id, func(f *store.Frame) {
		// Apply to copies; the frame takes them only if every field is valid.
		c, name := f.Config, f.Name
		if bad = applyConfig(&c, &name, patch); bad != nil {
			return
		}
		f.Config, f.Name = c, name
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

// Limits on config strings. Every config message carries them to the frame, and MCP bodies
// may be up to 1 MiB, so the field itself is bounded, not just the transport.
const (
	maxConfigText = 256  // name, splash_text, screen_off, screen_on, each tag
	maxConfigURL  = 2048 // url
	maxConfigTags = 32
)

// configString accepts a string (or null, meaning "") of at most max bytes.
func configString(k string, v any, max int) (string, error) {
	if v == nil {
		return "", nil
	}
	str, ok := v.(string)
	if !ok {
		return "", fmt.Errorf("%w: %s must be a string", ErrInvalid, k)
	}
	if len(str) > max {
		return "", fmt.Errorf("%w: %s is %d bytes, limit %d", ErrInvalid, k, len(str), max)
	}
	return str, nil
}

// applyConfig writes patch into c and name, stopping at the first invalid field.
func applyConfig(c *store.Config, name *string, patch map[string]any) error {
	for k, v := range patch {
		var err error
		switch k {
		case "url":
			c.URL, err = configString(k, v, maxConfigURL)
		case "rotation":
			n, ok := asInt(v)
			if !ok || n < 0 || n > 3 {
				return fmt.Errorf("%w: rotation must be 0..3", ErrInvalid)
			}
			c.Rotation = n
		case "brightness":
			n, ok := asInt(v)
			if !ok || n < 0 || n > 100 {
				return fmt.Errorf("%w: brightness must be 0..100", ErrInvalid)
			}
			c.Brightness = n
		case "screen_off":
			c.ScreenOff, err = configString(k, v, maxConfigText)
		case "screen_on":
			c.ScreenOn, err = configString(k, v, maxConfigText)
		case "tags":
			c.Tags, err = configTags(v)
		case "name":
			*name, err = configString(k, v, maxConfigText)
		case "splash_text":
			c.SplashText, err = configString(k, v, maxConfigText)
		case "splash_bg":
			var str string
			if str, err = configString(k, v, maxConfigText); err == nil && str != "" && !rgbRe.MatchString(str) {
				err = fmt.Errorf("%w: splash_bg must be #rrggbb", ErrInvalid)
			}
			c.SplashBG = str
		case "splash_png":
			var str string
			if str, err = configString(k, v, 2*MaxSplashPNG); err == nil {
				err = checkSplashPNG(str)
			}
			c.SplashPNG = str
		default:
			return fmt.Errorf("%w: unknown config field %q", ErrInvalid, k)
		}
		if err != nil {
			return err
		}
	}
	return nil
}

// configTags accepts a list of strings or one comma-separated string, within the limits.
func configTags(v any) ([]string, error) {
	switch v.(type) {
	case nil, string, []string, []any:
	default:
		return nil, fmt.Errorf("%w: tags must be a list of strings", ErrInvalid)
	}
	tags := asStrings(v)
	if len(tags) > maxConfigTags {
		return nil, fmt.Errorf("%w: %d tags, limit %d", ErrInvalid, len(tags), maxConfigTags)
	}
	for _, t := range tags {
		if len(t) > maxConfigText {
			return nil, fmt.Errorf("%w: a tag is %d bytes, limit %d", ErrInvalid, len(t), maxConfigText)
		}
	}
	return tags, nil
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
var ValidActions = []string{"reboot", "restart-kiosk", "reload-page", "screen", "update-software", "update-system", "rollback", "screenshot", "bench"}

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
		"splash_text": c.SplashText, "splash_bg": c.SplashBG, "splash_png": c.SplashPNG,
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
	case errors.Is(err, ErrInvalid), errors.Is(err, store.ErrInvalidID):
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
