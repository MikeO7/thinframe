package api

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json"
	"io"
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
	_, simPage := do(t, "GET", ts.URL+"/frame/hall?sim=1280x800&fit=800x600", "", nil)
	if !strings.Contains(string(simPage), "width:1280px;height:800px") || !strings.Contains(string(simPage), "scale(0.62500)") {
		t.Fatalf("sim size/scale not applied")
	}
	_, simPage = do(t, "GET", ts.URL+"/frame/hall?sim=800x1280&fit=800x600", "", nil)
	if !strings.Contains(string(simPage), "scale(0.46875)") {
		t.Fatalf("portrait scale wrong")
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

func TestSplashConfig(t *testing.T) {
	_, ts := newTest(t, "")
	png := "iVBORw0KGgoAAAANSUhEUgAAAAQAAAADCAYAAAC09K7GAAAAPElEQVR42mN0UWiy2Xksvy3Pi8MnZi3HNSaX0E1pQqoyThF29xlWMf+ZzKLBaOPZyrBQf45manllY9xzAM7wEmsd43BoAAAAAElFTkSuQmCC"
	url := ts.URL + "/api/frames/kitchen/config"
	resp, body := do(t, "PUT", url, "", map[string]any{"splash_text": "Kitchen", "splash_bg": "#203040", "splash_png": png})
	if resp.StatusCode != 200 {
		t.Fatalf("%d %s", resp.StatusCode, body)
	}
	var cfg map[string]any
	_ = json.Unmarshal(body, &cfg)
	if cfg["splash_text"] != "Kitchen" || cfg["splash_bg"] != "#203040" || cfg["splash_png"] != png {
		t.Fatalf("splash fields not in the frame's config: %v", cfg)
	}
	big := base64.StdEncoding.EncodeToString(append([]byte("\x89PNG\r\n\x1a\n"), make([]byte, MaxSplashPNG)...))
	for _, bad := range []map[string]any{
		{"splash_bg": "203040"},
		{"splash_bg": "#20304"},
		{"splash_png": "not base64!"},
		{"splash_png": base64.StdEncoding.EncodeToString([]byte("GIF89a"))},
		{"splash_png": big},
	} {
		resp, body := do(t, "PUT", url, "", bad)
		if resp.StatusCode != 400 || !strings.Contains(string(body), "splash") {
			t.Fatalf("accepted %v: %d %s", bad, resp.StatusCode, body)
		}
	}
	resp, body = do(t, "PUT", url, "", map[string]any{"splash_png": "", "splash_bg": ""})
	_ = json.Unmarshal(body, &cfg)
	if resp.StatusCode != 200 || cfg["splash_png"] != "" || cfg["splash_text"] != "Kitchen" {
		t.Fatalf("clearing the logo: %d %v", resp.StatusCode, cfg)
	}
}

func TestTouchSurvivesHeartbeat(t *testing.T) {
	s, ts := newTest(t, "secret")
	if _, err := s.recordHeartbeat("f1", store.Heartbeat{IP: "10.0.0.5"}); err != nil {
		t.Fatal(err)
	}
	for _, ms := range []int{40, 20, 30} {
		do(t, "POST", ts.URL+"/api/frames/f1/beacon", "", map[string]any{"latency_ms": ms, "painted_at_ms": 1700000000000 + ms})
	}
	do(t, "POST", ts.URL+"/api/frames/ghost/beacon", "", map[string]any{"latency_ms": 9})
	if _, err := s.recordHeartbeat("f1", store.Heartbeat{IP: "10.0.0.6"}); err != nil {
		t.Fatal(err)
	}
	f, _ := s.Store.Get("f1")
	if f.Touch == nil || f.Touch.LastMS != 30 || f.Touch.PaintedAtMS != 1700000000030 || f.Touch.P50() != 30 || f.Touch.Max() != 40 {
		t.Fatalf("touch after heartbeat: %+v", f.Touch)
	}
	if _, err := s.Store.Get("ghost"); err == nil {
		t.Fatal("a beacon created an unknown frame")
	}
	req, _ := http.NewRequest("GET", ts.URL+"/admin", nil)
	req.SetBasicAuth("admin", "secret")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	page, _ := io.ReadAll(resp.Body)
	if !bytes.Contains(page, []byte("30 ms")) || !bytes.Contains(page, []byte("max 40")) {
		t.Fatalf("admin does not show the touch latency")
	}
}

// Deleting a frame must drop its live socket: it was authenticated against the record just removed.
func TestDeleteFrameKicksSocket(t *testing.T) {
	s, ts := newTest(t, "tok")
	url := "ws" + strings.TrimPrefix(ts.URL, "http") + "/api/frames/sim/ws"
	c, _, err := websocket.Dial(context.Background(), url, &websocket.DialOptions{HTTPHeader: http.Header{"Authorization": {"Bearer tok"}}})
	if err != nil {
		t.Fatal(err)
	}
	defer func() { _ = c.CloseNow() }()
	readMsg(t, c) // config on connect
	if !s.Hub.Online("sim") {
		t.Fatal("sim should be online")
	}
	if resp, _ := do(t, "DELETE", ts.URL+"/api/frames/sim", "tok", nil); resp.StatusCode != 204 {
		t.Fatalf("delete: %d", resp.StatusCode)
	}
	deadline := time.Now().Add(3 * time.Second)
	for s.Hub.Online("sim") {
		if time.Now().After(deadline) {
			t.Fatal("deleted frame still online")
		}
		time.Sleep(10 * time.Millisecond)
	}
}
