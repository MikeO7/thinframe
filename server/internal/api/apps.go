package api

import (
	"bytes"
	"context"
	"crypto/hmac"
	"crypto/sha256"
	"crypto/subtle"
	_ "embed"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"regexp"
	"strings"
	"sync"
	"time"

	"github.com/MikeO7/thinframe/server/internal/apps"
	"github.com/MikeO7/thinframe/server/internal/store"
)

// Apps are web pages MCP writes (internal/apps). The frame shows one full screen
// (/apps/<name>/index.html?frame=<id>) or inside an "app" widget. Every app document is served
// with a CSP sandbox, so it runs in an opaque origin: it cannot read this server's pages, the
// admin page's Basic credentials, or anything else same-origin, and talks to the server only
// through the public endpoints below, which answer any origin. /apps-sdk.js is injected into
// every HTML file and gives the app window.thinframe (docs/apps.md).

//go:embed appsdk.js
var appSDK []byte

// appCSP sandboxes every app document. No allow-same-origin: that would undo the sandbox.
// frame-ancestors: only this server's dashboard may frame an app ('self' is the origin of the
// app's URL, even for a sandboxed document), and the frame's saved dashboard, which opens
// from a file:// URL at boot (tfctl offline.rs). A framed app talks only to its parent, so a
// page that frames one gets nothing but its own postMessages.
const appCSP = "sandbox allow-scripts allow-forms allow-pointer-lock allow-downloads; frame-ancestors 'self' file:"

// pageCSP is the dashboard page's: no other site may frame it (a framed page could be clicked
// into switching lights).
const pageCSP = "frame-ancestors 'self'"

// MaxEmit bounds one emit body; emitBurst emits per emitWindow per frame (or app) are allowed.
const (
	MaxEmit    = 8 << 10
	emitBurst  = 20
	emitWindow = 10 * time.Second
	maxEvents  = 200
)

var headRe = regexp.MustCompile(`(?i)<head(\s[^>]*)?>|<!doctype[^>]*>`)

// EmitKey is what a public emit for frameID must carry (query parameter k) when the server
// has a token (secret): the first 32 hex digits of HMAC-SHA256(secret, "emit:"+frameID). Only
// documents this server serves carry it (the dashboard page, an app shown full screen for that
// frame), and no other origin can read either, so a cross-site page on the LAN cannot fire a
// hook (docs/decisions.md "Apps"). Without a secret (LAN dev mode) it is "" and emit is open.
func EmitKey(secret, frameID string) string {
	if secret == "" {
		return ""
	}
	m := hmac.New(sha256.New, []byte(secret))
	m.Write([]byte("emit:" + frameID))
	return hex.EncodeToString(m.Sum(nil))[:32]
}

// emitKeyOK: the request carries frameID's emit key, or none is needed.
func (s *Server) emitKeyOK(r *http.Request, frameID string) bool {
	want := EmitKey(s.opts.Token, frameID)
	return want == "" || subtle.ConstantTimeCompare([]byte(r.URL.Query().Get("k")), []byte(want)) == 1
}

// injectSDK puts the SDK script first in the document head (or after the doctype, never
// before it: that would switch the page to quirks mode). A non-empty key (hex) goes on the tag
// as data-k; the SDK reads it, removes it and adds it to its emits.
func injectSDK(b []byte, key string) []byte {
	tag := []byte(`<script src="/apps-sdk.js"></script>`)
	if key != "" {
		tag = []byte(`<script src="/apps-sdk.js" data-k="` + key + `"></script>`)
	}
	loc := headRe.FindIndex(b)
	if loc == nil {
		return append(tag, b...)
	}
	// Prefer <head> over the doctype when both exist.
	if m := regexp.MustCompile(`(?i)<head(\s[^>]*)?>`).FindIndex(b); m != nil {
		loc = m
	}
	out := make([]byte, 0, len(b)+len(tag))
	out = append(out, b[:loc[1]]...)
	out = append(out, tag...)
	return append(out, b[loc[1]:]...)
}

