package api

import (
	"bytes"
	"context"
	"errors"
	"image"
	"image/color"
	"image/jpeg"
	"math/rand/v2"
	"sync"
	"testing"
	"time"

	"github.com/MikeO7/thinframe/server/internal/protect"
	"github.com/MikeO7/thinframe/server/internal/store"
)

func init() {
	// Ring pictures in tests: three snapshots 20 ms apart, not 8 seconds' worth.
	ringOffsets = []time.Duration{0, 20 * time.Millisecond, 40 * time.Millisecond}
	ringEnough = 40 * time.Millisecond
}

// testJPEG is a w x h picture: "black", "grey" (flat), or "scene" (detail).
func testJPEG(t *testing.T, w, h int, kind string) []byte {
	t.Helper()
	img := image.NewRGBA(image.Rect(0, 0, w, h))
	rnd := rand.New(rand.NewPCG(1, 2))
	for y := range h {
		for x := range w {
			c := color.RGBA{A: 255}
			switch kind {
			case "grey":
				c.R, c.G, c.B = 128, 128, 128
			case "scene": // blocks and a gradient: survives shrinking, like a real picture
				v := uint8(40 + 140*((x/40+y/40)%2) + rnd.IntN(20) + x*20/w)
				c.R, c.G, c.B = v, v/2+60, 255-v
			}
			img.Set(x, y, c)
		}
	}
	var b bytes.Buffer
	if err := jpeg.Encode(&b, img, nil); err != nil {
		t.Fatal(err)
	}
	return b.Bytes()
}

// seqProtect hands out snaps in order (the last one again when they run out).
type seqProtect struct {
	fakeProtect
	mu    sync.Mutex
	snaps [][]byte
	n     int
}

func (f *seqProtect) Snapshot(context.Context, string) ([]byte, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	b := f.snaps[min(f.n, len(f.snaps)-1)]
	f.n++
	return b, nil
}

func camLayout() store.Layout {
	return store.Layout{Cols: 4, Rows: 3, Widgets: []store.Widget{{ID: "cams", Type: "cameras", W: 2, H: 2}}}
}

func jpegWidth(t *testing.T, b []byte) int {
	t.Helper()
	cfg, err := jpeg.DecodeConfig(bytes.NewReader(b))
	if err != nil {
		t.Fatalf("not a JPEG: %v", err)
	}
	return cfg.Width
}

func TestCameraShrunkToAskedWidth(t *testing.T) {
	big := testJPEG(t, 1600, 900, "scene")
	s, ts := newDataServer(t, Options{Protect: &seqProtect{fakeProtect: fakeProtect{cams: testCams}, snaps: [][]byte{big}}})
	if _, err := s.Store.Ensure("hall"); err != nil {
		t.Fatal(err)
	}
	if _, err := s.SetLayout("hall", camLayout()); err != nil {
		t.Fatal(err)
	}
	for q, want := range map[string]int{"?w=400": 480, "?w=320": 320, "": 1280, "?w=9999": 1280, "?w=x": 1280} {
		resp, body := do(t, "GET", ts.URL+"/api/data/camera/c1"+q, "", nil)
		if resp.StatusCode != 200 || jpegWidth(t, body) != want {
			t.Errorf("%q: %d, width %d, want %d", q, resp.StatusCode, jpegWidth(t, body), want)
		}
	}
	small := testJPEG(t, 200, 100, "scene")
	if got := shrinkJPEG(small, 480); !bytes.Equal(got, small) {
		t.Error("a picture narrower than asked was re-encoded")
	}
}

func TestLookFindsBlankPictures(t *testing.T) {
	for kind, want := range map[string]bool{"black": true, "grey": true, "scene": false} {
		img, err := jpeg.Decode(bytes.NewReader(testJPEG(t, 640, 360, kind)))
		if err != nil {
			t.Fatal(err)
		}
		if blank, _ := look(img); blank != want {
			t.Errorf("%s: blank %v, want %v", kind, blank, want)
		}
	}
}

