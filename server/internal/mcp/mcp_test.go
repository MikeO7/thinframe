package mcp

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	sdk "github.com/modelcontextprotocol/go-sdk/mcp"

	"github.com/MikeO7/thinframe/server/internal/api"
	"github.com/MikeO7/thinframe/server/internal/store"
)

func TestToolsOverHTTP(t *testing.T) {
	st, _ := store.Open(t.TempDir())
	s := api.New(st, api.Options{Token: "tok"})
	_, _ = s.Store.Ensure("kitchen")
	mux := http.NewServeMux()
	mux.Handle("/mcp", Handler(s, "test"))
	ts := httptest.NewServer(mux)
	defer ts.Close()

	// unauthenticated is refused
	resp, _ := http.Post(ts.URL+"/mcp", "application/json", strings.NewReader("{}"))
	if resp.StatusCode != 401 {
		t.Fatalf("want 401, got %d", resp.StatusCode)
	}

	hc := &http.Client{Transport: roundTripper{token: "tok"}}
	client := sdk.NewClient(&sdk.Implementation{Name: "test", Version: "0"}, nil)
	sess, err := client.Connect(context.Background(), &sdk.StreamableClientTransport{Endpoint: ts.URL + "/mcp", HTTPClient: hc}, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer sess.Close()

	tools, err := sess.ListTools(context.Background(), nil)
	if err != nil {
		t.Fatal(err)
	}
	names := map[string]bool{}
	for _, tl := range tools.Tools {
		names[tl.Name] = true
	}
	for _, want := range []string{"list_frames", "set_frame_config", "frame_action", "set_layout", "get_logs", "release_status"} {
		if !names[want] {
			t.Errorf("missing tool %s", want)
		}
	}

	res, err := sess.CallTool(context.Background(), &sdk.CallToolParams{Name: "set_frame_config", Arguments: map[string]any{"id": "kitchen", "rotation": 1}})
	if err != nil || res.IsError {
		t.Fatalf("call: %v %+v", err, res)
	}
	f, _ := s.Store.Get("kitchen")
	if f.Config.Rotation != 1 {
		t.Fatalf("rotation not applied: %+v", f.Config)
	}

	res, err = sess.CallTool(context.Background(), &sdk.CallToolParams{Name: "frame_action", Arguments: map[string]any{"name": "reboot"}})
	if err != nil {
		t.Fatal(err)
	}
	if !res.IsError {
		t.Fatalf("all-frames action without confirm should error: %+v", res)
	}

	res, _ = sess.CallTool(context.Background(), &sdk.CallToolParams{Name: "list_frames", Arguments: map[string]any{}})
	b, _ := json.Marshal(res.Content)
	if !strings.Contains(string(b), "kitchen") {
		t.Fatalf("list_frames: %s", b)
	}
}

type roundTripper struct{ token string }

func (r roundTripper) RoundTrip(req *http.Request) (*http.Response, error) {
	req.Header.Set("Authorization", "Bearer "+r.token)
	return http.DefaultTransport.RoundTrip(req)
}

// rpc posts one JSON-RPC message the way a non-Go client (Python or TypeScript SDK, curl) would
// and returns the decoded result or error object from the SSE or JSON body.
func rpc(t *testing.T, url string, hdr map[string]string, body string) (int, map[string]any) {
	t.Helper()
	req, _ := http.NewRequest("POST", url, strings.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Accept", "application/json, text/event-stream")
	req.Header.Set("Authorization", "Bearer tok")
	for k, v := range hdr {
		req.Header.Set(k, v)
	}
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	raw, _ := io.ReadAll(resp.Body)
	s := string(raw)
	if i := strings.Index(s, "data: "); i >= 0 {
		s = strings.TrimSpace(strings.SplitN(s[i+6:], "\n", 2)[0])
	}
	var m map[string]any
	_ = json.Unmarshal([]byte(s), &m)
	return resp.StatusCode, m
}

func newTS(t *testing.T) *httptest.Server {
	st, _ := store.Open(t.TempDir())
	s := api.New(st, api.Options{Token: "tok"})
	_, _ = s.Store.Ensure("kitchen")
	ts := httptest.NewServer(Handler(s, "v-test"))
	t.Cleanup(ts.Close)
	return ts
}

// Every protocol version the SDK negotiates works, the newest stateless one included.
func TestProtocolVersions(t *testing.T) {
	ts := newTS(t)
	for _, v := range []string{"2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"} {
		_, m := rpc(t, ts.URL, nil, `{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"`+v+
			`","capabilities":{},"clientInfo":{"name":"py","version":"1"}}}`)
		r, _ := m["result"].(map[string]any)
		if r["protocolVersion"] != v {
			t.Errorf("initialize %s: %v", v, m)
		}
		info, _ := r["serverInfo"].(map[string]any)
		if info["version"] != "v-test" || r["instructions"] != Instructions {
			t.Errorf("initialize %s: serverInfo/instructions %v", v, r)
		}
	}
	meta := `"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28",` +
		`"io.modelcontextprotocol/clientCapabilities":{},"io.modelcontextprotocol/clientInfo":{"name":"c","version":"1"}}`
	h := map[string]string{"MCP-Protocol-Version": "2026-07-28", "Mcp-Method": "server/discover"}
	_, m := rpc(t, ts.URL, h, `{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{`+meta+`}}`)
	r, _ := m["result"].(map[string]any)
	if vs, _ := r["supportedVersions"].([]any); len(vs) == 0 || vs[0] != "2026-07-28" {
		t.Fatalf("server/discover: %v", m)
	}
	h["Mcp-Method"] = "tools/list"
	_, m = rpc(t, ts.URL, h, `{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{`+meta+`}}`)
	r, _ = m["result"].(map[string]any)
	tools, _ := r["tools"].([]any)
	if len(tools) != 12 {
		t.Fatalf("tools/list (2026-07-28): %v", m)
	}
	for _, x := range tools {
		tl := x.(map[string]any)
		if tl["title"] == nil || tl["annotations"] == nil {
			t.Errorf("%v: no title or annotations", tl["name"])
		}
		if os, ok := tl["outputSchema"].(map[string]any); ok && os["type"] != "object" {
			t.Errorf("%v: outputSchema type %v, older clients need object", tl["name"], os["type"])
		}
	}
}

// structuredContent is an object for every read tool, so clients validating against the
// 2025-06-18 schema accept it, and the text fallback is not duplicated.
func TestResultsAreObjects(t *testing.T) {
	ts := newTS(t)
	client := sdk.NewClient(&sdk.Implementation{Name: "test", Version: "0"}, nil)
	sess, err := client.Connect(context.Background(), &sdk.StreamableClientTransport{
		Endpoint: ts.URL, HTTPClient: &http.Client{Transport: roundTripper{token: "tok"}}}, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer sess.Close()
	for name, args := range map[string]map[string]any{
		"list_frames": {}, "get_frame": {"id": "kitchen"}, "list_widgets": {}, "get_logs": {"id": "kitchen"},
		"release_status": {}, "frame_action": {"id": "kitchen", "name": "reload-page"},
	} {
		res, err := sess.CallTool(context.Background(), &sdk.CallToolParams{Name: name, Arguments: args})
		if err != nil || res.IsError {
			t.Fatalf("%s: %v %+v", name, err, res)
		}
		if _, ok := res.StructuredContent.(map[string]any); !ok {
			t.Errorf("%s: structuredContent %T, want object", name, res.StructuredContent)
		}
		if len(res.Content) != 1 {
			t.Errorf("%s: %d content blocks, want 1", name, len(res.Content))
		}
	}
}

func TestOriginAndAuth(t *testing.T) {
	ts := newTS(t)
	init := `{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"x","version":"1"}}}`
	if code, _ := rpc(t, ts.URL, map[string]string{"Origin": "https://evil.example", "Sec-Fetch-Site": "cross-site"}, init); code != http.StatusForbidden {
		t.Errorf("cross-site browser request: %d, want 403", code)
	}
	req, _ := http.NewRequest("POST", ts.URL, strings.NewReader(init))
	resp, _ := http.DefaultClient.Do(req)
	resp.Body.Close()
	if resp.StatusCode != 401 || !strings.HasPrefix(resp.Header.Get("WWW-Authenticate"), "Bearer") {
		t.Errorf("no token: %d %q", resp.StatusCode, resp.Header.Get("WWW-Authenticate"))
	}
}
