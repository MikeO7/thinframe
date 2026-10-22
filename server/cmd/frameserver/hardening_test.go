package main

import (
	"bufio"
	"context"
	"errors"
	"net"
	"net/http"
	"net/url"
	"strings"
	"testing"
	"time"

	"github.com/coder/websocket"
	sdk "github.com/modelcontextprotocol/go-sdk/mcp"
)

func bearerAuth(r *http.Request) { r.Header.Set("Authorization", "Bearer "+token) }
func basicAuth(r *http.Request)  { r.SetBasicAuth("anyone", token) }

// TestBodyLimits: every route that reads a body bounds it. Oversized answers 413 or 400 and
// nothing is stored.
func TestBodyLimits(t *testing.T) {
	e := newEnv(t)
	big := func(n int) string { return strings.Repeat("x", n) }
	mcpAuth := func(r *http.Request) {
		bearerAuth(r)
		r.Header.Set("Accept", "application/json, text/event-stream")
	}
	cases := []struct {
		name, method, path, body, ctype string
		set                             func(*http.Request)
		want                            []int
	}{
		{"config body over 128 KiB", "PUT", "/api/frames/kitchen/config", `{"name":"` + big(200<<10) + `"}`, jsonT, bearerAuth, []int{400}},
		{"config name over field limit", "PUT", "/api/frames/kitchen/config", `{"name":"` + big(1000) + `"}`, jsonT, bearerAuth, []int{400}},
		{"config url over field limit", "PUT", "/api/frames/kitchen/config", `{"url":"http://x/` + big(4000) + `"}`, jsonT, bearerAuth, []int{400}},
		{"config too many tags", "PUT", "/api/frames/kitchen/config", `{"tags":[` + strings.Repeat(`"t",`, 40) + `"t"]}`, jsonT, bearerAuth, []int{400}},
		{"config wrong type", "PUT", "/api/frames/kitchen/config", `{"name":5}`, jsonT, bearerAuth, []int{400}},
		{"config tags wrong type", "PUT", "/api/frames/kitchen/config", `{"tags":7}`, jsonT, bearerAuth, []int{400}},
		{"layout over 1 MiB", "PUT", "/api/frames/kitchen/layout", `{"cols":4,"rows":3,"theme":"` + big(2<<20) + `"}`, jsonT, bearerAuth, []int{400}},
		{"heartbeat over 64 KiB", "POST", "/api/frames/kitchen/heartbeat", `{"ip":"` + big(100<<10) + `"}`, jsonT, bearerAuth, []int{400}},
		{"action over 64 KiB", "POST", "/api/frames/kitchen/actions", `{"name":"` + big(100<<10) + `"}`, jsonT, bearerAuth, []int{400}},
		{"beacon over 4 KiB", "POST", "/api/frames/kitchen/beacon", `{"widget":"` + big(1<<20) + `"}`, "text/plain", nil, []int{413}},
		{"pair request over 4 KiB", "POST", "/api/pair", `{"id":"bigpair","nonce":"` + big(10<<10) + `"}`, jsonT, nil, []int{400}},
		{"pair accept over 4 KiB", "POST", "/pair/pairacc/accept", "nonce=" + big(1<<20), form, nil, []int{413}},
		{"admin config over 64 KiB", "POST", "/admin/adminf/config", "name=" + big(100<<10), form, basicAuth, []int{413}},
		{"admin layout over 64 KiB", "POST", "/admin/adminf/layout", "layout=" + big(100<<10), form, basicAuth, []int{413}},
		{"admin action over 64 KiB", "POST", "/admin/adminf/action", "name=" + big(100<<10), form, basicAuth, []int{413}},
		{"mcp over 1 MiB", "POST", "/mcp", `{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"x":"` + big(2<<20) + `"}}`, jsonT, mcpAuth, []int{413, 400}},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			code, _, body := e.send(t, c.method, c.path, c.body, c.ctype, c.set)
			ok := false
			for _, w := range c.want {
				ok = ok || code == w
			}
			if !ok {
				t.Fatalf("status %d, want %v: %.200s", code, c.want, body)
			}
		})
	}
	// alive never reads its body: an oversized one is ignored, not buffered.
	if code, _, _ := e.send(t, "POST", "/api/frames/kitchen/alive", big(1<<20), "text/plain", nil); code != 204 {
		t.Fatalf("alive with a big body: %d", code)
	}
	f, _ := e.srv.Store.Get("kitchen")
	if f.Name != "" || len(f.Pending) != 0 || len(f.Config.Tags) != 0 {
		t.Fatalf("a refused body changed kitchen: name %q, %d pending, tags %v", f.Name, len(f.Pending), f.Config.Tags)
	}
	if _, err := e.srv.Store.Get("bigpair"); err == nil {
		t.Fatal("oversized pair request created a frame")
	}
}

