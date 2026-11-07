package api

import (
	"bytes"
	"context"
	"image"
	"image/jpeg"
	"log"
	"math"
	"net/http"
	"slices"
	"strconv"
	"sync"
	"time"

	"github.com/MikeO7/thinframe/server/internal/protect"
)

// Camera pictures as the frame gets them, and the doorbell's ring picture.
//
// Protect sends snapshots at the camera's size: 3840x2160 from a 4K camera, 1.5 MB. The A33
// takes most of a second to decode one, so the page asks for the width it shows (?w=) and the
// server shrinks it once for every frame (images.go shrink).
//
// A ring keeps one picture of who rang: snapshots at ringOffsets, the clearest one that is not
// blank (black, grey or washed out), preferring one taken while Protect sees a person or a face
// at that camera. The page shows that picture, not a live view that may show an empty porch
// by the time the screen is on.

// camWidths are the widths served; a request is rounded up to one, so the cache stays small.
var camWidths = []int{320, 480, 640, 960, 1280}

const (
	camSnapTTL = time.Second
	ringW      = 1280 // ring pictures are kept at most this wide
	ringKeep   = 8    // ring pictures kept, in memory
	personKeep = 2 * time.Minute
)

// ringOffsets are when ring snapshots are taken, from the press. From ringEnough on, any clear
// picture ends it; before, only a clear one with a person in view. Tests shorten them.
var (
	ringOffsets = []time.Duration{0, 1200 * time.Millisecond, 2400 * time.Millisecond,
		3600 * time.Millisecond, 5 * time.Second, 6500 * time.Millisecond, 8 * time.Second}
	ringEnough = 3600 * time.Millisecond
)

func camWidth(q string) int {
	n, err := strconv.Atoi(q)
	if err != nil || n <= 0 {
		return camWidths[len(camWidths)-1]
	}
	for _, w := range camWidths {
		if n <= w {
			return w
		}
	}
	return camWidths[len(camWidths)-1]
}

type camShot struct {
	mu  sync.Mutex // one fetch per camera and width at a time; others wait and share it
	jpg []byte
	at  time.Time
}

type camShots struct {
	mu sync.Mutex
	m  map[string]*camShot
}

// cameraJPEG is camera id's current picture at most w wide, shared between requests for a second.
func (s *Server) cameraJPEG(ctx context.Context, id string, w int) ([]byte, error) {
	key := id + "@" + strconv.Itoa(w)
	s.cams.mu.Lock()
	if s.cams.m == nil {
		s.cams.m = map[string]*camShot{}
	}
	c := s.cams.m[key]
	if c == nil {
		c = &camShot{}
		s.cams.m[key] = c
	}
	s.cams.mu.Unlock()
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.jpg != nil && time.Since(c.at) < camSnapTTL {
		return c.jpg, nil
	}
	b, err := s.opts.Protect.Snapshot(ctx, id)
	if err != nil {
		return nil, err
	}
	c.jpg, c.at = shrinkJPEG(b, w), time.Now()
	return c.jpg, nil
}

// shrinkJPEG returns picture b at most w wide. One it cannot read, or already narrow enough,
// is returned as it came.
func shrinkJPEG(b []byte, w int) []byte {
	cfg, _, err := image.DecodeConfig(bytes.NewReader(b))
	if err != nil || cfg.Width <= w {
		return b
	}
	img, err := decodeBounded(b)
	if err != nil {
		return b
	}
	return encodeJPEG(shrink(img, w), b)
}

func encodeJPEG(img image.Image, fallback []byte) []byte {
	var out bytes.Buffer
	if err := jpeg.Encode(&out, img, &jpeg.Options{Quality: 80}); err != nil {
		return fallback
	}
	return out.Bytes()
}

// look measures a picture shrunk to 160 wide: blank when it is near black, near white or one
// flat colour (a camera switching to night mode, a grey "no signal" frame); sharp is the mean
// brightness step between neighbouring pixels, higher for a crisp picture than a blurred one.
func look(img image.Image) (blank bool, sharp float64) {
	small := shrink(img, 160)
	b := small.Bounds()
	w, h := b.Dx(), b.Dy()
	if w < 2 || h < 2 {
		return true, 0
	}
	lum := make([]float64, w*h)
	var sum, sq float64
	for y := range h {
		for x := range w {
			r, g, bl, _ := small.At(b.Min.X+x, b.Min.Y+y).RGBA()
			l := (0.299*float64(r) + 0.587*float64(g) + 0.114*float64(bl)) / 257
			lum[y*w+x] = l
			sum += l
			sq += l * l
		}
	}
	n := float64(w * h)
	mean := sum / n
	sd := math.Sqrt(max(0, sq/n-mean*mean))
	var d float64
	for y := range h - 1 {
		for x := range w - 1 {
			l := lum[y*w+x]
			d += math.Abs(lum[y*w+x+1]-l) + math.Abs(lum[(y+1)*w+x]-l)
		}
	}
	sharp = d / float64((w-1)*(h-1))
	return mean < 14 || mean > 240 || sd < 10, sharp
}

// ringShot is one ring and its picture. done closes when the picture is chosen; jpg stays nil
// when no snapshot was clear.
type ringShot struct {
	Ring
	done chan struct{}
	jpg  []byte
}

// personSeen: a person or face detection that has not ended, by event id.
type personSeen struct {
	cam string
	at  time.Time
}

func isPerson(types []string) bool {
	return slices.Contains(types, "person") || slices.Contains(types, "face")
}

