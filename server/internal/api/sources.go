package api

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/MikeO7/thinframe/server/internal/store"
	"github.com/MikeO7/thinframe/server/internal/widgets"
)

// Sources are named JSON objects that something outside the server pushes (a script, a home
// automation hub, MCP). A widget with props.source = key receives the object as its data.
// They live in DATA_DIR/sources, so what a private frame shows never has to be in the layout
// shipped with this repo.

// MaxSource bounds one source object, and MaxSources how many are kept.
const (
	MaxSource  = 256 << 10
	MaxSources = 64
)

type sourceSet struct {
	mu   sync.RWMutex
	dir  string
	data map[string]sourceEntry
}

type sourceEntry struct {
	Data    json.RawMessage `json:"data"`
	Updated time.Time       `json:"updated"`
}

// SourceInfo is what GET /api/sources lists.
type SourceInfo struct {
	Key     string    `json:"key"`
	Bytes   int       `json:"bytes"`
	Updated time.Time `json:"updated"`
}

func loadSources(dir string) *sourceSet {
	s := &sourceSet{dir: dir, data: map[string]sourceEntry{}}
	ents, _ := os.ReadDir(dir)
	for _, e := range ents {
		key, ok := strings.CutSuffix(e.Name(), ".json")
		if !ok || !store.ValidID(key) {
			continue
		}
		b, err := os.ReadFile(filepath.Join(dir, e.Name()))
		var ent sourceEntry
		if err == nil && json.Unmarshal(b, &ent) == nil && len(ent.Data) > 0 {
			s.data[key] = ent
		}
	}
	return s
}

func (s *sourceSet) get(key string) (json.RawMessage, bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	e, ok := s.data[key]
	return e.Data, ok
}

func (s *sourceSet) list() []SourceInfo {
	s.mu.RLock()
	defer s.mu.RUnlock()
	out := make([]SourceInfo, 0, len(s.data))
	for k, e := range s.data {
		out = append(out, SourceInfo{Key: k, Bytes: len(e.Data), Updated: e.Updated})
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Key < out[j].Key })
	return out
}

// put stores data and reports whether it differs from what was stored. An unchanged source
// is not rewritten, so a poller that pushes the same state every minute costs nothing.
func (s *sourceSet) put(key string, data json.RawMessage) (bool, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	old, ok := s.data[key]
	if ok && bytes.Equal(old.Data, data) {
		return false, nil
	}
	if !ok && len(s.data) >= MaxSources {
		return false, fmt.Errorf("%w: %d sources, limit %d", ErrInvalid, len(s.data), MaxSources)
	}
	ent := sourceEntry{Data: data, Updated: time.Now().UTC()}
	b, err := json.Marshal(ent)
	if err != nil {
		return false, err
	}
	if err := os.MkdirAll(s.dir, 0o700); err != nil {
		return false, err
	}
	if err := writeDurable(filepath.Join(s.dir, key+".json"), b); err != nil {
		return false, err
	}
	s.data[key] = ent
	return true, nil
}

func (s *sourceSet) delete(key string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, ok := s.data[key]; !ok {
		return false
	}
	delete(s.data, key)
	_ = os.Remove(filepath.Join(s.dir, key+".json"))
	return true
}

// writeDurable writes b to a temporary file, syncs it, renames it over path and syncs the
// directory, so a power cut leaves either the old file or the whole new one (as store.save).
func writeDurable(path string, b []byte) error {
	tmp := path + ".tmp"
	w, err := os.OpenFile(tmp, os.O_WRONLY|os.O_CREATE|os.O_TRUNC, 0o600)
	if err != nil {
		return err
	}
	if _, err := w.Write(b); err != nil {
		_ = w.Close()
		return err
	}
	if err := w.Sync(); err != nil {
		_ = w.Close()
		return err
	}
	if err := w.Close(); err != nil {
		return err
	}
	if err := os.Rename(tmp, path); err != nil {
		return err
	}
	if d, err := os.Open(filepath.Dir(path)); err == nil {
		_ = d.Sync()
		_ = d.Close()
	}
	return nil
}

// checkSource accepts a JSON object and returns it compacted.
func checkSource(key string, raw []byte) (json.RawMessage, error) {
	if !store.ValidID(key) {
		return nil, fmt.Errorf("%w: source key %q (lowercase letters, digits, - and _)", ErrInvalid, key)
	}
	if len(raw) > MaxSource {
		return nil, fmt.Errorf("%w: source is %d bytes, limit %d", ErrInvalid, len(raw), MaxSource)
	}
	var obj map[string]any
	if err := json.Unmarshal(raw, &obj); err != nil || obj == nil {
		return nil, fmt.Errorf("%w: source must be a JSON object", ErrInvalid)
	}
	var buf bytes.Buffer
	if err := json.Compact(&buf, raw); err != nil {
		return nil, fmt.Errorf("%w: %v", ErrInvalid, err)
	}
	return buf.Bytes(), nil
}