// TestPathTraversal: URL segments that name files cannot leave their directory.
func TestPathTraversal(t *testing.T) {
	e := newEnv(t)
	if code, _, body := e.send(t, "GET", "/api/artifacts/tfctl-armv7", "", "", bearerAuth); code != 200 || body != "tfctl" {
		t.Fatalf("control: %d %q", code, body)
	}
	for _, p := range []string{
		"/api/artifacts/..",
		"/api/artifacts/%2e%2e",
		"/api/artifacts/../secret.txt",
		"/api/artifacts/..%2fsecret.txt",
		"/api/artifacts/%2e%2e%2fsecret.txt",
		"/api/artifacts/%2e%2e%5csecret.txt",
		"/api/artifacts/%2e%2e%2fsecret.txt.sha256",
		"/api/artifacts/%2fetc%2fpasswd",
		"/api/artifacts/%2F..%2F..%2Fsecret.txt",
		"/api/artifacts/secret.txt%00",
		"/api/artifacts/tfctl-armv7%00.sha256",
		"/api/artifacts/.hidden",
		"/api/artifacts/.hidden.sha256",
		"/frame/..",
		"/frame/%2e%2e",
		"/frame/..%2f..%2fsecret",
		"/frame/a%00b",
		"/api/stream/%2e%2e",
		"/api/stream/a%00b",
		"/api/frames/%2e%2e/config",
		"/api/frames/..%2fsecret/config",
	} {
		t.Run(p, func(t *testing.T) {
			code, _, body := e.send(t, "GET", p, "", "", bearerAuth)
			if code == 200 || code >= 500 || strings.Contains(body, secret) {
				t.Fatalf("status %d, body %.200q", code, body)
			}
		})
	}
	// The photo endpoint takes no path from the URL: a hostile frame id only picks a cursor.
	code, hdr, body := e.send(t, "GET", "/api/data/photo/next?frame="+url.QueryEscape("../../secret.txt")+"&w=1280&h=800", "", "", nil)
	if code != 200 || hdr.Get("Content-Type") != "image/jpeg" || strings.Contains(body, secret) {
		t.Fatalf("photo with a hostile frame id: %d %s", code, hdr.Get("Content-Type"))
	}
}

type authRT struct{}

func (authRT) RoundTrip(r *http.Request) (*http.Response, error) {
	r = r.Clone(r.Context())
	bearerAuth(r)
	return http.DefaultTransport.RoundTrip(r)
}

// TestMCPBadInputs: wrong types and huge strings come back as clean tool errors; nothing changes.
func TestMCPBadInputs(t *testing.T) {
	e := newEnv(t)
	ctx := context.Background()
	cl := sdk.NewClient(&sdk.Implementation{Name: "matrix", Version: "0"}, nil)
	sess, err := cl.Connect(ctx, &sdk.StreamableClientTransport{Endpoint: e.url + "/mcp", HTTPClient: &http.Client{Transport: authRT{}}}, nil)
	must(t, err)
	defer sess.Close()
	huge := strings.Repeat("x", 500<<10)
	for _, c := range []struct {
		name string
		tool string
		args map[string]any
	}{
		{"rotation as string", "set_frame_config", map[string]any{"id": "kitchen", "rotation": "abc"}},
		{"id as number", "set_frame_config", map[string]any{"id": 12, "brightness": 5}},
		{"huge name", "set_frame_config", map[string]any{"id": "kitchen", "name": huge}},
		{"huge splash text", "set_frame_config", map[string]any{"id": "kitchen", "splash_text": huge}},
		{"huge splash png", "set_frame_config", map[string]any{"id": "kitchen", "splash_png": huge}},
		{"tags as numbers", "set_frame_config", map[string]any{"id": "kitchen", "tags": []any{1, 2}}},
		{"huge id", "get_frame", map[string]any{"id": huge}},
		{"huge id write", "set_frame_config", map[string]any{"id": huge, "brightness": 5}},
		{"huge action name", "frame_action", map[string]any{"id": "kitchen", "name": huge}},
		{"layout not an object", "set_layout", map[string]any{"id": "kitchen", "layout": "nope"}},
		{"layout huge theme", "set_layout", map[string]any{"id": "kitchen", "layout": map[string]any{"cols": 4, "rows": 3, "theme": huge}}},
		{"n as string", "get_logs", map[string]any{"id": "kitchen", "n": "many"}},
		{"unknown tool", "no_such_tool", map[string]any{}},
	} {
		t.Run(c.name, func(t *testing.T) {
			res, err := sess.CallTool(ctx, &sdk.CallToolParams{Name: c.tool, Arguments: c.args})
			if err == nil && !res.IsError {
				t.Fatalf("accepted: %+v", res)
			}
		})
	}
	if res, err := sess.CallTool(ctx, &sdk.CallToolParams{Name: "list_frames", Arguments: map[string]any{}}); err != nil || res.IsError {
		t.Fatalf("server unusable after bad inputs: %v", err)
	}
	f, _ := e.srv.Store.Get("kitchen")
	if f.Name != "" || f.Config.SplashText != "" || len(f.Pending) != 0 || f.Config.Rotation != 3 {
		t.Fatalf("a refused call changed kitchen: %+v", f.Config)
	}
}

