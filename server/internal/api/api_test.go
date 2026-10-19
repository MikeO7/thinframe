package api

import (
	"bytes"
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"

	"github.com/coder/websocket"

	"github.com/MikeO7/thinframe/server/internal/store"
)

func newTest(t *testing.T, token string) (*Server, *httptest.Server) {
	st, err := store.Open(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	s := New(st, Options{Token: token, Version: "test"})
	ts := httptest.NewServer(s.Routes())
	t.Cleanup(ts.Close)
	return s, ts
}

func do(t *testing.T, method, url, token string, body any) (*http.Response, []byte) {
	var rd *bytes.Reader
	if body != nil {
		b, _ := json.Marshal(body)
		rd = bytes.NewReader(b)
	} else {
		rd = bytes.NewReader(nil)
	}
	req, _ := http.NewRequest(method, url, rd)
	if token != "" {
		req.Header.Set("Authorization", "Bearer "+token)
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	var buf bytes.Buffer
	_, _ = buf.ReadFrom(resp.Body)
	resp.Body.Close()
	return resp, buf.Bytes()
}

func TestAuth(t *testing.T) {
	_, ts := newTest(t, "secret")
	resp, _ := do(t, "GET", ts.URL+"/api/frames", "", nil)
	if resp.StatusCode != 401 {
		t.Fatalf("want 401, got %d", resp.StatusCode)
	}
	resp, _ = do(t, "GET", ts.URL+"/api/frames", "secret", nil)
	if resp.StatusCode != 200 {
		t.Fatalf("want 200, got %d", resp.StatusCode)
	}
	resp, _ = do(t, "GET", ts.URL+"/frame/kitchen", "", nil) // page is public
	if resp.StatusCode != 200 {
		t.Fatalf("page want 200, got %d", resp.StatusCode)
	}
}

func TestConfigRoundTrip(t *testing.T) {
	_, ts := newTest(t, "")
	resp, body := do(t, "PUT", ts.URL+"/api/frames/kitchen/config", "", map[string]any{"rotation": 1, "name": "Kitchen"})
	if resp.StatusCode != 200 {
		t.Fatalf("%d %s", resp.StatusCode, body)
	}
	var cfg map[string]any
	_ = json.Unmarshal(body, &cfg)
	if cfg["rotation"] != float64(1) || cfg["url"] != "/frame/kitchen" || cfg["name"] != "Kitchen" {
		t.Fatalf("bad config %v", cfg)
	}
	resp, body = do(t, "PUT", ts.URL+"/api/frames/kitchen/config", "", map[string]any{"rotation": 7})
	if resp.StatusCode != 400 || !strings.Contains(string(body), "rotation") {
		t.Fatalf("bad rotation accepted: %d %s", resp.StatusCode, body)
	}
	resp, body = do(t, "PUT", ts.URL+"/api/frames/kitchen/config", "", map[string]any{"bogus": 1})
	if resp.StatusCode != 400 {
		t.Fatalf("unknown field accepted: %d %s", resp.StatusCode, body)
	}
}

func readMsg(t *testing.T, c *websocket.Conn) map[string]any {
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	_, data, err := c.Read(ctx)
	if err != nil {
		t.Fatalf("read: %v", err)
	}
	var m map[string]any
	_ = json.Unmarshal(data, &m)
	return m
}

// closeAndWait closes a frame socket and waits until the server has finished
// its disconnect bookkeeping, so TempDir cleanup never races a store write.
func closeAndWait(t *testing.T, s *Server, id string, c *websocket.Conn) {
	t.Helper()
	_ = c.Close(websocket.StatusNormalClosure, "test done")
	deadline := time.Now().Add(3 * time.Second)
	for s.Hub.Online(id) {
		if time.Now().After(deadline) {
			t.Fatal("frame never went offline")
		}
		time.Sleep(10 * time.Millisecond)
	}
}

func send(t *testing.T, c *websocket.Conn, v any) {
	b, _ := json.Marshal(v)
	if err := c.Write(context.Background(), websocket.MessageText, b); err != nil {
		t.Fatal(err)
	}
}

func TestWebSocketControlPlane(t *testing.T) {
	s, ts := newTest(t, "tok")
	url := "ws" + strings.TrimPrefix(ts.URL, "http") + "/api/frames/sim/ws"
	c, _, err := websocket.Dial(context.Background(), url, &websocket.DialOptions{HTTPHeader: http.Header{"Authorization": {"Bearer tok"}}})
	if err != nil {
		t.Fatal(err)
	}
	defer closeAndWait(t, s, "sim", c)

	// 1. config arrives on connect
	m := readMsg(t, c)
	if m["type"] != "config" {
		t.Fatalf("first message %v", m)
	}
	// 2. hello gets time
	send(t, c, map[string]any{"type": "hello", "tfctl_version": "0.1", "ip": "10.0.0.5", "mem_used_mb": 120, "mem_total_mb": 990})
	m = readMsg(t, c)
	if m["type"] != "hello" || m["time_unix"] == nil {
		t.Fatalf("hello reply %v", m)
	}
	if !s.Hub.Online("sim") {
		t.Fatal("hub does not see the frame")
	}
	// 3. a config PUT is pushed
	resp, _ := do(t, "PUT", ts.URL+"/api/frames/sim/config", "tok", map[string]any{"rotation": 2})
	if resp.StatusCode != 200 {
		t.Fatal(resp.StatusCode)
	}
	m = readMsg(t, c)
	cfg, _ := m["config"].(map[string]any)
	if m["type"] != "config" || cfg["rotation"] != float64(2) {
		t.Fatalf("pushed config %v", m)
	}
	// 4. an action is pushed, acked, and moves to history
	resp, body := do(t, "POST", ts.URL+"/api/frames/sim/actions", "tok", map[string]any{"name": "screen", "args": map[string]string{"arg": "off"}})
	if resp.StatusCode != 202 {
		t.Fatalf("%d %s", resp.StatusCode, body)
	}
	m = readMsg(t, c)
	if m["type"] != "action" || m["name"] != "screen" || m["state"] != "off" {
		t.Fatalf("action %v", m)
	}
	send(t, c, map[string]any{"type": "ack", "id": m["id"], "ok": true})
	deadline := time.Now().Add(2 * time.Second)
	for {
		f, _ := s.Store.Get("sim")
		if len(f.Pending) == 0 && len(f.History) == 1 && f.History[0].OK != nil && *f.History[0].OK {
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("ack not applied: %+v", f)
		}
		time.Sleep(20 * time.Millisecond)
	}
	// 5. heartbeat updates the view
	send(t, c, map[string]any{"type": "heartbeat", "temp_c": 41.5, "ip": "10.0.0.5"})
	time.Sleep(50 * time.Millisecond)
	_, body = do(t, "GET", ts.URL+"/api/frames/sim", "tok", nil)
	if !strings.Contains(string(body), `"temp_c":41.5`) || !strings.Contains(string(body), `"online":true`) {
		t.Fatalf("view: %s", body)
	}
	// 6. unknown action rejected
	resp, _ = do(t, "POST", ts.URL+"/api/frames/sim/actions", "tok", map[string]any{"name": "explode"})
	if resp.StatusCode != 400 {
		t.Fatal("unknown action accepted")
	}
}

func TestPendingActionResentOnReconnect(t *testing.T) {
	s, ts := newTest(t, "")
	if _, err := s.Act("f1", "reboot", nil); err != nil {
		t.Fatal(err)
	}
	url := "ws" + strings.TrimPrefix(ts.URL, "http") + "/api/frames/f1/ws"
	c, _, err := websocket.Dial(context.Background(), url, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer closeAndWait(t, s, "f1", c)
	_ = readMsg(t, c) // config
	m := readMsg(t, c)
	if m["type"] != "action" || m["name"] != "reboot" {
		t.Fatalf("queued action not resent: %v", m)
	}
}

func TestLayoutAndPage(t *testing.T) {
	_, ts := newTest(t, "")
	lay := store.Layout{Cols: 2, Rows: 1, Widgets: []store.Widget{
		{ID: "t", Type: "text", W: 1, H: 1, Props: map[string]any{"text": "hello frame"}},
		{ID: "c", Type: "clock", X: 1, W: 1, H: 1},
	}}
	resp, body := do(t, "PUT", ts.URL+"/api/frames/hall/layout", "", lay)
	if resp.StatusCode != 200 {
		t.Fatalf("%d %s", resp.StatusCode, body)
	}
	_, page := do(t, "GET", ts.URL+"/frame/hall", "", nil)
	if !strings.Contains(string(page), "hello frame") || !strings.Contains(string(page), `id="w-c"`) {
		t.Fatalf("page: %s", page)
	}
	_, cfg := do(t, "GET", ts.URL+"/api/frames/hall/config", "", nil)
	if !strings.Contains(string(cfg), `"page_version":2`) {
		t.Fatalf("page_version not bumped: %s", cfg)
	}
	lay.Widgets[1].X = 0 // overlap
	resp, body = do(t, "PUT", ts.URL+"/api/frames/hall/layout", "", lay)
	if resp.StatusCode != 400 || !strings.Contains(string(body), "overlaps") {
		t.Fatalf("overlap accepted: %d %s", resp.StatusCode, body)
	}
}

func TestAdminRenders(t *testing.T) {
	s, ts := newTest(t, "")
	_, _ = s.Store.Ensure("kitchen")
	resp, body := do(t, "GET", ts.URL+"/admin", "", nil)
	if resp.StatusCode != 200 || !strings.Contains(string(body), "kitchen") || !strings.Contains(string(body), "save layout") {
		t.Fatalf("%d %s", resp.StatusCode, body[:min(len(body), 300)])
	}
	// form post path
	req, _ := http.NewRequest("POST", ts.URL+"/admin/kitchen/config", strings.NewReader("rotation=1&brightness=50&name=K&screen_off=&screen_on=&url=&tags=a,b"))
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	cl := http.Client{CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}
	r2, err := cl.Do(req)
	if err != nil || r2.StatusCode != 303 || strings.Contains(r2.Header.Get("Location"), "err=") {
		t.Fatalf("admin post: %v %d %s", err, r2.StatusCode, r2.Header.Get("Location"))
	}
	f, _ := s.Store.Get("kitchen")
	if f.Config.Rotation != 1 || f.Config.Brightness != 50 || len(f.Config.Tags) != 2 {
		t.Fatalf("%+v", f.Config)
	}
}
