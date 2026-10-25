package api

import (
	"context"
	"encoding/json"
	"fmt"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/MikeO7/thinframe/server/internal/store"
)

func srcLayout() store.Layout {
	return store.Layout{Cols: 2, Rows: 1, Widgets: []store.Widget{
		{ID: "a", Type: "list", W: 1, H: 1, Props: map[string]any{"source": "house"}},
		{ID: "b", Type: "tiles", X: 1, W: 1, H: 1, Props: map[string]any{"source": "other"}},
	}}
}

func recv(t *testing.T, ch chan []byte) map[string]any {
	t.Helper()
	select {
	case b := <-ch:
		var m map[string]any
		if err := json.Unmarshal(b, &m); err != nil {
			t.Fatal(err)
		}
		return m
	case <-time.After(2 * time.Second):
		t.Fatal("no message")
	}
	return nil
}

func TestSourcePublishPersistReplay(t *testing.T) {
	s, ts := newTest(t, "tok")
	if _, err := s.SetLayout("office", srcLayout()); err != nil {
		t.Fatal(err)
	}
	ch, unsub := s.sse.subscribe("office")
	defer unsub()
	if r, _ := do(t, "PUT", ts.URL+"/api/sources/house", "", map[string]any{"items": []any{}}); r.StatusCode != 401 {
		t.Fatalf("no token: %d", r.StatusCode)
	}
	if r, b := do(t, "PUT", ts.URL+"/api/sources/house", "tok", map[string]any{"items": []any{map[string]any{"label": "Door"}}}); r.StatusCode != 204 {
		t.Fatalf("put: %d %s", r.StatusCode, b)
	}
	m := recv(t, ch)
	if m["widget"] != "a" || !strings.Contains(string(mustJSON(m["data"])), "Door") {
		t.Fatalf("published %v", m)
	}
	select {
	case b := <-ch:
		t.Fatalf("widget b does not read house: %s", b)
	default:
	}
	// The same object again is not re-sent.
	if err := s.SetSource("house", []byte(`{"items":[{"label":"Door"}]}`)); err != nil {
		t.Fatal(err)
	}
	select {
	case b := <-ch:
		t.Fatalf("unchanged source re-sent: %s", b)
	case <-time.After(100 * time.Millisecond):
	}
	// A new server on the same data dir has it, and replays it on connect.
	s2 := New(s.Store, Options{})
	if d, ok := s2.Source("house"); !ok || !strings.Contains(string(d), "Door") {
		t.Fatalf("not persisted: %s", d)
	}
	f, _ := s2.Store.Get("office")
	ch2, unsub2 := s2.sse.subscribe("office")
	defer unsub2()
	s2.pushSources(f)
	if m := recv(t, ch2); m["widget"] != "a" {
		t.Fatalf("replay %v", m)
	}
	if r, b := do(t, "GET", ts.URL+"/api/sources", "tok", nil); r.StatusCode != 200 || !strings.Contains(string(b), `"key":"house"`) {
		t.Fatalf("list: %d %s", r.StatusCode, b)
	}
}

func mustJSON(v any) []byte { b, _ := json.Marshal(v); return b }

func TestSourceRejects(t *testing.T) {
	s, _ := newTest(t, "")
	for key, body := range map[string]string{"house": `[1,2]`, "Bad Key": `{}`, "nul": `null`, "big": `{"x":"` + strings.Repeat("a", MaxSource) + `"}`, "trunc": `{"x":`} {
		if err := s.SetSource(key, []byte(body)); err == nil {
			t.Errorf("%s accepted", key)
		}
	}
}

func TestAssetServing(t *testing.T) {
	s, ts := newTest(t, "tok")
	dir := filepath.Join(filepath.Dir(s.Store.Dir()), "assets")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "car.png"), []byte("\x89PNG\r\n\x1a\nxx"), 0o644); err != nil {
		t.Fatal(err)
	}
	r, _ := do(t, "GET", ts.URL+"/api/data/asset/car.png", "", nil)
	if r.StatusCode != 200 || r.Header.Get("Content-Type") != "image/png" || r.Header.Get("X-Content-Type-Options") != "nosniff" {
		t.Fatalf("asset: %d %v", r.StatusCode, r.Header)
	}
	for _, p := range []string{"nope.png", "car.svg", "..%2Fframes%2Foffice.json", ".hidden.png"} {
		if r, _ := do(t, "GET", ts.URL+"/api/data/asset/"+p, "", nil); r.StatusCode == 200 {
			t.Errorf("%s served", p)
		}
	}
}

