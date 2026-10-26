// Package store keeps one JSON file per frame under DATA_DIR/frames/{id}.json.
// It is the only place state lives. Writes are atomic (temp file + rename).
package store

import (
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"sync"
	"time"
)

// Config is what a frame obeys. Sent whole on every change.
type Config struct {
	URL             string   `json:"url,omitempty"` // override; empty = /frame/{id} on this server
	Rotation        int      `json:"rotation"`      // quarter turns 0..3
	Brightness      int      `json:"brightness"`    // 0..100
	ScreenOff       string   `json:"screen_off,omitempty"`
	ScreenOn        string   `json:"screen_on,omitempty"`
	PageVersion     int      `json:"page_version"`
	SoftwareVersion string   `json:"software_version,omitempty"`
	SystemVersion   string   `json:"system_version,omitempty"`
	Tags            []string `json:"tags,omitempty"`
	SplashText      string   `json:"splash_text,omitempty"` // boot splash line (TASK 15)
	SplashBG        string   `json:"splash_bg,omitempty"`   // "#rrggbb"
	SplashPNG       string   `json:"splash_png,omitempty"`  // base64 PNG, at most MaxSplashPNG bytes decoded
	Debug           bool     `json:"debug"`                 // tfctl logs everything and reports on connect
	// The light sensor beyond "dark room = off" (tfctl sense.rs).
	AutoBrightness bool `json:"auto_brightness"` // backlight follows the room light, up to Brightness
	Wave           bool `json:"wave"`            // a hand over the sensor wakes the screen, or turns the page to the next tab
	MotionWake     bool `json:"motion_wake"`     // a shadow passing wakes the screen (lamp-lit rooms; experimental)
}

// Dir is where the store keeps its files (<data>/frames).
func (s *Store) Dir() string { return s.dir }

// Heartbeat is what a frame reports.
type Heartbeat struct {
	At            time.Time `json:"at"`
	UptimeS       int64     `json:"uptime_s"`
	MemUsedMB     int       `json:"mem_used_mb"`
	MemTotalMB    int       `json:"mem_total_mb"`
	IP            string    `json:"ip"`
	WifiRSSI      int       `json:"wifi_rssi"`
	TempC         float64   `json:"temp_c"`
	TfctlVersion  string    `json:"tfctl_version"`
	SystemVersion string    `json:"system_version"`
	Slot          string    `json:"slot"`
	PageURL       string    `json:"page_url"`
	ScreenState   string    `json:"screen_state"`
	CogPID        int       `json:"cog_pid"`
	LoadAvg       float64   `json:"load_avg"`
	Light         *int      `json:"light,omitempty"` // light sensor 0..63 (dark room 0..3, lit 60..62)
}

// Touch is the page's own touch-to-paint measurement (its beacon), kept beside the heartbeat so a
// heartbeat does not wipe it. PaintedAtMS is the frame's wall clock when the response was on
// screen; `tfctl bench` compares it with the moment it injected the tap.
type Touch struct {
	LastMS      int       `json:"last_ms"`
	PaintedAtMS int64     `json:"painted_at_ms,omitempty"`
	SamplesMS   []int     `json:"samples_ms"` // oldest first, at most MaxTouchSamples
	At          time.Time `json:"at"`
}

// MaxTouchSamples bounds Touch.SamplesMS.
const MaxTouchSamples = 50

// Add records one measurement.
func (t *Touch) Add(ms int, paintedAtMS int64, at time.Time) {
	t.LastMS, t.PaintedAtMS, t.At = ms, paintedAtMS, at
	t.SamplesMS = append(t.SamplesMS, ms)
	if n := len(t.SamplesMS); n > MaxTouchSamples {
		t.SamplesMS = append([]int(nil), t.SamplesMS[n-MaxTouchSamples:]...)
	}
}

// P50 is the median sample (0 with none).
func (t Touch) P50() int {
	if len(t.SamplesMS) == 0 {
		return 0
	}
	s := append([]int(nil), t.SamplesMS...)
	sort.Ints(s)
	return s[len(s)/2]
}

// Max is the slowest sample.
func (t Touch) Max() int {
	m := 0
	for _, v := range t.SamplesMS {
		m = max(m, v)
	}
	return m
}

// Action is a one-shot command. It stays pending until the frame acks it.
type Action struct {
	ID        string            `json:"id"`
	Name      string            `json:"name"`
	Args      map[string]string `json:"args,omitempty"`
	CreatedAt time.Time         `json:"created_at"`
	AckedAt   *time.Time        `json:"acked_at,omitempty"`
	OK        *bool             `json:"ok,omitempty"`
	Error     string            `json:"error,omitempty"`
}

