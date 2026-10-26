// Package apps stores web apps that MCP writes: a folder of HTML, CSS, JS and images that a
// frame shows full screen or inside an "app" widget. Every put is a new version; the last
// MaxVersions are kept so a bad change can be rolled back. Apps live in DATA_DIR/apps/<name>/
// as v<N>/ folders plus meta.json.
package apps

import (
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"
)

// Limits for one app version.
const (
	MaxFiles    = 64
	MaxBytes    = 4 << 20
	MaxVersions = 10
	MaxApps     = 64
	MaxDesc     = 500
)

// ErrInvalid marks a refused put (bad name, path, type or size).
var ErrInvalid = errors.New("invalid")

// ErrNotFound marks an unknown app, version or file.
var ErrNotFound = errors.New("not found")

var (
	nameRe = regexp.MustCompile(`^[a-z0-9][a-z0-9_-]{0,31}$`)
	segRe  = regexp.MustCompile(`^[A-Za-z0-9_-][A-Za-z0-9._-]{0,63}$`)
)

// Types maps the extensions an app may contain to their Content-Type.
var Types = map[string]string{
	".html": "text/html; charset=utf-8", ".css": "text/css; charset=utf-8",
	".js": "text/javascript; charset=utf-8", ".mjs": "text/javascript; charset=utf-8",
	".json": "application/json", ".svg": "image/svg+xml", ".txt": "text/plain; charset=utf-8",
	".png": "image/png", ".jpg": "image/jpeg", ".jpeg": "image/jpeg", ".webp": "image/webp",
	".gif": "image/gif", ".woff2": "font/woff2",
}

// Version is one stored put.
type Version struct {
	N     int       `json:"n"`
	At    time.Time `json:"at"`
	Note  string    `json:"note,omitempty"`
	Files int       `json:"files"`
	Bytes int       `json:"bytes"`
}

// Meta describes an app: which version is live and which are kept.
type Meta struct {
	Name        string    `json:"name"`
	Description string    `json:"description,omitempty"`
	Current     int       `json:"current"`
	Versions    []Version `json:"versions"`
}

// Store is the apps folder.
type Store struct {
	mu  sync.Mutex
	dir string
}

// Open returns the store rooted at dir (created on first put).
func Open(dir string) *Store { return &Store{dir: dir} }

// ValidName reports whether name can name an app.
func ValidName(name string) bool { return nameRe.MatchString(name) }

// ValidPath reports whether p is a relative file path an app may contain: up to four
// segments of letters, digits, '.', '_' and '-', none starting with a dot, with a known
// extension.
func ValidPath(p string) bool {
	segs := strings.Split(p, "/")
	if len(p) > 200 || len(segs) > 4 {
		return false
	}
	for _, s := range segs {
		if !segRe.MatchString(s) {
			return false
		}
	}
	_, ok := Types[strings.ToLower(path.Ext(p))]
	return ok
}

// ContentType is the type served for p.
func ContentType(p string) string { return Types[strings.ToLower(path.Ext(p))] }

func (s *Store) appDir(name string) string { return filepath.Join(s.dir, name) }

func (s *Store) readMeta(name string) (Meta, error) {
	b, err := os.ReadFile(filepath.Join(s.appDir(name), "meta.json"))
	if errors.Is(err, fs.ErrNotExist) {
		return Meta{}, fmt.Errorf("%w: app %q", ErrNotFound, name)
	}
	if err != nil {
		return Meta{}, err
	}
	var m Meta
	if err := json.Unmarshal(b, &m); err != nil {
		return Meta{}, fmt.Errorf("app %s: meta.json: %w", name, err)
	}
	return m, nil
}

func (s *Store) writeMeta(m Meta) error {
	b, err := json.MarshalIndent(m, "", "  ")
	if err != nil {
		return err
	}
	p := filepath.Join(s.appDir(m.Name), "meta.json")
	tmp := p + ".tmp"
	if err := os.WriteFile(tmp, b, 0o600); err != nil {
		return err
	}
	return os.Rename(tmp, p)
}

