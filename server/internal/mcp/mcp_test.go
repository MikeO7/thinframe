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

func TestToolsOverHTTP(t *testing.T) {
	st, _ := store.Open(t.TempDir())
	s := api.New(st, api.Options{Token: "tok"})
	_, _ = s.Store.Ensure("kitchen")
	mux := http.NewServeMux()
	mux.Handle("/mcp", Handler(s))
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