// SetSource stores a source and publishes it to every widget, on every frame, that reads it.
func (s *Server) SetSource(key string, raw []byte) error {
	data, err := checkSource(key, raw)
	if err != nil {
		return err
	}
	changed, err := s.src.put(key, data)
	if err != nil || !changed {
		return err
	}
	for _, f := range s.Store.List() {
		for _, w := range f.Layout.Widgets {
			if reads(w, key) {
				s.sse.publish(f.ID, map[string]any{"widget": w.ID, "data": data})
			}
		}
	}
	s.sse.publishApps("", map[string]any{"source": key, "data": data})
	return nil
}

// Sources lists the stored sources.
func (s *Server) Sources() []SourceInfo { return s.src.list() }

// Source returns one stored source.
func (s *Server) Source(key string) (json.RawMessage, bool) { return s.src.get(key) }

// DeleteSource forgets a source. Pages keep what they last showed until they reload.
func (s *Server) DeleteSource(key string) bool { return s.src.delete(key) }

// sourceOf is the widget's main source; reads also lists health, the services feed a hero
// summarizes in its badge.
func sourceOf(w store.Widget) string {
	k, _ := w.Props["source"].(string)
	return k
}

func reads(w store.Widget, key string) bool {
	h, _ := w.Props["health"].(string)
	return key != "" && (sourceOf(w) == key || h == key)
}

// pushSources replays the stored sources to a frame's widgets (on SSE connect).
func (s *Server) pushSources(f store.Frame) {
	for _, w := range f.Layout.Widgets {
		for _, p := range []string{"source", "health"} {
			if k, _ := w.Props[p].(string); k != "" {
				if data, ok := s.src.get(k); ok {
					s.sse.publish(f.ID, map[string]any{"widget": w.ID, "data": data})
				}
			}
		}
	}
}

func (s *Server) handlePutSource(w http.ResponseWriter, r *http.Request) {
	b, err := io.ReadAll(http.MaxBytesReader(w, r.Body, MaxSource+1))
	if err != nil {
		code := http.StatusBadRequest
		if _, big := errors.AsType[*http.MaxBytesError](err); big {
			code = http.StatusRequestEntityTooLarge
		}
		http.Error(w, err.Error(), code)
		return
	}
	if err := s.SetSource(r.PathValue("key"), b); err != nil {
		writeErr(w, err)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func (s *Server) handleGetSource(w http.ResponseWriter, r *http.Request) {
	data, ok := s.src.get(r.PathValue("key"))
	if !ok {
		http.Error(w, "no such source", http.StatusNotFound)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	_, _ = w.Write(data)
}

func (s *Server) handleListSources(w http.ResponseWriter, _ *http.Request) {
	writeJSON(w, http.StatusOK, s.src.list())
}

func (s *Server) handleDeleteSource(w http.ResponseWriter, r *http.Request) {
	if !s.src.delete(r.PathValue("key")) {
		http.Error(w, "no such source", http.StatusNotFound)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

// ---- assets -------------------------------------------------------------------------------

// Assets are image and font files the server owner drops into DATA_DIR/assets (car pictures,
// a font). Widgets refer to them as "asset:<name>". They are public like the page: the frame's
// browser has no token. No SVG or HTML, so a file opened directly cannot run script.
var (
	assetRe    = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$`)
	assetTypes = map[string]string{".png": "image/png", ".jpg": "image/jpeg", ".jpeg": "image/jpeg",
		".webp": "image/webp", ".gif": "image/gif", ".woff2": "font/woff2"}
)

func (s *Server) handleAsset(w http.ResponseWriter, r *http.Request) {
	name := r.PathValue("name")
	ct, ok := assetTypes[strings.ToLower(filepath.Ext(name))]
	if !assetRe.MatchString(name) || !ok {
		http.Error(w, "bad asset name", http.StatusBadRequest)
		return
	}
	f, err := os.Open(filepath.Join(s.assets, name))
	if err != nil {
		http.Error(w, "no such asset", http.StatusNotFound)
		return
	}
	defer f.Close()
	st, err := f.Stat()
	if err != nil || !st.Mode().IsRegular() {
		http.Error(w, "no such asset", http.StatusNotFound)
		return
	}
	w.Header().Set("Content-Type", ct)
	// A theme font among the assets must also load into the frame's saved dashboard, which
	// opens from a file:// URL at boot (tfctl offline.rs).
	w.Header().Set("Access-Control-Allow-Origin", "*")
	w.Header().Set("X-Content-Type-Options", "nosniff")
	w.Header().Set("Cache-Control", "max-age=3600")
	http.ServeContent(w, r, name, st.ModTime(), f)
}

// handleFont serves the fonts built into the server (widgets.Font). Public like the page; apps
// may use them too.
func (s *Server) handleFont(w http.ResponseWriter, r *http.Request) {
	b, ct, ok := widgets.Font(r.PathValue("name"))
	if !ok {
		http.Error(w, "no such font", http.StatusNotFound)
		return
	}
	w.Header().Set("Content-Type", ct)
	w.Header().Set("Access-Control-Allow-Origin", "*")
	w.Header().Set("X-Content-Type-Options", "nosniff")
	w.Header().Set("Cache-Control", "max-age=86400")
	_, _ = w.Write(b)
}
