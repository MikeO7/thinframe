package mcp

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	sdk "github.com/modelcontextprotocol/go-sdk/mcp"

	"github.com/MikeO7/thinframe/server/internal/api"
	"github.com/MikeO7/thinframe/server/internal/store"
)

func TestAppTools(t *testing.T) {
	st, _ := store.Open(t.TempDir())
	s := api.New(st, api.Options{Token: "tok"})
	_, _ = s.Store.Ensure("kitchen")
	mux := http.NewServeMux()
	mux.Handle("/mcp", Handler(s, "test"))
	ts := httptest.NewServer(mux)
	defer ts.Close()
	client := sdk.NewClient(&sdk.Implementation{Name: "test", Version: "0"}, nil)
	sess, err := client.Connect(context.Background(), &sdk.StreamableClientTransport{
		Endpoint: ts.URL + "/mcp", HTTPClient: &http.Client{Transport: roundTripper{token: "tok"}}}, nil)
	if err != nil {
		t.Fatal(err)
	}
	defer sess.Close()
	call := func(name string, args map[string]any) (map[string]any, bool) {
		t.Helper()
		res, err := sess.CallTool(context.Background(), &sdk.CallToolParams{Name: name, Arguments: args})
		if err != nil {
			t.Fatalf("%s: %v", name, err)
		}
		m, _ := res.StructuredContent.(map[string]any)
		if res.IsError {
			b, _ := json.Marshal(res.Content)
			return map[string]any{"error": string(b)}, false
		}
		return m, true
	}
	if r, ok := call("app_guide", map[string]any{}); !ok || r["bytes"].(float64) < 1000 {
		t.Fatalf("guide %v", r)
	}
	if _, ok := call("put_app", map[string]any{"name": "hi", "files": map[string]any{"index.html": "<p>1</p>", "app.js": "1"},
		"binary_files": map[string]any{"a.png": "iVBORw0KGgo="}, "description": "hello"}); !ok {
		t.Fatal("put_app")
	}
	if r, ok := call("put_app", map[string]any{"name": "hi", "merge": true, "files": map[string]any{"app.js": "2"}, "remove": []string{"a.png"}}); !ok {
		t.Fatalf("merge put %v", r)
	}
	r, ok := call("get_app", map[string]any{"name": "hi"})
	files, _ := r["files"].(map[string]any)
	if !ok || files["app.js"] != "2" || files["index.html"] != "<p>1</p>" || len(r["binary_files"].(map[string]any)) != 0 || r["version"] != float64(2) {
		t.Fatalf("get_app %v", r)
	}
	if r, ok := call("show_app", map[string]any{"id": "kitchen", "app": "hi"}); !ok || r["url"] != "/apps/hi/index.html?frame=kitchen" {
		t.Fatalf("show_app %v", r)
	}
	if r, _ := call("list_apps", map[string]any{}); !strings.Contains(mustJSON(r), `"shown_on":["kitchen"]`) {
		t.Fatalf("list_apps %v", r)
	}
	if _, ok := call("delete_app", map[string]any{"name": "hi"}); ok {
		t.Fatal("deleted an app on screen")
	}
	if r, ok := call("rollback_app", map[string]any{"name": "hi"}); !ok || !strings.Contains(mustJSON(r), `"current":1`) {
		t.Fatalf("rollback %v", r)
	}
	if _, ok := call("set_hook", map[string]any{"name": "lamp", "url": "http://ha.local/x", "headers": map[string]any{"Authorization": "Bearer s3cret"}}); !ok {
		t.Fatal("set_hook")
	}
	if r, _ := call("list_hooks", map[string]any{}); strings.Contains(mustJSON(r), "s3cret") {
		t.Fatalf("hook secret listed: %v", r)
	}
	if _, ok := call("set_poll", map[string]any{"key": "feed", "url": "http://x.local/", "every_s": 5}); ok {
		t.Fatal("every_s 5 accepted")
	}
	if _, ok := call("set_app_state", map[string]any{"name": "hi", "key": "n", "value": 3}); !ok {
		t.Fatal("set_app_state")
	}
	if r, _ := call("get_app_state", map[string]any{"name": "hi"}); !strings.Contains(mustJSON(r), `"n":3`) {
		t.Fatalf("state %v", r)
	}
	if _, ok := call("send_to_app", map[string]any{"app": "hi", "name": "flash"}); !ok {
		t.Fatal("send_to_app")
	}
	if r, _ := call("get_app_events", map[string]any{}); r["events"] == nil {
		t.Fatalf("events %v", r)
	}
	// Not connected: a clear error, not a 25 s wait.
	if r, ok := call("screenshot", map[string]any{"id": "kitchen"}); ok || !strings.Contains(r["error"].(string), "not connected") {
		t.Fatalf("screenshot offline %v", r)
	}
	if _, ok := call("show_app", map[string]any{"id": "kitchen", "app": ""}); !ok {
		t.Fatal("back to dashboard")
	}
	if _, ok := call("delete_app", map[string]any{"name": "hi"}); !ok {
		t.Fatal("delete after leaving the screen")
	}
}

func mustJSON(v any) string { b, _ := json.Marshal(v); return string(b) }