// appHeaders: every app file. cors adds Access-Control-Allow-Origin * (an app's own scripts,
// fonts and data, fetched from its opaque origin); never on a document that carries an emit key.
func appHeaders(h http.Header, cors bool) {
	h.Set("Content-Security-Policy", appCSP)
	if cors {
		h.Set("Access-Control-Allow-Origin", "*")
	}
	h.Set("X-Content-Type-Options", "nosniff")
	h.Set("Referrer-Policy", "no-referrer")
	h.Set("Cache-Control", "no-cache")
}

func (s *Server) handleAppFile(w http.ResponseWriter, r *http.Request) {
	name, p := r.PathValue("name"), r.PathValue("path")
	if p == "" {
		p = "index.html"
	}
	f, v, err := s.apps.File(name, p, 0)
	if err != nil {
		http.Error(w, "no such app file", http.StatusNotFound)
		return
	}
	defer f.Close()
	h := w.Header()
	html := strings.HasSuffix(p, ".html")
	etag := fmt.Sprintf("%s-%d-%s", name, v, s.opts.Version)
	// Full screen (?frame=<id>) a document carries that frame's emit key, so no other origin
	// may read it: no CORS header on it.
	key := ""
	if fr := r.URL.Query().Get("frame"); html && store.ValidID(fr) {
		key = EmitKey(s.opts.Token, fr)
	}
	appHeaders(h, key == "")
	if key != "" {
		// A new token is a new key: a cached copy must not revalidate.
		sum := sha256.Sum256([]byte(key))
		etag += "-" + hex.EncodeToString(sum[:4])
	}
	h.Set("Content-Type", apps.ContentType(p))
	h.Set("ETag", `"`+etag+`"`)
	if html {
		b, err := io.ReadAll(io.LimitReader(f, apps.MaxBytes))
		if err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
		http.ServeContent(w, r, "", time.Time{}, bytes.NewReader(injectSDK(b, key)))
		return
	}
	http.ServeContent(w, r, "", time.Time{}, f)
}

func (s *Server) handleAppSDK(w http.ResponseWriter, r *http.Request) {
	h := w.Header()
	h.Set("Content-Type", "text/javascript; charset=utf-8")
	h.Set("Access-Control-Allow-Origin", "*")
	h.Set("X-Content-Type-Options", "nosniff")
	h.Set("Cache-Control", "no-cache")
	h.Set("ETag", `"sdk-`+s.opts.Version+`"`)
	http.ServeContent(w, r, "", time.Time{}, bytes.NewReader(appSDK))
}