// ringPicture rings c1 with these snapshots and returns the ring picture's status and body.
func ringPicture(t *testing.T, snaps [][]byte, person bool) (int, []byte, *Server) {
	t.Helper()
	fp := &seqProtect{fakeProtect: fakeProtect{cams: testCams}, snaps: snaps}
	s, ts := newDataServer(t, Options{Protect: fp})
	if _, err := s.Store.Ensure("hall"); err != nil {
		t.Fatal(err)
	}
	if person {
		s.protectEvent(protect.Event{ID: "p1", Type: "smartDetectZone", Device: "c1", Smart: []string{"person"}})
	}
	s.protectEvent(protect.Event{ID: "r1", Type: "ring", Device: "c1"})
	resp, body := do(t, "GET", ts.URL+"/api/data/ring/r1", "", nil)
	return resp.StatusCode, body, s
}

func TestRingPictureSkipsBlankSnapshots(t *testing.T) {
	black, grey, scene := testJPEG(t, 640, 480, "black"), testJPEG(t, 640, 480, "grey"), testJPEG(t, 640, 480, "scene")
	code, body, _ := ringPicture(t, [][]byte{black, grey, scene}, false)
	if code != 200 {
		t.Fatalf("ring picture: %d", code)
	}
	img, err := jpeg.Decode(bytes.NewReader(body))
	if err != nil {
		t.Fatal(err)
	}
	if blank, _ := look(img); blank {
		t.Fatal("the ring picture is a blank snapshot")
	}
	if code, _, _ := ringPicture(t, [][]byte{black, grey}, false); code != 404 {
		t.Fatalf("only blank snapshots: %d, want 404 (the page shows the live view)", code)
	}
}

// With Protect seeing a person, the first clear snapshot after the press is enough.
func TestRingPictureStopsEarlyWithPerson(t *testing.T) {
	scene := testJPEG(t, 640, 480, "scene")
	_, _, s := ringPicture(t, [][]byte{scene, scene, scene}, true)
	fp := s.opts.Protect.(*seqProtect)
	fp.mu.Lock()
	n := fp.n
	fp.mu.Unlock()
	if n != 2 {
		t.Fatalf("%d snapshots with a person in view, want 2", n)
	}
	s.protectEvent(protect.Event{ID: "p1", Ended: true})
	if s.personNow("c1") {
		t.Fatal("person still seen after the detection ended")
	}
	d := s.camerasData(camLayout().Widgets[0])
	rings, _ := d["rings"].([]map[string]any)
	if len(rings) != 1 || rings[0]["id"] != "r1" || rings[0]["still"] != true {
		t.Fatalf("rings in widget data %v", d["rings"])
	}
}

// flakyProtect fails every snapshot once fail is set (Protect's 429 to a burst).
type flakyProtect struct {
	fakeProtect
	mu    sync.Mutex
	fail  bool
	calls int
}

func (f *flakyProtect) Snapshot(_ context.Context, id string) ([]byte, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.calls++
	if f.fail {
		return nil, errors.New("429 Too Many Requests")
	}
	return []byte("jpeg-" + id), nil
}

// One Protect fetch serves every width; a failed one serves the last good picture.
func TestCameraKeepsLastPictureOnFailure(t *testing.T) {
	fp := &flakyProtect{fakeProtect: fakeProtect{cams: testCams}}
	s, _ := newDataServer(t, Options{Protect: fp})
	ctx := context.Background()
	for _, w := range []int{320, 640, 1280} {
		if _, err := s.cameraJPEG(ctx, "c1", w); err != nil {
			t.Fatal(err)
		}
	}
	if fp.calls != 1 {
		t.Fatalf("%d Protect fetches for three widths, want 1", fp.calls)
	}
	fp.mu.Lock()
	fp.fail = true
	fp.mu.Unlock()
	s.cams.mu.Lock()
	for _, c := range s.cams.m {
		c.at = time.Now().Add(-2 * camSnapTTL)
	}
	s.cams.mu.Unlock()
	if b, err := s.cameraJPEG(ctx, "c1", 640); err != nil || string(b) != "jpeg-c1" {
		t.Fatalf("after a failure: %q %v, want the last picture", b, err)
	}
	if _, err := s.cameraJPEG(ctx, "c2", 640); err == nil {
		t.Fatal("a camera with no good picture yet answered")
	}
}
