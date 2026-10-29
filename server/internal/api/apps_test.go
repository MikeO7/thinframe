package api

import (
	"bytes"
	"context"
	"encoding/json"
	"image/png"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/coder/websocket"

	"github.com/MikeO7/thinframe/server/internal/store"
)

func get(t *testing.T, url string, hdr map[string]string) (*http.Response, string) {
	t.Helper()
	req, _ := http.NewRequest("GET", url, nil)
	for k, v := range hdr {
		req.Header.Set(k, v)
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	b, _ := io.ReadAll(resp.Body)
	resp.Body.Close()
	return resp, string(b)
}

func post(t *testing.T, url, body string) (*http.Response, string) {
	t.Helper()
	resp, err := http.Post(url, "text/plain", strings.NewReader(body))
	if err != nil {
		t.Fatal(err)
	}
	b, _ := io.ReadAll(resp.Body)
	resp.Body.Close()
	return resp, string(b)
}

func TestInjectSDK(t *testing.T) {
	tag := `<script src="/apps-sdk.js"></script>`
	for in, want := range map[string]string{
		`<!doctype html><html><head><title>x</title></head>`: `<!doctype html><html><head>` + tag + `<title>x</title></head>`,
		`<!DOCTYPE html><HEAD lang="en"><p>`:                 `<!DOCTYPE html><HEAD lang="en">` + tag + `<p>`,
		`<!doctype html><p>hi`:                               `<!doctype html>` + tag + `<p>hi`,
		`<p>hi`:                                              tag + `<p>hi`,
		`<!doctype html><header>x</header>`:                  `<!doctype html>` + tag + `<header>x</header>`,
	} {
		if got := string(injectSDK([]byte(in), "")); got != want {
			t.Errorf("inject %q:\n got %q\nwant %q", in, got, want)
		}
	}
}

func TestAppServing(t *testing.T) {
	s, ts := newTest(t, "tok")
	if _, err := s.PutApp("hello", "demo", "", map[string][]byte{
		"index.html": []byte("<!doctype html><head></head><body>v1"),
		"app.js":     []byte("1"),
		"img/a.svg":  []byte("<svg/>"),
	}); err != nil {
		t.Fatal(err)
	}
	resp, body := get(t, ts.URL+"/apps/hello/index.html", nil)
	if resp.StatusCode != 200 || !strings.Contains(body, `<head><script src="/apps-sdk.js"></script></head>`) {
		t.Fatalf("index: %d %q", resp.StatusCode, body)
	}
	h := resp.Header
	if !strings.HasPrefix(h.Get("Content-Security-Policy"), "sandbox allow-scripts") || strings.Contains(h.Get("Content-Security-Policy"), "allow-same-origin") {
		t.Fatalf("CSP %q", h.Get("Content-Security-Policy"))
	}
	if h.Get("Access-Control-Allow-Origin") != "*" || h.Get("X-Content-Type-Options") != "nosniff" || h.Get("Cache-Control") != "no-cache" {
		t.Fatalf("headers %v", h)
	}
	etag := h.Get("ETag")
	if r, _ := get(t, ts.URL+"/apps/hello/index.html", map[string]string{"If-None-Match": etag}); r.StatusCode != 304 {
		t.Fatalf("revalidate: %d", r.StatusCode)
	}
	if r, b := get(t, ts.URL+"/apps/hello/app.js", nil); r.StatusCode != 200 || b != "1" || !strings.HasPrefix(r.Header.Get("Content-Type"), "text/javascript") {
		t.Fatalf("app.js %d %q %q", r.StatusCode, b, r.Header.Get("Content-Type"))
	}
	if r, _ := get(t, ts.URL+"/apps/hello/img/a.svg", nil); r.Header.Get("Content-Security-Policy") == "" {
		t.Fatal("svg without the sandbox")
	}
	// A new version changes the ETag and is served at once.
	if _, err := s.PutApp("hello", "", "", map[string][]byte{"index.html": []byte("v2")}); err != nil {
		t.Fatal(err)
	}
	r, b := get(t, ts.URL+"/apps/hello/", map[string]string{"If-None-Match": etag})
	if r.StatusCode != 200 || !strings.HasSuffix(b, "v2") {
		t.Fatalf("after put: %d %q", r.StatusCode, b)
	}
	if r, b := get(t, ts.URL+"/apps-sdk.js", nil); r.StatusCode != 200 || !strings.Contains(b, "window.thinframe = tf") {
		t.Fatalf("sdk %d", r.StatusCode)
	}
}

func TestShowAppAndWidgets(t *testing.T) {
	s, _ := newTest(t, "tok")
	_, _ = s.Store.Ensure("office")
	if _, err := s.ShowApp("office", "nope"); err == nil {
		t.Fatal("showed a missing app")
	}
	_, _ = s.PutApp("clock", "", "", map[string][]byte{"index.html": []byte("x")})
	f, err := s.ShowApp("office", "clock")
	if err != nil || s.ConfigFor(f)["url"] != "/apps/clock/index.html?frame=office" {
		t.Fatalf("show: %v %v", err, s.ConfigFor(f)["url"])
	}
	if err := s.DeleteApp("clock"); err == nil || !strings.Contains(err.Error(), "office") {
		t.Fatalf("deleted an app on screen: %v", err)
	}
	if f, _ = s.ShowApp("office", ""); s.ConfigFor(f)["url"] != "/frame/office" {
		t.Fatalf("back to dashboard: %v", s.ConfigFor(f)["url"])
	}
	lay := store.Layout{Cols: 2, Rows: 1, Widgets: []store.Widget{{ID: "a", Type: "app", W: 1, H: 1, Props: map[string]any{"app": "ghost"}}}}
	if _, err := s.SetLayout("office", lay); err == nil {
		t.Fatal("layout with a missing app accepted")
	}
	lay.Widgets[0].Props = map[string]any{"app": "clock", "props": map[string]any{"tz": "UTC"}}
	if _, err := s.SetLayout("office", lay); err != nil {
		t.Fatal(err)
	}
	if u := s.AppUsers("clock"); len(u) != 1 || u[0] != "office" {
		t.Fatalf("users %v", u)
	}
	if err := s.DeleteApp("clock"); err == nil {
		t.Fatal("deleted an app a widget shows")
	}
}

func TestAppMessages(t *testing.T) {
	s, ts := newTest(t, "tok")
	_, _ = s.PutApp("todo", "", "", map[string][]byte{"index.html": []byte("x")})
	plain, unsubP := s.sse.subscribe("office")
	defer unsubP()
	apps, unsubA := s.sse.subscribeAs("hall", true)
	defer unsubA()
	if err := s.SetSource("weather2", []byte(`{"t":1}`)); err != nil {
		t.Fatal(err)
	}
	if m := recv(t, apps); m["source"] != "weather2" {
		t.Fatalf("app subscriber got %v", m)
	}
	if r, _ := post(t, ts.URL+"/api/apps/todo/state/items", `["milk"]`); r.StatusCode != 204 {
		t.Fatalf("state set %d", r.StatusCode)
	}
	m := recv(t, apps)
	st, _ := m["app_state"].(map[string]any)
	if st["app"] != "todo" || st["key"] != "items" {
		t.Fatalf("state message %v", m)
	}
	if r, b := get(t, ts.URL+"/api/apps/todo/state", nil); r.StatusCode != 200 || !strings.Contains(b, "milk") || r.Header.Get("Access-Control-Allow-Origin") != "*" {
		t.Fatalf("state get %d %s", r.StatusCode, b)
	}
	if r, _ := post(t, ts.URL+"/api/apps/todo/state/items", `{bad`); r.StatusCode != 400 {
		t.Fatalf("bad JSON state: %d", r.StatusCode)
	}
	if r, _ := post(t, ts.URL+"/api/apps/none/state/items", `1`); r.StatusCode != 404 {
		t.Fatalf("state of a missing app: %d", r.StatusCode)
	}
	s.SendToApp("hall", "todo", "ping", json.RawMessage(`{"n":1}`))
	if m := recv(t, apps); m["app_event"] == nil {
		t.Fatalf("event %v", m)
	}
	_, _ = s.PutApp("todo", "", "", map[string][]byte{"index.html": []byte("y")})
	if m := recv(t, apps); m["app_reload"] != "todo" {
		t.Fatalf("reload %v", m)
	}
	select {
	case b := <-plain:
		t.Fatalf("a page without apps got %s", b)
	default:
	}
	if r, b := get(t, ts.URL+"/api/data/source/weather2", nil); r.StatusCode != 200 || b != `{"t":1}` {
		t.Fatalf("data source %d %s", r.StatusCode, b)
	}
}

func TestEmitHookAndLimit(t *testing.T) {
	s, ts := newTest(t, "tok")
	_, _ = s.Store.Ensure("office")
	emitURL := ts.URL + "/api/frames/office/emit?k=" + EmitKey("tok", "office")
	var got atomic.Value
	target := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		b, _ := io.ReadAll(r.Body)
		got.Store(r.Method + " " + r.URL.Path + " " + r.Header.Get("Authorization") + " " + string(b))
		_, _ = w.Write([]byte(`{"state":"on"}`))
	}))
	defer target.Close()
	if err := s.SetHook("lamp", Hook{URL: target.URL + "/api/services/{{app}}", Headers: map[string]string{"Authorization": "Bearer secret"},
		Body: `{"entity":{{data}},"frame":"{{frame}}"}`, ReturnBody: true}); err != nil {
		t.Fatal(err)
	}
	if h := s.Hooks()["lamp"]; h.Headers["Authorization"] != "(set)" || h.Method != "POST" {
		t.Fatalf("listed hook %+v", h)
	}
	r, b := post(t, emitURL, `{"app":"lights","name":"lamp","data":"light.desk"}`)
	if r.StatusCode != 200 || !strings.Contains(b, `"ok":true`) || !strings.Contains(b, `"state":"on"`) {
		t.Fatalf("emit: %d %s", r.StatusCode, b)
	}
	if g := got.Load(); g != `POST /api/services/lights Bearer secret {"entity":"light.desk","frame":"office"}` {
		t.Fatalf("hook request %q", g)
	}
	ev := s.AppEvents("office", "", 0)
	if len(ev) != 1 || !ev[0].Hook || ev[0].Status != 200 {
		t.Fatalf("events %+v", ev)
	}
	// No hook: recorded, ok.
	if r, b := post(t, emitURL, `{"name":"tap"}`); r.StatusCode != 200 || !strings.Contains(b, `"ok":true`) {
		t.Fatalf("plain emit %d %s", r.StatusCode, b)
	}
	if r, _ := post(t, emitURL, `{"name":"Bad Name"}`); r.StatusCode != 400 {
		t.Fatalf("bad name %d", r.StatusCode)
	}
	if r, _ := post(t, emitURL, strings.Repeat("x", MaxEmit+1)); r.StatusCode != 413 {
		t.Fatalf("big %d", r.StatusCode)
	}
	limited := false
	for range emitBurst + 2 {
		if r, _ := post(t, emitURL, `{"name":"tap"}`); r.StatusCode == 429 {
			limited = true
		}
	}
	if !limited {
		t.Fatal("emit not rate limited")
	}
	for _, bad := range []Hook{{URL: "file:///etc/passwd"}, {URL: "http://x", Method: "TRACE"}, {URL: "http://x", Headers: map[string]string{"A": "b\r\nc"}}, {URL: "http://x", TimeoutS: 99}} {
		if err := s.SetHook("x", bad); err == nil {
			t.Errorf("hook %+v accepted", bad)
		}
	}
	if !s.DeleteHook("lamp") || s.DeleteHook("lamp") {
		t.Fatal("delete hook")
	}
}