// handleDataSource is a source as the apps read it (thinframe.data). Public like the page.
func (s *Server) handleDataSource(w http.ResponseWriter, r *http.Request) {
	appCORS(w, r)
	w.Header().Set("Cache-Control", "no-store")
	data, ok := s.src.get(r.PathValue("key"))
	if !ok {
		http.Error(w, "no such source", http.StatusNotFound)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	_, _ = w.Write(data)
}

// ---- operations (REST, MCP) -----------------------------------------------------------------

// Apps is the app store.
func (s *Server) Apps() *apps.Store { return s.apps }

// PutApp stores a new version and reloads every page showing the app.
func (s *Server) PutApp(name, desc, note string, files map[string][]byte) (apps.Meta, error) {
	m, err := s.apps.Put(name, desc, note, files)
	if err == nil {
		s.sse.publishApps("", map[string]any{"app_reload": name})
	}
	return m, err
}

// RollbackApp makes an older version current (0 = the one before) and reloads its pages.
func (s *Server) RollbackApp(name string, n int) (apps.Meta, error) {
	m, err := s.apps.Rollback(name, n)
	if err == nil {
		s.sse.publishApps("", map[string]any{"app_reload": name})
	}
	return m, err
}

// AppUsers lists the frames that show an app (full screen or in a widget).
func (s *Server) AppUsers(name string) []string {
	var out []string
	for _, f := range s.Store.List() {
		use := appOfURL(f.Config.URL) == name
		for _, w := range f.Layout.Widgets {
			use = use || w.Type == "app" && w.Props["app"] == name
		}
		if use {
			out = append(out, f.ID)
		}
	}
	return out
}

// DeleteApp removes an app no frame shows; a frame never loses its page to a delete.
func (s *Server) DeleteApp(name string) error {
	if users := s.AppUsers(name); len(users) > 0 {
		return fmt.Errorf("%w: app %s is shown on %s; show_app another page (or set_layout without it) first", ErrInvalid, name, strings.Join(users, ", "))
	}
	return s.apps.Delete(name)
}

// AppURL is the page URL that shows an app full screen on a frame.
func AppURL(name, frame string) string { return "/apps/" + name + "/index.html?frame=" + frame }

var appURLRe = regexp.MustCompile(`^/apps/([a-z0-9][a-z0-9_-]{0,31})/`)

func appOfURL(u string) string {
	if m := appURLRe.FindStringSubmatch(u); m != nil {
		return m[1]
	}
	return ""
}

// ShowApp shows an app full screen on a frame; name "" goes back to the dashboard.
func (s *Server) ShowApp(id, name string) (store.Frame, error) {
	if _, err := s.Store.Get(id); err != nil {
		return store.Frame{}, err
	}
	u := ""
	if name != "" {
		if !s.apps.Exists(name) {
			return store.Frame{}, fmt.Errorf("%w: no app %q (put_app first)", apps.ErrNotFound, name)
		}
		u = AppURL(name, id)
	}
	return s.SetConfig(id, map[string]any{"url": u})
}

// checkAppWidgets refuses a layout whose app widgets name an app that is not stored.
func (s *Server) checkAppWidgets(l store.Layout) error {
	for _, w := range l.Widgets {
		if w.Type != "app" {
			continue
		}
		if n, _ := w.Props["app"].(string); !s.apps.Exists(n) {
			return fmt.Errorf("%w: widget %s: no app %q (put_app first)", ErrInvalid, w.ID, n)
		}
	}
	return nil
}

// SetAppState saves one state key and tells every page hosting apps.
func (s *Server) SetAppState(name, key string, value json.RawMessage) error {
	changed, err := s.apps.SetState(name, key, value)
	if err == nil && changed {
		s.sse.publishApps("", map[string]any{"app_state": map[string]any{"app": name, "key": key, "value": value}})
	}
	return err
}

// SendToApp delivers an event to an app's pages (thinframe.listen). frame "" = every frame.
func (s *Server) SendToApp(frame, app, name string, data json.RawMessage) {
	s.sse.publishApps(frame, map[string]any{"app_event": map[string]any{"app": app, "name": name, "data": data}})
}

// ---- events ---------------------------------------------------------------------------------

// AppEvent is one thinframe.emit from an app, with what its hook answered.
type AppEvent struct {
	At     time.Time       `json:"at"`
	Frame  string          `json:"frame"`
	App    string          `json:"app,omitempty"`
	Name   string          `json:"name"`
	Data   json.RawMessage `json:"data,omitempty"`
	Hook   bool            `json:"hook,omitempty"`
	Status int             `json:"status,omitempty"`
	Error  string          `json:"error,omitempty"`
}

// EmitResult is what the app's emit promise resolves to.
type EmitResult struct {
	OK     bool            `json:"ok"`
	Status int             `json:"status,omitempty"`
	Body   json.RawMessage `json:"body,omitempty"`
	Error  string          `json:"error,omitempty"`
}

type eventLog struct {
	mu   sync.Mutex
	list []AppEvent
	hits map[string][]time.Time
}

// allow is a sliding window limiter: at most n hits per window per key.
func (l *eventLog) allow(key string, n int, window time.Duration) bool {
	l.mu.Lock()
	defer l.mu.Unlock()
	now := time.Now()
	if l.hits == nil {
		l.hits = map[string][]time.Time{}
	}
	keep := l.hits[key][:0]
	for _, t := range l.hits[key] {
		if now.Sub(t) < window {
			keep = append(keep, t)
		}
	}
	if len(keep) >= n {
		l.hits[key] = keep
		return false
	}
	l.hits[key] = append(keep, now)
	if len(l.hits) > 1024 { // forget idle keys
		for k, v := range l.hits {
			if len(v) == 0 || now.Sub(v[len(v)-1]) > window {
				delete(l.hits, k)
			}
		}
	}
	return true
}

func (l *eventLog) add(e AppEvent) {
	l.mu.Lock()
	defer l.mu.Unlock()
	l.list = append(l.list, e)
	if len(l.list) > maxEvents {
		l.list = l.list[len(l.list)-maxEvents:]
	}
}

// AppEvents returns the newest n emits (all when n <= 0), filtered by frame and app when set.
func (s *Server) AppEvents(frame, app string, n int) []AppEvent {
	s.events.mu.Lock()
	defer s.events.mu.Unlock()
	out := []AppEvent{}
	for _, e := range s.events.list {
		if (frame == "" || e.Frame == frame) && (app == "" || e.App == app) {
			out = append(out, e)
		}
	}
	if n > 0 && len(out) > n {
		out = out[len(out)-n:]
	}
	return out
}

// Emit records an app event and runs the hook of the same name, if there is one.
func (s *Server) Emit(ctx context.Context, frame, app, name string, data json.RawMessage) EmitResult {
	ev := AppEvent{At: time.Now().UTC(), Frame: frame, App: app, Name: name, Data: data}
	res := EmitResult{OK: true}
	if h, ok := s.hooks.get(name); ok {
		ev.Hook = true
		res = s.runHook(ctx, h, hookVars{frame: frame, app: app, name: name, data: data})
		ev.Status, ev.Error = res.Status, res.Error
	}
	s.events.add(ev)
	return res
}

// readSmall reads a body of at most limit bytes: 413 past it.
func readSmall(w http.ResponseWriter, r *http.Request, limit int64) ([]byte, bool) {
	b, err := io.ReadAll(http.MaxBytesReader(w, r.Body, limit))
	if err != nil {
		code := http.StatusBadRequest
		if _, big := errors.AsType[*http.MaxBytesError](err); big {
			code = http.StatusRequestEntityTooLarge
		}
		http.Error(w, err.Error(), code)
		return nil, false
	}
	return b, true
}

// handleEmit: POST /api/frames/{id}/emit?k=<key> {"app","name","data"}. No token (the page
// and the apps have none), but with a server token it needs the frame's emit key (EmitKey),
// which only documents this server serves carry: a hook can unlock the car, and any web page
// open on the LAN could otherwise POST here. The key is checked first, so a caller without it
// cannot probe which frames exist. Limited per frame; it never creates a frame record.
func (s *Server) handleEmit(w http.ResponseWriter, r *http.Request) {
	appCORS(w, r)
	id := r.PathValue("id")
	if !s.emitKeyOK(r, id) {
		http.Error(w, "emit needs this frame's key (k): only pages this server serves carry it", http.StatusForbidden)
		return
	}
	if _, err := s.Store.Get(id); err != nil {
		http.Error(w, "no such frame", http.StatusNotFound)
		return
	}
	b, ok := readSmall(w, r, MaxEmit)
	if !ok {
		return
	}
	var in struct {
		App  string          `json:"app"`
		Name string          `json:"name"`
		Data json.RawMessage `json:"data"`
	}
	if err := json.Unmarshal(b, &in); err != nil {
		http.Error(w, "body must be JSON {app,name,data}", http.StatusBadRequest)
		return
	}
	if !store.ValidID(in.Name) || in.App != "" && !apps.ValidName(in.App) {
		http.Error(w, "name (and app) must be lowercase ids", http.StatusBadRequest)
		return
	}
	if !s.events.allow("emit:"+id, emitBurst, emitWindow) {
		http.Error(w, "too many events", http.StatusTooManyRequests)
		return
	}
	writeJSON(w, http.StatusOK, s.Emit(r.Context(), id, in.App, in.Name, in.Data))
}

func (s *Server) handleGetAppState(w http.ResponseWriter, r *http.Request) {
	appCORS(w, r)
	w.Header().Set("Cache-Control", "no-store")
	st, err := s.apps.State(r.PathValue("name"))
	if err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, http.StatusOK, st)
}

// handleSetAppState: POST (a simple request, no preflight from the sandbox) with the JSON value.
func (s *Server) handleSetAppState(w http.ResponseWriter, r *http.Request) {
	appCORS(w, r)
	name := r.PathValue("name")
	if !s.apps.Exists(name) {
		http.Error(w, "no such app", http.StatusNotFound)
		return
	}
	b, ok := readSmall(w, r, apps.MaxStateValue)
	if !ok {
		return
	}
	if !s.events.allow("state:"+name, 4*emitBurst, emitWindow) {
		http.Error(w, "too many writes", http.StatusTooManyRequests)
		return
	}
	if err := s.SetAppState(name, r.PathValue("key"), bytes.TrimSpace(b)); err != nil {
		writeErr(w, err)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}
