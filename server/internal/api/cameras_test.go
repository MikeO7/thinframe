package api

import (
	"bufio"
	"context"
	"encoding/json"
	"net/http"
	"strings"
	"testing"
	"time"

	"github.com/MikeO7/thinframe/server/internal/protect"
	"github.com/MikeO7/thinframe/server/internal/store"
)

type fakeProtect struct{ cams []protect.Camera }

func (f fakeProtect) Cameras(context.Context) ([]protect.Camera, error) { return f.cams, nil }
func (f fakeProtect) Snapshot(_ context.Context, id string) ([]byte, error) {
	return []byte("jpeg-" + id), nil
}
func (f fakeProtect) Watch(ctx context.Context, _ func(protect.Event)) { <-ctx.Done() }

var testCams = []protect.Camera{{ID: "c1", Name: "Front Door", Online: true}, {ID: "c2", Name: "Driveway", Online: true}, {ID: "c3", Name: "Nursery", Online: true}}

func camServer(t *testing.T) (*Server, string) {
	s, ts := newDataServer(t, Options{Protect: fakeProtect{cams: testCams}})
	if _, err := s.Store.Ensure("hall"); err != nil {
		t.Fatal(err)
	}
	if _, err := s.SetLayout("hall", store.Layout{Cols: 4, Rows: 3, Widgets: []store.Widget{
		{ID: "cams", Type: "cameras", X: 0, Y: 0, W: 2, H: 2, Props: map[string]any{"cameras": []any{"Driveway", "c1"}}},
	}}); err != nil {
		t.Fatal(err)
	}
	return s, ts.URL
}

func TestCameraSnapshotsOnlyForShownCameras(t *testing.T) {
	s, url := camServer(t)
	for id, want := range map[string]int{"c1": 200, "c2": 200, "c3": 404, "nope": 404} {
		resp, body := do(t, "GET", url+"/api/data/camera/"+id, "", nil)
		if resp.StatusCode != want {
			t.Errorf("camera %s: %d, want %d", id, resp.StatusCode, want)
		}
		if want == 200 && (string(body) != "jpeg-"+id || resp.Header.Get("Content-Type") != "image/jpeg") {
			t.Errorf("camera %s: %q %s", id, body, resp.Header.Get("Content-Type"))
		}
	}
	// A doorbell that rang may be shown in the pop-up although no widget lists it.
	s.protectEvent(protect.Event{ID: "r1", Type: "ring", Device: "c3"})
	if resp, _ := do(t, "GET", url+"/api/data/camera/c3", "", nil); resp.StatusCode != 200 {
		t.Errorf("rung doorbell: %d", resp.StatusCode)
	}
	_, off := newDataServer(t, Options{})
	if resp, _ := do(t, "GET", off.URL+"/api/data/camera/c1", "", nil); resp.StatusCode != 404 {
		t.Errorf("unconfigured: %d", resp.StatusCode)
	}
}

func TestCamerasWidgetData(t *testing.T) {
	s, _ := camServer(t)
	f, _ := s.Store.Get("hall")
	d := s.camerasData(f.Layout.Widgets[0])
	cams, _ := d["cameras"].([]protect.Camera)
	if len(cams) != 2 || cams[0].Name != "Driveway" || cams[1].ID != "c1" {
		t.Fatalf("widget cameras %+v", d)
	}
	if got := camerasFor(store.Widget{}, testCams); len(got) != 3 {
		t.Fatalf("no cameras prop = all, got %d", len(got))
	}
}

// A ring reaches an open page at once and a page that connects shortly after (the screen woke).
func TestRingPushedAndReplayed(t *testing.T) {
	s, url := camServer(t)
	read := func() map[string]any {
		ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
		defer cancel()
		req, _ := http.NewRequestWithContext(ctx, "GET", url+"/api/stream/hall", nil)
		resp, err := http.DefaultClient.Do(req)
		if err != nil {
			t.Fatal(err)
		}
		defer resp.Body.Close()
		sc := bufio.NewScanner(resp.Body)
		for sc.Scan() {
			if line, ok := strings.CutPrefix(sc.Text(), "data: "); ok && strings.Contains(line, `"doorbell"`) {
				var m map[string]map[string]any
				_ = json.Unmarshal([]byte(line), &m)
				return m["doorbell"]
			}
		}
		return nil
	}
	s.protectEvent(protect.Event{ID: "r1", Type: "motion", Device: "c1"}) // not a ring: ignored
	s.protectEvent(protect.Event{ID: "r2", Type: "ring", Device: "c1"})
	d := read()
	if d == nil || d["id"] != "r2" || d["camera"] != "c1" || d["name"] != "Front Door" {
		t.Fatalf("replayed ring %v", d)
	}
	f, _ := s.Store.Get("hall")
	for _, a := range f.Pending {
		if a.Name == "wake" {
			t.Fatal("an offline frame was sent a wake")
		}
	}
	s.ring.mu.Lock()
	s.ring.last.At = time.Now().Add(-2 * ringShow)
	s.ring.mu.Unlock()
	ctx, cancel := context.WithTimeout(context.Background(), 300*time.Millisecond)
	defer cancel()
	req, _ := http.NewRequestWithContext(ctx, "GET", url+"/api/stream/hall", nil)
	if resp, err := http.DefaultClient.Do(req); err == nil {
		sc := bufio.NewScanner(resp.Body)
		for sc.Scan() {
			if strings.Contains(sc.Text(), `"doorbell"`) {
				t.Fatal("an old ring was replayed")
			}
		}
		resp.Body.Close()
	}
}
