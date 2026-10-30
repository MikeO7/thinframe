package mcp

import (
	"context"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	sdk "github.com/modelcontextprotocol/go-sdk/mcp"

	"github.com/MikeO7/thinframe/server/internal/api"
	"github.com/MikeO7/thinframe/server/internal/store"
)

// An MCP-only token opens /mcp and nothing else.
func TestMCPTokenOpensOnlyMCP(t *testing.T) {
	const extra = "openbot-0123456789abcdef0123456789abcdef"
	st, _ := store.Open(t.TempDir())
	s := api.New(st, api.Options{Token: "tok", MCPTokens: []string{extra}})
	mux := http.NewServeMux()
	mux.Handle("/mcp", Handler(s, "test"))
	mux.Handle("/", s.Routes())
	ts := httptest.NewServer(mux)
	defer ts.Close()

	for _, tok := range []string{extra, "tok"} {
		client := sdk.NewClient(&sdk.Implementation{Name: "test", Version: "0"}, nil)
		sess, err := client.Connect(context.Background(), &sdk.StreamableClientTransport{Endpoint: ts.URL + "/mcp",
			HTTPClient: &http.Client{Transport: roundTripper{token: tok}}}, nil)
		if err != nil {
			t.Fatalf("token %.6s: %v", tok, err)
		}
		if _, err := sess.ListTools(context.Background(), nil); err != nil {
			t.Fatalf("token %.6s: list tools: %v", tok, err)
		}
		sess.Close()
	}

	for _, tc := range []struct {
		path, token string
		want        int
	}{
		{"/mcp", "wrong-0123456789abcdef0123456789abcdef", 401},
		{"/api/frames", extra, 401},
		{"/api/frames", "tok", 200},
	} {
		req, _ := http.NewRequest("GET", ts.URL+tc.path, nil)
		if tc.path == "/mcp" {
			req, _ = http.NewRequest("POST", ts.URL+tc.path, strings.NewReader("{}"))
		}
		req.Header.Set("Authorization", "Bearer "+tc.token)
		resp, err := http.DefaultClient.Do(req)
		if err != nil {
			t.Fatal(err)
		}
		resp.Body.Close()
		if resp.StatusCode != tc.want {
			t.Errorf("%s with %.6s: %d; want %d", tc.path, tc.token, resp.StatusCode, tc.want)
		}
	}
}