// Check validates a set of files without storing them.
func Check(name string, files map[string][]byte) error {
	if !ValidName(name) {
		return fmt.Errorf("%w: app name %q (lowercase letters, digits, - and _, up to 32)", ErrInvalid, name)
	}
	if len(files) == 0 || len(files) > MaxFiles {
		return fmt.Errorf("%w: %d files, need 1..%d", ErrInvalid, len(files), MaxFiles)
	}
	if _, ok := files["index.html"]; !ok {
		return fmt.Errorf("%w: an app needs index.html", ErrInvalid)
	}
	total := 0
	for p, b := range files {
		if !ValidPath(p) {
			return fmt.Errorf("%w: file path %q (relative, a-z 0-9 . _ -, no leading dot, up to 4 levels; types: %s)", ErrInvalid, p, strings.Join(Extensions(), " "))
		}
		total += len(b)
	}
	if total > MaxBytes {
		return fmt.Errorf("%w: %d bytes, limit %d", ErrInvalid, total, MaxBytes)
	}
	return nil
}

// Extensions lists the allowed file extensions, sorted.
func Extensions() []string {
	out := make([]string, 0, len(Types))
	for k := range Types {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}

// Put stores files as the app's new current version. An empty description keeps the old one.
func (s *Store) Put(name, description, note string, files map[string][]byte) (Meta, error) {
	if err := Check(name, files); err != nil {
		return Meta{}, err
	}
	if len(description) > MaxDesc || len(note) > MaxDesc {
		return Meta{}, fmt.Errorf("%w: description and note are limited to %d bytes", ErrInvalid, MaxDesc)
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	m, err := s.readMeta(name)
	if errors.Is(err, ErrNotFound) {
		if n := len(s.names()); n >= MaxApps {
			return Meta{}, fmt.Errorf("%w: %d apps, limit %d", ErrInvalid, n, MaxApps)
		}
		m, err = Meta{Name: name}, nil
	}
	if err != nil {
		return Meta{}, err
	}
	n := 1
	for _, v := range m.Versions {
		n = max(n, v.N+1)
	}
	vdir := filepath.Join(s.appDir(name), "v"+strconv.Itoa(n))
	tmp := vdir + ".tmp"
	_ = os.RemoveAll(tmp)
	total := 0
	for p, b := range files {
		dst := filepath.Join(tmp, filepath.FromSlash(p))
		if err := os.MkdirAll(filepath.Dir(dst), 0o700); err != nil {
			return Meta{}, err
		}
		if err := os.WriteFile(dst, b, 0o600); err != nil {
			_ = os.RemoveAll(tmp)
			return Meta{}, err
		}
		total += len(b)
	}
	if err := os.Rename(tmp, vdir); err != nil {
		_ = os.RemoveAll(tmp)
		return Meta{}, err
	}
	if description != "" {
		m.Description = description
	}
	m.Current = n
	m.Versions = append(m.Versions, Version{N: n, At: time.Now().UTC(), Note: note, Files: len(files), Bytes: total})
	for len(m.Versions) > MaxVersions {
		_ = os.RemoveAll(filepath.Join(s.appDir(name), "v"+strconv.Itoa(m.Versions[0].N)))
		m.Versions = m.Versions[1:]
	}
	return m, s.writeMeta(m)
}

// Get returns an app's meta.
func (s *Store) Get(name string) (Meta, error) {
	if !ValidName(name) {
		return Meta{}, fmt.Errorf("%w: app %q", ErrNotFound, name)
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.readMeta(name)
}

// Exists reports whether an app is stored.
func (s *Store) Exists(name string) bool { _, err := s.Get(name); return err == nil }

func (s *Store) names() []string {
	ents, _ := os.ReadDir(s.dir)
	var out []string
	for _, e := range ents {
		if e.IsDir() && ValidName(e.Name()) {
			if _, err := os.Stat(filepath.Join(s.dir, e.Name(), "meta.json")); err == nil {
				out = append(out, e.Name())
			}
		}
	}
	return out
}

// List returns every app's meta, by name.
func (s *Store) List() []Meta {
	s.mu.Lock()
	defer s.mu.Unlock()
	out := []Meta{}
	for _, n := range s.names() {
		if m, err := s.readMeta(n); err == nil {
			out = append(out, m)
		}
	}
	return out
}

// Delete removes an app and all its versions.
func (s *Store) Delete(name string) error {
	if _, err := s.Get(name); err != nil {
		return err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	return os.RemoveAll(s.appDir(name))
}

// Rollback makes version n current; n 0 means the version before the current one.
func (s *Store) Rollback(name string, n int) (Meta, error) {
	if !ValidName(name) {
		return Meta{}, fmt.Errorf("%w: app %q", ErrNotFound, name)
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	m, err := s.readMeta(name)
	if err != nil {
		return m, err
	}
	if n == 0 {
		for _, v := range m.Versions {
			if v.N < m.Current {
				n = v.N
			}
		}
		if n == 0 {
			return m, fmt.Errorf("%w: no version before v%d", ErrNotFound, m.Current)
		}
	}
	found := false
	for _, v := range m.Versions {
		found = found || v.N == n
	}
	if !found {
		return m, fmt.Errorf("%w: app %s has no kept version %d", ErrNotFound, name, n)
	}
	m.Current = n
	return m, s.writeMeta(m)
}

// File opens one file of the current version (or version v when v > 0).
func (s *Store) File(name, p string, v int) (*os.File, int, error) {
	if !ValidName(name) || !ValidPath(p) {
		return nil, 0, fmt.Errorf("%w: %s/%s", ErrNotFound, name, p)
	}
	m, err := s.Get(name)
	if err != nil {
		return nil, 0, err
	}
	if v <= 0 {
		v = m.Current
	}
	f, err := os.Open(filepath.Join(s.appDir(name), "v"+strconv.Itoa(v), filepath.FromSlash(p)))
	if err != nil {
		return nil, 0, fmt.Errorf("%w: %s/%s", ErrNotFound, name, p)
	}
	if st, err := f.Stat(); err != nil || !st.Mode().IsRegular() {
		_ = f.Close()
		return nil, 0, fmt.Errorf("%w: %s/%s", ErrNotFound, name, p)
	}
	return f, v, nil
}

// Files returns every file of a version (current when v <= 0), for get_app.
func (s *Store) Files(name string, v int) (map[string][]byte, int, error) {
	m, err := s.Get(name)
	if err != nil {
		return nil, 0, err
	}
	if v <= 0 {
		v = m.Current
	}
	root := filepath.Join(s.appDir(name), "v"+strconv.Itoa(v))
	out := map[string][]byte{}
	err = filepath.WalkDir(root, func(p string, d fs.DirEntry, err error) error {
		if err != nil || d.IsDir() {
			return err
		}
		rel, _ := filepath.Rel(root, p)
		b, err := os.ReadFile(p)
		out[filepath.ToSlash(rel)] = b
		return err
	})
	if errors.Is(err, fs.ErrNotExist) {
		return nil, 0, fmt.Errorf("%w: app %s version %d", ErrNotFound, name, v)
	}
	return out, v, err
}

// State limits: what an app's pages may keep on the server (thinframe.state in the SDK).
const (
	MaxStateValue = 16 << 10
	MaxState      = 64 << 10
	MaxStateKeys  = 64
)

// State returns an app's saved state (key -> JSON value).
func (s *Store) State(name string) (map[string]json.RawMessage, error) {
	if _, err := s.Get(name); err != nil {
		return nil, err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.readState(name), nil
}

func (s *Store) readState(name string) map[string]json.RawMessage {
	out := map[string]json.RawMessage{}
	if b, err := os.ReadFile(filepath.Join(s.appDir(name), "state.json")); err == nil {
		_ = json.Unmarshal(b, &out)
	}
	return out
}

// SetState saves one key; a JSON null deletes it. It reports whether anything changed.
func (s *Store) SetState(name, key string, value json.RawMessage) (bool, error) {
	if !nameRe.MatchString(key) {
		return false, fmt.Errorf("%w: state key %q (lowercase letters, digits, - and _)", ErrInvalid, key)
	}
	if len(value) > MaxStateValue || !json.Valid(value) {
		return false, fmt.Errorf("%w: state value must be JSON of at most %d bytes", ErrInvalid, MaxStateValue)
	}
	if _, err := s.Get(name); err != nil {
		return false, err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	st := s.readState(name)
	old, had := st[key]
	if string(value) == "null" {
		if !had {
			return false, nil
		}
		delete(st, key)
	} else {
		if had && string(old) == string(value) {
			return false, nil
		}
		st[key] = value
	}
	total := 0
	for k, v := range st {
		total += len(k) + len(v)
	}
	if len(st) > MaxStateKeys || total > MaxState {
		return false, fmt.Errorf("%w: app state is limited to %d keys and %d bytes", ErrInvalid, MaxStateKeys, MaxState)
	}
	b, err := json.Marshal(st)
	if err != nil {
		return false, err
	}
	p := filepath.Join(s.appDir(name), "state.json")
	if err := os.WriteFile(p+".tmp", b, 0o600); err != nil {
		return false, err
	}
	return true, os.Rename(p+".tmp", p)
}