func TestPolls(t *testing.T) {
	s, _ := newTest(t, "tok")
	var n atomic.Int32
	target := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		n.Add(1)
		if r.Header.Get("X-Key") != "k" {
			w.WriteHeader(403)
			return
		}
		_, _ = w.Write([]byte(`{"data":{"items":[{"t":"a"},{"t":"b"}]}}`))
	}))
	defer target.Close()
	if err := s.SetPoll("feed", Poll{URL: target.URL, EveryS: 5}); err == nil {
		t.Fatal("every_s 5 accepted")
	}
	if err := s.SetPoll("feed", Poll{URL: target.URL, EveryS: 60, Headers: map[string]string{"X-Key": "k"}, Pick: "data.items.1"}); err != nil {
		t.Fatal(err)
	}
	s.pollDue(context.Background())
	deadline := time.Now().Add(3 * time.Second)
	for {
		if d, ok := s.Source("feed"); ok {
			if string(d) != `{"t":"b"}` {
				t.Fatalf("source %s", d)
			}
			break
		}
		if time.Now().After(deadline) {
			t.Fatalf("poll never stored: %+v", s.Polls())
		}
		time.Sleep(20 * time.Millisecond)
	}
	time.Sleep(50 * time.Millisecond)
	if p := s.Polls(); len(p) != 1 || p[0].LastOK.IsZero() || p[0].Poll.Headers["X-Key"] != "(set)" {
		t.Fatalf("status %+v", p)
	}
	s.pollDue(context.Background()) // not due again for 60 s
	time.Sleep(50 * time.Millisecond)
	if n.Load() != 1 {
		t.Fatalf("fetched %d times", n.Load())
	}
	for in, want := range map[string]string{`[1,2]`: `{"value":[1,2]}`, `42`: `{"value":42}`, `hello <b>`: `{"text":"hello <b>"}`, `{"a":"<"}`: `{"a":"<"}`} {
		if got, err := pollObject([]byte(in), ""); err != nil || string(got) != want {
			t.Errorf("pollObject(%s) = %s %v, want %s", in, got, err, want)
		}
	}
}