// Widget is one placed widget on the grid.
type Widget struct {
	ID    string         `json:"id"`
	Type  string         `json:"type"`
	X     int            `json:"x"`
	Y     int            `json:"y"`
	W     int            `json:"w"`
	H     int            `json:"h"`
	Tab   string         `json:"tab,omitempty"` // Tab.ID it sits on; "" = the first tab
	Props map[string]any `json:"props,omitempty"`
}

// Tab is one page of a tabbed layout. A tab with Parent set has no button of its own: it is a
// second page of its parent, picked from a switch next to the parent's button.
type Tab struct {
	ID     string `json:"id"`
	Title  string `json:"title"`
	Icon   string `json:"icon,omitempty"`
	Parent string `json:"parent,omitempty"`
	// Label names the page in its tab's segmented switch; empty = Title.
	Label string `json:"label,omitempty"`
}

// Layout is the dashboard composition for one frame. Every tab uses the same Cols x Rows grid.
type Layout struct {
	Cols    int      `json:"cols"`
	Rows    int      `json:"rows"`
	Theme   string   `json:"theme,omitempty"`
	Tabs    []Tab    `json:"tabs,omitempty"`
	IdleS   int      `json:"idle_s,omitempty"` // back to the first tab after this many idle seconds; 0 = 90
	Nav     string   `json:"nav,omitempty"`    // where the tab bar sits: "left" (default) or "bottom"
	Widgets []Widget `json:"widgets"`
}

// Event is a line in a frame's log.
type Event struct {
	At   time.Time `json:"at"`
	Kind string    `json:"kind"`
	Text string    `json:"text"`
}

// Frame is everything known about one frame.
type Frame struct {
	ID        string     `json:"id"`
	Name      string     `json:"name,omitempty"`
	Config    Config     `json:"config"`
	Layout    Layout     `json:"layout"`
	FirstSeen time.Time  `json:"first_seen"`
	LastSeen  time.Time  `json:"last_seen"`
	Last      *Heartbeat `json:"last_heartbeat,omitempty"`
	Touch     *Touch     `json:"touch,omitempty"`
	Pending   []Action   `json:"pending_actions"`
	History   []Action   `json:"action_history"`
	Events    []Event    `json:"events"`
	// TokenHash is sha256(hex) of the token minted by one-press pairing; empty = not paired.
	TokenHash string     `json:"token_hash,omitempty"`
	PairedAt  *time.Time `json:"paired_at,omitempty"`
}

var idRe = regexp.MustCompile(`^[a-z0-9][a-z0-9_-]{0,31}$`)

// ValidID reports whether id is safe for file names and URLs.
func ValidID(id string) bool { return idRe.MatchString(id) }

// DefaultConfig is what a brand new frame gets.
func DefaultConfig() Config { return Config{Rotation: 3, Brightness: 100, PageVersion: 1} }

// DefaultLayout is a clock + photo + status page.
func DefaultLayout() Layout {
	return Layout{Cols: 4, Rows: 3, Theme: "dark", Widgets: []Widget{
		{ID: "clock", Type: "clock", X: 0, Y: 0, W: 2, H: 1},
		{ID: "photo", Type: "photo", X: 2, Y: 0, W: 2, H: 3, Props: map[string]any{"interval_s": 600}},
		{ID: "weather", Type: "weather", X: 0, Y: 1, W: 2, H: 1},
		{ID: "status", Type: "status", X: 0, Y: 2, W: 2, H: 1},
	}}
}

// Store is a directory of frame files with an in-memory cache.
type Store struct {
	dir    string
	mu     sync.RWMutex
	frames map[string]*Frame
}

// Open loads every frame file under dir/frames.
func Open(dir string) (*Store, error) {
	s := &Store{dir: filepath.Join(dir, "frames"), frames: map[string]*Frame{}}
	if err := os.MkdirAll(s.dir, 0o755); err != nil {
		return nil, err
	}
	entries, err := os.ReadDir(s.dir)
	if err != nil {
		return nil, err
	}
	for _, e := range entries {
		if !strings.HasSuffix(e.Name(), ".json") {
			continue
		}
		b, err := os.ReadFile(filepath.Join(s.dir, e.Name()))
		if err != nil {
			return nil, err
		}
		var f Frame
		if err := json.Unmarshal(b, &f); err != nil || f.ID == "" {
			// A torn or empty file after a power cut must not keep every frame dark: set it
			// aside and carry on; the frame recreates itself on its next heartbeat.
			bad := filepath.Join(s.dir, e.Name()+".bad")
			log.Printf("store: %s: %v; moved to %s", e.Name(), err, filepath.Base(bad))
			_ = os.Rename(filepath.Join(s.dir, e.Name()), bad)
			continue
		}
		s.frames[f.ID] = &f
	}
	return s, nil
}