// personNow reports whether Protect currently sees a person or a face at camera cam.
func (s *Server) personNow(cam string) bool {
	s.ring.mu.Lock()
	defer s.ring.mu.Unlock()
	for _, p := range s.ring.persons {
		if p.cam == cam && time.Since(p.at) < personKeep {
			return true
		}
	}
	return false
}

// smartEvent follows Protect's person and face detections, for picking the ring picture.
func (s *Server) smartEvent(ev protect.Event) {
	s.ring.mu.Lock()
	defer s.ring.mu.Unlock()
	if s.ring.persons == nil {
		s.ring.persons = map[string]personSeen{}
	}
	for id, p := range s.ring.persons {
		if time.Since(p.at) > personKeep {
			delete(s.ring.persons, id)
		}
	}
	if ev.Ended {
		delete(s.ring.persons, ev.ID)
	} else if ev.ID != "" && ev.Device != "" && isPerson(ev.Smart) {
		s.ring.persons[ev.ID] = personSeen{cam: ev.Device, at: time.Now()}
	}
}

// startRingShot records ring r and starts choosing its picture.
func (s *Server) startRingShot(r Ring) {
	shot := &ringShot{Ring: r, done: make(chan struct{})}
	s.ring.mu.Lock()
	if s.ring.shots == nil {
		s.ring.shots = map[string]*ringShot{}
	}
	if _, dup := s.ring.shots[r.ID]; dup {
		s.ring.mu.Unlock()
		return
	}
	s.ring.shots[r.ID] = shot
	s.ring.order = append(s.ring.order, r.ID)
	for len(s.ring.order) > ringKeep {
		delete(s.ring.shots, s.ring.order[0])
		s.ring.order = s.ring.order[1:]
	}
	s.ring.mu.Unlock()
	go func() {
		jpg := s.pickRingPicture(r.Camera)
		s.ring.mu.Lock()
		shot.jpg = jpg
		s.ring.mu.Unlock()
		close(shot.done)
		if jpg == nil {
			log.Printf("protect: doorbell %q: no clear picture in %s", r.Name, ringOffsets[len(ringOffsets)-1])
		}
		s.pushCameras() // the cameras widgets show the last ring
	}()
}

// pickRingPicture takes snapshots of cam at ringOffsets and returns the best, ringW wide.
func (s *Server) pickRingPicture(cam string) []byte {
	start := time.Now()
	var best []byte
	bestScore := -1.0
	for _, off := range ringOffsets {
		time.Sleep(time.Until(start.Add(off)))
		ctx, cancel := context.WithTimeout(context.Background(), 4*time.Second)
		b, err := s.opts.Protect.Snapshot(ctx, cam)
		cancel()
		if err != nil {
			log.Printf("protect: ring snapshot: %v", err)
			continue
		}
		img, err := decodeBounded(b)
		if err != nil {
			continue
		}
		blank, sharp := look(img)
		person := s.personNow(cam)
		if !blank {
			score := sharp
			if person {
				score *= 2
			}
			if score > bestScore {
				best, bestScore = encodeJPEG(shrink(img, ringW), nil), score
			}
		}
		if best != nil && (off >= ringEnough || person && off > 0) {
			break
		}
	}
	return best
}

// ringHistory is the rings of the last rangKeep at cameras cams, newest first.
func (s *Server) ringHistory(cams []protect.Camera) []map[string]any {
	s.ring.mu.Lock()
	defer s.ring.mu.Unlock()
	out := []map[string]any{}
	for i := len(s.ring.order) - 1; i >= 0; i-- {
		r := s.ring.shots[s.ring.order[i]]
		if r == nil || time.Since(r.At) > rangKeep || !slices.ContainsFunc(cams, func(c protect.Camera) bool { return c.ID == r.Camera }) {
			continue
		}
		ready := false
		select {
		case <-r.done:
			ready = r.jpg != nil
		default:
		}
		out = append(out, map[string]any{"id": r.ID, "camera": r.Camera, "name": r.Name, "at": r.At.UnixMilli(), "still": ready})
	}
	return out
}

// pushCameras sends every cameras widget fresh data (after a ring picture is chosen).
func (s *Server) pushCameras() {
	for _, f := range s.Store.List() {
		for _, w := range f.Layout.Widgets {
			if w.Type == "cameras" {
				s.sse.publish(f.ID, map[string]any{"widget": w.ID, "data": s.camerasData(w)})
			}
		}
	}
}

// handleRing: GET /api/data/ring/{id}, the picture of who rang (JPEG, ?w= like a camera). It
// waits while the picture is being chosen; 404 when none was clear.
func (s *Server) handleRing(w http.ResponseWriter, r *http.Request) {
	s.ring.mu.Lock()
	shot := s.ring.shots[r.PathValue("id")]
	s.ring.mu.Unlock()
	if shot == nil {
		http.Error(w, "no such ring", http.StatusNotFound)
		return
	}
	select {
	case <-shot.done:
	case <-r.Context().Done():
		return
	case <-time.After(camWait + ringOffsets[len(ringOffsets)-1]):
		http.Error(w, "ring picture not ready", http.StatusServiceUnavailable)
		return
	}
	s.ring.mu.Lock()
	b := shot.jpg
	s.ring.mu.Unlock()
	if b == nil {
		http.Error(w, "no clear picture of this ring", http.StatusNotFound)
		return
	}
	if q := r.URL.Query().Get("w"); q != "" {
		b = shrinkJPEG(b, camWidth(q))
	}
	w.Header().Set("Content-Type", "image/jpeg")
	w.Header().Set("Cache-Control", "private, max-age=86400") // a ring picture never changes
	_, _ = w.Write(b)
}