func TestScreenshot(t *testing.T) {
	s, ts := newTest(t, "tok")
	url := "ws" + strings.TrimPrefix(ts.URL, "http") + "/api/frames/sim/ws"
	c, _, err := websocket.Dial(context.Background(), url, &websocket.DialOptions{HTTPHeader: http.Header{"Authorization": {"Bearer tok"}}})
	if err != nil {
		t.Fatal(err)
	}
	defer closeAndWait(t, s, "sim", c)
	readMsg(t, c) // config
	done := make(chan []byte, 1)
	go func() {
		b, err := s.Screenshot(context.Background(), "sim")
		if err != nil {
			t.Error(err)
		}
		done <- b
	}()
	m := readMsg(t, c)
	if m["type"] != "action" || m["name"] != "screenshot" {
		t.Fatalf("action %v", m)
	}
	// 4x2 RGB: red, green, blue, white / black x4.
	px := []byte{255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0}
	req, _ := http.NewRequest("POST", ts.URL+"/api/frames/sim/screenshot?w=4&h=2", bytes.NewReader(px))
	req.Header.Set("Authorization", "Bearer tok")
	resp, err := http.DefaultClient.Do(req)
	if err != nil || resp.StatusCode != 201 {
		t.Fatalf("upload %v %v", err, resp.StatusCode)
	}
	resp.Body.Close()
	b := <-done
	img, err := png.Decode(bytes.NewReader(b))
	if err != nil {
		t.Fatal(err)
	}
	if r, g, bl, _ := img.At(2, 0).RGBA(); img.Bounds().Dx() != 4 || r != 0 || g != 0 || bl != 0xffff {
		t.Fatalf("pixel (2,0) = %d %d %d", r, g, bl)
	}
	small, err := ScalePNG(b, 2)
	if err != nil {
		t.Fatal(err)
	}
	si, _ := png.Decode(bytes.NewReader(small))
	if si.Bounds().Dx() != 2 || si.Bounds().Dy() != 1 {
		t.Fatalf("scaled %v", si.Bounds())
	}
	if latest, _, err := s.LatestScreenshot("sim"); err != nil || !bytes.Equal(latest, b) {
		t.Fatal("latest", err)
	}
	req, _ = http.NewRequest("POST", ts.URL+"/api/frames/sim/screenshot?w=4&h=2", bytes.NewReader(px[:5]))
	req.Header.Set("Authorization", "Bearer tok")
	if resp, _ := http.DefaultClient.Do(req); resp.StatusCode != 400 {
		t.Fatalf("short body %d", resp.StatusCode)
	}
	// A failure the frame reports ends the wait at once, with the reason.
	go func() {
		readMsg(t, c)
		send(t, c, map[string]any{"type": "event", "kind": "screenshot", "text": "failed: /dev/dri/card1: no CRTC is scanning out a framebuffer"})
	}()
	start := time.Now()
	if _, err := s.Screenshot(context.Background(), "sim"); err == nil || !strings.Contains(err.Error(), "screen is off") || time.Since(start) > 5*time.Second {
		t.Fatalf("failed screenshot: %v after %s", err, time.Since(start))
	}
	if _, err := s.Screenshot(context.Background(), "nobody"); err == nil {
		t.Fatal("screenshot of an unknown frame")
	}
}
