package api

import (
	"context"
	"encoding/json"
	"net/http"
	"strings"
	"testing"
	"time"

	"github.com/coder/websocket"
)

// actions reads the socket in the background (a Read whose context expires closes the
// connection) and passes on the names of action messages.
func actions(c *websocket.Conn) <-chan string {
	ch := make(chan string, 8)
	go func() {
		defer close(ch)
		for {
			_, data, err := c.Read(context.Background())
			if err != nil {
				return
			}
			var m map[string]any
			_ = json.Unmarshal(data, &m)
			if m["type"] == "action" {
				name, _ := m["name"].(string)
				ch <- name
			}
		}
	}()
	return ch
}

func nextAction(ch <-chan string, d time.Duration) (string, bool) {
	select {
	case name, ok := <-ch:
		return name, ok
	case <-time.After(d):
		return "", false
	}
}

func TestHungPageRestartsCogOnce(t *testing.T) {
	s, ts := newTest(t, "")
	s.pageStale, s.pageCheck = 300*time.Millisecond, 50*time.Millisecond
	alive := func() {
		resp, err := http.Post(ts.URL+"/api/frames/k/alive", "text/plain", nil)
		if err != nil || resp.StatusCode != http.StatusNoContent {
			t.Fatalf("alive: %v %v", err, resp)
		}
		resp.Body.Close()
	}
	// A ping from before the agent connected does not count.
	alive()
	time.Sleep(400 * time.Millisecond)
	c, _, err := websocket.Dial(context.Background(), "ws"+strings.TrimPrefix(ts.URL, "http")+"/api/frames/k/ws", nil)
	if err != nil {
		t.Fatal(err)
	}
	defer closeAndWait(t, s, "k", c)
	acts := actions(c)
	// Connected, no ping yet: nothing to judge.
	if name, ok := nextAction(acts, 200*time.Millisecond); ok {
		t.Fatalf("action %q without a stale page", name)
	}
	for range 4 {
		alive()
		if name, ok := nextAction(acts, 150*time.Millisecond); ok {
			t.Fatalf("action %q while the page pings", name)
		}
	}
	// The page stops pinging: one restart-kiosk, and an event saying why.
	if name, ok := nextAction(acts, 2*time.Second); !ok || name != "restart-kiosk" {
		t.Fatalf("got %q %v, want restart-kiosk", name, ok)
	}
	if name, ok := nextAction(acts, 700*time.Millisecond); ok {
		t.Fatalf("second action %q without a fresh ping", name)
	}
	f, _ := s.Store.Get("k")
	found := false
	for _, e := range f.Events {
		found = found || (e.Kind == "page" && strings.Contains(e.Text, "restarting cog"))
	}
	if !found {
		t.Fatalf("no page event in %v", f.Events)
	}
}

// Any path string used to become a permanent key in the ping map and the SSE broker.
func TestAliveAndStreamRejectBadIDs(t *testing.T) {
	_, ts := newTest(t, "")
	for _, bad := range []string{"Bad%20ID", "x..", "a%2Fb", strings.Repeat("a", 40)} {
		if resp, _ := do(t, "POST", ts.URL+"/api/frames/"+bad+"/alive", "", nil); resp.StatusCode != 400 {
			t.Fatalf("alive %q: %d", bad, resp.StatusCode)
		}
		if resp, _ := do(t, "GET", ts.URL+"/api/stream/"+bad, "", nil); resp.StatusCode != 400 {
			t.Fatalf("stream %q: %d", bad, resp.StatusCode)
		}
	}
}