func wsURL(e *tenv, path string) string { return "ws" + strings.TrimPrefix(e.url, "http") + path }

// TestWebSocketAuth dials the control socket for real.
func TestWebSocketAuth(t *testing.T) {
	e := newEnv(t)
	for _, c := range []struct {
		name, path, auth string
		ok               bool
	}{
		{"no token", "/api/frames/kitchen/ws", "", false},
		{"wrong token", "/api/frames/kitchen/ws", "Bearer nope", false},
		{"basic", "/api/frames/kitchen/ws", "Basic YW55b25lOm1hdHJpeC1zZWNyZXQ=", false},
		{"other frame's token", "/api/frames/kitchen/ws", "Bearer " + hallTok, false},
		{"frame token for an unknown id", "/api/frames/ghost/ws", "Bearer " + frameTok, false},
		{"FRAME_TOKEN", "/api/frames/kitchen/ws", "Bearer " + token, true},
		{"own frame token", "/api/frames/kitchen/ws", "Bearer " + frameTok, true},
		{"token in query", "/api/frames/kitchen/ws?token=" + token, "", true},
		{"wrong token in query", "/api/frames/kitchen/ws?token=nope", "", false},
	} {
		t.Run(c.name, func(t *testing.T) {
			ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
			defer cancel()
			h := http.Header{}
			if c.auth != "" {
				h.Set("Authorization", c.auth)
			}
			conn, resp, err := websocket.Dial(ctx, wsURL(e, c.path), &websocket.DialOptions{HTTPHeader: h})
			if c.ok {
				if err != nil {
					t.Fatalf("dial: %v", err)
				}
				conn.CloseNow()
				return
			}
			if err == nil {
				conn.CloseNow()
				t.Fatal("dial succeeded")
			}
			if resp == nil || resp.StatusCode != 401 {
				t.Fatalf("want 401, got %v", resp)
			}
		})
	}
	// A message over the 64 KiB read limit closes the socket rather than being buffered.
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	conn, _, err := websocket.Dial(ctx, wsURL(e, "/api/frames/kitchen/ws"), &websocket.DialOptions{HTTPHeader: http.Header{"Authorization": {"Bearer " + token}}})
	must(t, err)
	defer conn.CloseNow()
	_ = conn.Write(ctx, websocket.MessageText, []byte(`{"type":"event","text":"`+strings.Repeat("x", 100<<10)+`"}`))
	for {
		if _, _, err := conn.Read(ctx); err != nil {
			if ctx.Err() != nil { // timed out: the server kept the socket open
				t.Fatalf("socket not closed for an oversized message: %v", err)
			}
			break
		}
	}
}

// TestGracefulShutdown: Shutdown with an open control socket and an open SSE stream returns
// cleanly and well inside shutdownTimeout, and both clients see their connection end.
func TestGracefulShutdown(t *testing.T) {
	e := newEnv(t)
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	must(t, err)
	hs := newHTTPServer("", handler(e.srv))
	served := make(chan error, 1)
	go func() { served <- hs.Serve(ln) }()
	base := "http://" + ln.Addr().String()

	ctx := context.Background()
	conn, _, err := websocket.Dial(ctx, "ws://"+ln.Addr().String()+"/api/frames/kitchen/ws", &websocket.DialOptions{HTTPHeader: http.Header{"Authorization": {"Bearer " + token}}})
	must(t, err)
	defer conn.CloseNow()
	wsDone := make(chan struct{})
	go func() {
		defer close(wsDone)
		for {
			if _, _, err := conn.Read(ctx); err != nil {
				return
			}
		}
	}()
	for i := 0; !e.srv.Hub.Online("kitchen"); i++ {
		if i > 200 {
			t.Fatal("socket never registered")
		}
		time.Sleep(10 * time.Millisecond)
	}
	resp, err := http.Get(base + "/api/stream/kitchen")
	must(t, err)
	defer resp.Body.Close()
	if resp.StatusCode != 200 {
		t.Fatalf("sse: %d", resp.StatusCode)
	}
	sseDone := make(chan struct{})
	go func() {
		defer close(sseDone)
		rd := bufio.NewReader(resp.Body)
		for {
			if _, err := rd.ReadString('\n'); err != nil {
				return
			}
		}
	}()

	sctx, cancel := context.WithTimeout(ctx, shutdownTimeout)
	defer cancel()
	start := time.Now()
	if err := hs.Shutdown(sctx); err != nil {
		t.Fatalf("Shutdown: %v after %s", err, time.Since(start))
	}
	if d := time.Since(start); d > shutdownTimeout/2 {
		t.Fatalf("Shutdown took %s", d)
	}
	if err := <-served; !errors.Is(err, http.ErrServerClosed) {
		t.Fatalf("Serve: %v", err)
	}
	for name, ch := range map[string]chan struct{}{"sse": sseDone, "websocket": wsDone} {
		select {
		case <-ch:
		case <-time.After(3 * time.Second):
			t.Fatalf("%s client still open after Shutdown", name)
		}
	}
}