// ErrInvalidID is returned for ids ValidID refuses.
var ErrInvalidID = errors.New("invalid frame id")

// ErrNotFound is returned for unknown frame ids.
var ErrNotFound = errors.New("frame not found")

// Get returns a copy of a frame.
func (s *Store) Get(id string) (Frame, error) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	f, ok := s.frames[id]
	if !ok {
		return Frame{}, ErrNotFound
	}
	return clone(*f), nil
}

// List returns copies of all frames sorted by id.
func (s *Store) List() []Frame {
	s.mu.RLock()
	defer s.mu.RUnlock()
	out := make([]Frame, 0, len(s.frames))
	for _, f := range s.frames {
		out = append(out, clone(*f))
	}
	sort.Slice(out, func(i, j int) bool { return out[i].ID < out[j].ID })
	return out
}

// Ensure returns the frame, creating it with defaults when unknown.
func (s *Store) Ensure(id string) (Frame, error) {
	if !ValidID(id) {
		return Frame{}, fmt.Errorf("%w %q", ErrInvalidID, id)
	}
	return s.Update(id, func(f *Frame) {})
}

// Update applies fn under the lock and persists. Creates the frame if needed.
func (s *Store) Update(id string, fn func(*Frame)) (Frame, error) {
	if !ValidID(id) {
		return Frame{}, fmt.Errorf("%w %q", ErrInvalidID, id)
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	var f Frame
	if cur, ok := s.frames[id]; ok {
		f = clone(*cur)
	} else {
		now := time.Now().UTC()
		f = Frame{ID: id, Config: DefaultConfig(), Layout: DefaultLayout(), FirstSeen: now, LastSeen: now}
	}
	fn(&f)
	if len(f.History) > 50 {
		f.History = f.History[len(f.History)-50:]
	}
	if len(f.Events) > 200 {
		f.Events = f.Events[len(f.Events)-200:]
	}
	if len(f.Pending) > MaxPending {
		f.Pending = f.Pending[len(f.Pending)-MaxPending:]
	}
	// fn ran on a copy: memory takes the change only once it is on disk, so a failed save
	// (full disk) never leaves the server believing something the next restart will not.
	if err := s.save(&f); err != nil {
		return Frame{}, err
	}
	s.frames[id] = &f
	return clone(f), nil
}

// MaxPending caps the actions queued for an offline frame; older ones are dropped. It matches
// the hub's send buffer, so a reconnect replays all of them.
const MaxPending = 32

// Delete removes a frame and its file.
func (s *Store) Delete(id string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, ok := s.frames[id]; !ok {
		return ErrNotFound
	}
	delete(s.frames, id)
	return os.Remove(filepath.Join(s.dir, id+".json"))
}

func (s *Store) save(f *Frame) error {
	b, err := json.MarshalIndent(f, "", "  ")
	if err != nil {
		return err
	}
	final := filepath.Join(s.dir, f.ID+".json")
	tmp := final + ".tmp"
	// Durable rename: the bytes reach the disk before the name does, and the directory entry
	// is flushed after, so a power cut leaves either the old file or the whole new one.
	w, err := os.OpenFile(tmp, os.O_WRONLY|os.O_CREATE|os.O_TRUNC, 0o644)
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
	if err := os.Rename(tmp, final); err != nil {
		return err
	}
	if d, err := os.Open(s.dir); err == nil {
		_ = d.Sync()
		_ = d.Close()
	}
	return nil
}

func clone(f Frame) Frame {
	b, _ := json.Marshal(f)
	var c Frame
	_ = json.Unmarshal(b, &c)
	return c
}

// AddEvent appends a log line.
func AddEvent(f *Frame, kind, text string) {
	f.Events = append(f.Events, Event{At: time.Now().UTC(), Kind: kind, Text: text})
}

// Ack resolves a pending action by id, moving it to history.
func Ack(f *Frame, id string, ok bool, errText string) bool {
	for i, a := range f.Pending {
		if a.ID != id {
			continue
		}
		now := time.Now().UTC()
		a.AckedAt, a.OK, a.Error = &now, &ok, errText
		f.Pending = append(f.Pending[:i], f.Pending[i+1:]...)
		f.History = append(f.History, a)
		AddEvent(f, "ack", fmt.Sprintf("%s ok=%v %s", a.Name, ok, errText))
		return true
	}
	return false
}
