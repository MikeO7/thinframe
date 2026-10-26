package protect

import (
	"context"
	"net/http"
	"net/http/httptest"
	"sync/atomic"
	"testing"
	"time"

	"github.com/coder/websocket"
)

// fakeConsole answers the three Integration API calls the client makes.
func fakeConsole(t *testing.T, snaps *atomic.Int32, events []string) *httptest.Server {
	t.Helper()
	m := http.NewServeMux()
	auth := func(h http.HandlerFunc) http.HandlerFunc {
		return func(w http.ResponseWriter, r *http.Request) {
			if r.Header.Get("X-API-KEY") != "k" {
				http.Error(w, "no key", http.StatusUnauthorized)
				return
			}
			h(w, r)
		}
	}
	m.HandleFunc("GET "+apiPath+"/cameras", auth(func(w http.ResponseWriter, _ *http.Request) {
		_, _ = w.Write([]byte(`[{"id":"c1","name":"Front Door","state":"CONNECTED","modelKey":"camera"},
			{"id":"c2","name":"Garage","state":"DISCONNECTED"},{"id":"","name":"broken"}]`))
	}))
	m.HandleFunc("GET "+apiPath+"/cameras/{id}/snapshot", auth(func(w http.ResponseWriter, r *http.Request) {
		snaps.Add(1)
		if r.URL.Query().Get("highQuality") != "false" {
			t.Errorf("snapshot query %q", r.URL.RawQuery)
		}
		_, _ = w.Write([]byte("jpeg-" + r.PathValue("id")))
	}))
	m.HandleFunc("GET "+apiPath+"/subscribe/events", auth(func(w http.ResponseWriter, r *http.Request) {
		ws, err := websocket.Accept(w, r, nil)
		if err != nil {
			return
		}
		defer ws.CloseNow()
		for _, e := range events {
			_ = ws.Write(r.Context(), websocket.MessageText, []byte(e))
		}
		<-r.Context().Done()
	}))
	ts := httptest.NewTLSServer(m)
	t.Cleanup(ts.Close)
	return ts
}

func TestCamerasAndSnapshots(t *testing.T) {
	var snaps atomic.Int32
	ts := fakeConsole(t, &snaps, nil)
	c := New(ts.URL, "k", true)
	cams, err := c.Cameras(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	want := []Camera{{"c1", "Front Door", true}, {"c2", "Garage", false}}
	if len(cams) != 2 || cams[0] != want[0] || cams[1] != want[1] {
		t.Fatalf("cameras %+v", cams)
	}
	for range 3 {
		b, err := c.Snapshot(context.Background(), "c1")
		if err != nil || string(b) != "jpeg-c1" {
			t.Fatalf("snapshot %q %v", b, err)
		}
	}
	if n := snaps.Load(); n != 1 {
		t.Fatalf("3 snapshots within a second made %d fetches, want 1", n)
	}
	// Without the insecure flag the self-signed certificate is refused.
	if _, err := New(ts.URL, "k", false).Cameras(context.Background()); err == nil {
		t.Fatal("self-signed certificate accepted without PROTECT_INSECURE")
	}
	if _, err := New(ts.URL, "wrong", true).Cameras(context.Background()); err == nil {
		t.Fatal("wrong key accepted")
	}
}

func TestParseEvent(t *testing.T) {
	ev, ok := ParseEvent([]byte(`{"type":"add","item":{"modelKey":"event","id":"e1","type":"ring","start":1761450000000,"device":"c1"}}`))
	if !ok || ev.Type != "ring" || ev.Device != "c1" || ev.ID != "e1" || ev.Start.UnixMilli() != 1761450000000 {
		t.Fatalf("ring %+v %v", ev, ok)
	}
	for _, bad := range []string{
		`{"type":"update","item":{"modelKey":"event","id":"e1","end":1761450001000}}`,
		`{"type":"add","item":{"modelKey":"camera","id":"c1"}}`,
		`not json`, `{}`,
	} {
		if _, ok := ParseEvent([]byte(bad)); ok {
			t.Errorf("%s parsed as an event", bad)
		}
	}
}

func TestWatchDeliversEvents(t *testing.T) {
	var snaps atomic.Int32
	ts := fakeConsole(t, &snaps, []string{
		`{"type":"add","item":{"modelKey":"event","id":"m1","type":"motion","device":"c2"}}`,
		`{"type":"update","item":{"modelKey":"event","id":"m1"}}`,
		`{"type":"add","item":{"modelKey":"event","id":"r1","type":"ring","device":"c1"}}`,
	})
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	got := make(chan Event, 4)
	go New(ts.URL, "k", true).Watch(ctx, func(e Event) { got <- e })
	for _, want := range []string{"motion", "ring"} {
		select {
		case e := <-got:
			if e.Type != want {
				t.Fatalf("event %+v, want %s", e, want)
			}
		case <-ctx.Done():
			t.Fatalf("no %s event", want)
		}
	}
}