func TestParseDockerPS(t *testing.T) {
	out := "web\trunning\tUp 2 days (healthy)\ndb\trunning\tUp 1 hour (unhealthy)\n" +
		"job\texited\tExited (1) 3 hours ago\nnew\trunning\tUp 5 seconds (health: starting)\nplain\trunning\tUp 3 days\nbroken line\n"
	got := ParseDockerPS(out)
	want := []Container{{"web", "healthy", ""}, {"db", "unhealthy", ""}, {"job", "exited", "Exited (1) 3 hours ago"}, {"new", "starting", ""}, {"plain", "running", ""}}
	if len(got) != len(want) {
		t.Fatalf("got %v", got)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Errorf("%d: got %v want %v", i, got[i], want[i])
		}
	}
}

func TestParseDockerAPI(t *testing.T) {
	body := `[{"Names":["/web"],"State":"running","Status":"Up 2 days (healthy)"},{"Names":["/job"],"State":"exited","Status":"Exited (1) 3 hours ago"},
		{"Names":[],"State":"running"},{"Names":["/a-db"],"State":"running","Status":"Up 1 hour (unhealthy)"}]`
	got, err := ParseDockerAPI(strings.NewReader(body))
	want := []Container{{"a-db", "unhealthy", ""}, {"job", "exited", "Exited (1) 3 hours ago"}, {"web", "healthy", ""}}
	if err != nil || len(got) != len(want) {
		t.Fatalf("got %v %v", got, err)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Errorf("%d: got %v want %v", i, got[i], want[i])
		}
	}
}

// PollDocker talks to a unix socket and only asks for the container list.
func TestPollDockerSocket(t *testing.T) {
	s, _ := newTest(t, "")
	sock := filepath.Join(t.TempDir(), "d.sock")
	ln, err := net.Listen("unix", sock)
	if err != nil {
		t.Skip("no unix sockets:", err)
	}
	var mu sync.Mutex
	var paths []string
	srv := &http.Server{Handler: http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		mu.Lock()
		paths = append(paths, r.Method+" "+r.URL.String())
		mu.Unlock()
		_, _ = w.Write([]byte(`[{"Names":["/plex"],"State":"running","Status":"Up (healthy)"}]`))
	})}
	go func() { _ = srv.Serve(ln) }()
	defer srv.Close()
	ctx, cancel := context.WithCancel(context.Background())
	go s.PollDocker(ctx, sock, time.Hour)
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		if d, ok := s.Source("services"); ok {
			cancel()
			mu.Lock()
			defer mu.Unlock()
			if !strings.Contains(string(d), `"name":"plex","state":"healthy"`) || len(paths) != 1 || paths[0] != "GET /containers/json?all=1" {
				t.Fatalf("source %s, requests %v", d, paths)
			}
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	cancel()
	t.Fatal("no services source")
}

// A layout reading many sources gets all of them on connect, none dropped.
func TestReplayDoesNotDrop(t *testing.T) {
	s, _ := newTest(t, "")
	l := store.Layout{Cols: 12, Rows: 12}
	for i := 0; i < MaxSources; i++ {
		k := fmt.Sprintf("s%d", i)
		l.Widgets = append(l.Widgets, store.Widget{ID: k, Type: "tiles", X: i % 12, Y: i / 12, W: 1, H: 1, Props: map[string]any{"source": k}})
		if err := s.SetSource(k, []byte(`{"items":[]}`)); err != nil {
			t.Fatal(err)
		}
	}
	if _, err := s.SetLayout("busy", l); err != nil {
		t.Fatal(err)
	}
	ch, unsub := s.sse.subscribe("busy")
	defer unsub()
	f, _ := s.Store.Get("busy")
	s.pushSources(f)
	if len(ch) != MaxSources {
		t.Fatalf("queued %d of %d", len(ch), MaxSources)
	}
}
