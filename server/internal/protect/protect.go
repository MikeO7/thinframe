// Package protect reads UniFi Protect through its official Integration API: the camera list,
// camera snapshots, and the events stream (doorbell rings). Auth is an API key made in
// Protect (Settings, Control Plane, Integrations), sent as X-API-KEY. Paths and message shapes
// follow the uiprotect library [D]; docs/decisions.md "Cameras and the doorbell".
package protect

import (
	"context"
	"crypto/tls"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"

	"github.com/coder/websocket"
)

const (
	apiPath   = "/proxy/protect/integration/v1"
	camsTTL   = time.Minute
	snapTTL   = time.Second
	maxSnap   = 8 << 20
	maxBackof = time.Minute
)

// Camera is one Protect camera as the dashboard needs it.
type Camera struct {
	ID     string `json:"id"`
	Name   string `json:"name"`
	Online bool   `json:"online"`
}

// Event is one event from the stream. Type is Protect's ("ring", "motion",
// "smartDetectZone", ...); Device is the camera id.
type Event struct {
	ID     string
	Type   string
	Device string
	Start  time.Time
}

// Client talks to one Protect console. Safe for concurrent use.
type Client struct {
	base string
	key  string
	http *http.Client

	mu     sync.Mutex
	cams   []Camera
	camsAt time.Time
	snaps  map[string]*snap
}

type snap struct {
	mu  sync.Mutex // one fetch per camera at a time; others wait and share it
	jpg []byte
	at  time.Time
}

// New makes a client for the console at base (https://192.168.1.1). insecure skips TLS
// verification: consoles ship a self-signed certificate.
func New(base, key string, insecure bool) *Client {
	tr := http.DefaultTransport.(*http.Transport).Clone()
	if insecure {
		tr.TLSClientConfig = &tls.Config{InsecureSkipVerify: true} //nolint:gosec // opt-in, self-signed console certificate
	}
	return &Client{base: strings.TrimRight(base, "/"), key: key, http: &http.Client{Transport: tr}, snaps: map[string]*snap{}}
}

func (c *Client) get(ctx context.Context, path string, limit int64) ([]byte, error) {
	ctx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, "GET", c.base+apiPath+path, nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("X-API-KEY", c.key)
	resp, err := c.http.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("protect %s: %s", path, resp.Status)
	}
	b, err := io.ReadAll(io.LimitReader(resp.Body, limit+1))
	if err == nil && int64(len(b)) > limit {
		err = fmt.Errorf("protect %s: response over %d bytes", path, limit)
	}
	return b, err
}

// Cameras lists the cameras, cached for a minute. A failed refresh returns the old list too.
func (c *Client) Cameras(ctx context.Context) ([]Camera, error) {
	c.mu.Lock()
	if c.cams != nil && time.Since(c.camsAt) < camsTTL {
		defer c.mu.Unlock()
		return c.cams, nil
	}
	c.mu.Unlock()
	b, err := c.get(ctx, "/cameras", 4<<20)
	var cams []Camera
	if err == nil {
		cams, err = ParseCameras(b)
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if err != nil {
		return c.cams, err
	}
	c.cams, c.camsAt = cams, time.Now()
	return cams, nil
}

// ParseCameras reads GET /v1/cameras.
func ParseCameras(b []byte) ([]Camera, error) {
	var raw []struct {
		ID    string `json:"id"`
		Name  string `json:"name"`
		State string `json:"state"`
	}
	if err := json.Unmarshal(b, &raw); err != nil {
		return nil, fmt.Errorf("protect cameras: %w", err)
	}
	out := make([]Camera, 0, len(raw))
	for _, r := range raw {
		if r.ID == "" {
			continue
		}
		name := r.Name
		if name == "" {
			name = r.ID
		}
		out = append(out, Camera{ID: r.ID, Name: name, Online: r.State == "" || r.State == "CONNECTED"})
	}
	return out, nil
}

// Snapshot is a camera's current JPEG. Pages on several frames ask at once; within a second
// they share one fetch.
func (c *Client) Snapshot(ctx context.Context, id string) ([]byte, error) {
	c.mu.Lock()
	s := c.snaps[id]
	if s == nil {
		s = &snap{}
		c.snaps[id] = s
	}
	c.mu.Unlock()
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.jpg != nil && time.Since(s.at) < snapTTL {
		return s.jpg, nil
	}
	b, err := c.get(ctx, "/cameras/"+url.PathEscape(id)+"/snapshot?highQuality=false", maxSnap)
	if err != nil {
		return nil, err
	}
	s.jpg, s.at = b, time.Now()
	return b, nil
}

// ParseEvent reads one events-stream message. Only new events count ("add" of an "event"):
// updates carry the end of an event already seen.
func ParseEvent(b []byte) (Event, bool) {
	var m struct {
		Type string `json:"type"`
		Item struct {
			ModelKey string `json:"modelKey"`
			ID       string `json:"id"`
			Type     string `json:"type"`
			Device   string `json:"device"`
			Start    int64  `json:"start"`
		} `json:"item"`
	}
	if json.Unmarshal(b, &m) != nil || m.Type != "add" || m.Item.ModelKey != "event" || m.Item.Type == "" {
		return Event{}, false
	}
	ev := Event{ID: m.Item.ID, Type: m.Item.Type, Device: m.Item.Device, Start: time.Now()}
	if m.Item.Start > 0 {
		ev.Start = time.UnixMilli(m.Item.Start)
	}
	return ev, true
}

// Watch follows the events stream until ctx ends, reconnecting with backoff, and calls fn
// for each new event.
func (c *Client) Watch(ctx context.Context, fn func(Event)) {
	wait := 2 * time.Second
	for ctx.Err() == nil {
		start := time.Now()
		err := c.watchOnce(ctx, fn)
		if ctx.Err() != nil {
			return
		}
		if time.Since(start) > 5*time.Minute {
			wait = 2 * time.Second // it was up for a while: a fresh failure
		}
		log.Printf("protect: events stream: %v (again in %s)", err, wait)
		select {
		case <-ctx.Done():
			return
		case <-time.After(wait):
		}
		wait = min(wait*2, maxBackof)
	}
}

func (c *Client) watchOnce(ctx context.Context, fn func(Event)) error {
	u := "ws" + strings.TrimPrefix(c.base, "http") + apiPath + "/subscribe/events"
	dctx, cancel := context.WithTimeout(ctx, 15*time.Second)
	defer cancel()
	ws, _, err := websocket.Dial(dctx, u, &websocket.DialOptions{HTTPClient: c.http,
		HTTPHeader: http.Header{"X-API-KEY": []string{c.key}}})
	if err != nil {
		return err
	}
	defer ws.CloseNow()
	ws.SetReadLimit(1 << 20)
	log.Printf("protect: events stream connected")
	// Protect pings; a dead link with no ping for 10 minutes is dropped and redialled.
	for {
		rctx, rcancel := context.WithTimeout(ctx, 10*time.Minute)
		_, b, err := ws.Read(rctx)
		rcancel()
		if err != nil {
			return err
		}
		if ev, ok := ParseEvent(b); ok {
			fn(ev)
		}
	}
}
