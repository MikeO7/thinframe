package api

import (
	"context"
	"log"
	"net/http"
	"slices"
	"strconv"
	"sync"
	"time"

	"github.com/MikeO7/thinframe/server/internal/protect"
	"github.com/MikeO7/thinframe/server/internal/store"
)

// Cameras and the doorbell (UniFi Protect, internal/protect). The page shows snapshots, not
// video: the frame's WebKit cannot play RTSP, and a JPEG every few seconds is cheap on the A33.
// A ring wakes every connected frame's screen and pops the doorbell camera over the page.

const (
	ringShow = 90 * time.Second // a page that (re)loads this soon after a ring still shows it
	ringWake = "120"            // seconds a ring keeps an off screen on
	camWait  = 10 * time.Second // snapshot request timeout
	rangKeep = 24 * time.Hour   // a doorbell that rang may be fetched this long after
)

// Protect is what the server needs from a Protect console (protect.Client; a fake in tests).
type Protect interface {
	Cameras(ctx context.Context) ([]protect.Camera, error)
	Snapshot(ctx context.Context, id string) ([]byte, error)
	Watch(ctx context.Context, fn func(protect.Event))
}

type ringState struct {
	mu   sync.Mutex
	last Ring
	rang map[string]time.Time // camera id -> last ring
}

// Ring is the last doorbell press.
type Ring struct {
	ID     string    `json:"id"`
	Camera string    `json:"camera"`
	Name   string    `json:"name"`
	At     time.Time `json:"at"`
}

// RunProtect follows the Protect events stream until ctx ends.
func (s *Server) RunProtect(ctx context.Context) {
	if s.opts.Protect == nil {
		return
	}
	s.opts.Protect.Watch(ctx, s.protectEvent)
}

func (s *Server) protectEvent(ev protect.Event) {
	if ev.Type != "ring" || ev.Device == "" {
		return
	}
	name := "Doorbell"
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	if cams, _ := s.opts.Protect.Cameras(ctx); cams != nil {
		for _, c := range cams {
			if c.ID == ev.Device {
				name = c.Name
			}
		}
	}
	r := Ring{ID: ev.ID, Camera: ev.Device, Name: name, At: time.Now()}
	if r.ID == "" {
		r.ID = strconv.FormatInt(r.At.UnixMilli(), 36)
	}
	s.ring.mu.Lock()
	s.ring.last = r
	if s.ring.rang == nil {
		s.ring.rang = map[string]time.Time{}
	}
	s.ring.rang[ev.Device] = r.At
	s.ring.mu.Unlock()
	log.Printf("protect: doorbell %q rang", name)
	for _, f := range s.Store.List() {
		s.sse.publish(f.ID, s.ringMsg(r))
		// The page shows the ring; an off screen has a stopped browser, so wake it first. Its
		// fresh page gets the ring from the replay on connect (pushRing).
		if s.Hub.Online(f.ID) {
			if _, err := s.Act(f.ID, "wake", map[string]string{"seconds": ringWake}); err != nil {
				log.Printf("protect: wake %s: %v", f.ID, err)
			}
		}
	}
}

func (s *Server) ringMsg(r Ring) map[string]any {
	return map[string]any{"doorbell": map[string]any{"id": r.ID, "camera": r.Camera, "name": r.Name,
		"age_s": int(time.Since(r.At).Seconds())}}
}

// pushRing replays a recent ring to a page that just connected.
func (s *Server) pushRing(id string) {
	s.ring.mu.Lock()
	r := s.ring.last
	s.ring.mu.Unlock()
	if r.Camera != "" && time.Since(r.At) < ringShow {
		s.sse.publish(id, s.ringMsg(r))
	}
}

// camerasFor is a cameras widget's list: props.cameras names or ids, in that order; none = all.
func camerasFor(w store.Widget, all []protect.Camera) []protect.Camera {
	want := asStrings(w.Props["cameras"])
	if len(want) == 0 {
		return all
	}
	var out []protect.Camera
	for _, k := range want {
		for _, c := range all {
			if c.ID == k || c.Name == k {
				out = append(out, c)
				break
			}
		}
	}
	return out
}

// cameraAllowed: the page plane is public, so only cameras some layout shows, and doorbells
// that rang lately, are served.
func (s *Server) cameraAllowed(ctx context.Context, id string) bool {
	s.ring.mu.Lock()
	at, rang := s.ring.rang[id]
	s.ring.mu.Unlock()
	if rang && time.Since(at) < rangKeep {
		return true
	}
	all, _ := s.opts.Protect.Cameras(ctx)
	for _, f := range s.Store.List() {
		for _, w := range f.Layout.Widgets {
			if w.Type == "cameras" && slices.ContainsFunc(camerasFor(w, all), func(c protect.Camera) bool { return c.ID == id }) {
				return true
			}
		}
	}
	return false
}

// handleCamera: GET /api/data/camera/{id}, the camera's current snapshot (JPEG).
func (s *Server) handleCamera(w http.ResponseWriter, r *http.Request) {
	if s.opts.Protect == nil {
		http.Error(w, "cameras not configured (PROTECT_URL)", http.StatusNotFound)
		return
	}
	ctx, cancel := context.WithTimeout(r.Context(), camWait)
	defer cancel()
	id := r.PathValue("id")
	if !s.cameraAllowed(ctx, id) {
		http.Error(w, "no such camera on any dashboard", http.StatusNotFound)
		return
	}
	b, err := s.opts.Protect.Snapshot(ctx, id)
	if err != nil {
		log.Printf("protect: snapshot %s: %v", id, err)
		http.Error(w, "snapshot failed", http.StatusBadGateway)
		return
	}
	w.Header().Set("Content-Type", "image/jpeg")
	w.Header().Set("Cache-Control", "no-store")
	_, _ = w.Write(b)
}

// camerasData is a cameras widget's data: {cameras: [{id,name,online}]}.
func (s *Server) camerasData(w store.Widget) map[string]any {
	ctx, cancel := context.WithTimeout(context.Background(), camWait)
	defer cancel()
	all, err := s.opts.Protect.Cameras(ctx)
	if err != nil {
		log.Printf("protect: cameras: %v", err)
	}
	if all == nil {
		return map[string]any{"error": "cameras unavailable"}
	}
	cams := camerasFor(w, all)
	if cams == nil {
		cams = []protect.Camera{}
	}
	return map[string]any{"cameras": cams}
}
