package api

import (
	"net/http"
	"net/http/httptest"
	"os"
	"testing"

	"github.com/MikeO7/thinframe/server/internal/store"
)

func TestHostAllowlist(t *testing.T) {
	hs := newHostSet(ParseHosts(" Server.NOX , frames.example.com."))
	host, _ := os.Hostname()
	for _, h := range []string{"192.168.1.10:41007", "10.0.0.2", "[fd00::1]:41007", "[::1]", "localhost:8080",
		"frameserver:8080", "nas", "nas.local", "nas.lan:41007", "server.home.arpa", "box.internal", "app.localhost",
		"server.nox:41007", "SERVER.NOX.", "frames.example.com", "", host} {
		if !hs.allowed(h) {
			t.Errorf("%q refused", h)
		}
	}
	for _, h := range []string{"evil.example", "evil.example:41007", "server.nox.evil.example", "local.evil.com", "1.2.3.4.nip.io"} {
		if hs.allowed(h) {
			t.Errorf("%q allowed", h)
		}
	}
}

// A DNS-rebinding page reaches the server under its own name: public routes answer 421, the
// token-protected ones (with a token set) are left to their token, so MCP clients keep working.
func TestHostCheckOnPublicRoutes(t *testing.T) {
	st, err := store.Open(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	s := New(st, Options{Token: "tok", Version: "test", Hosts: []string{"server.nox"}})
	h := s.Routes()
	send := func(host, path, tok string) int {
		req := httptest.NewRequest("GET", path, nil)
		req.Host = host
		if tok != "" {
			req.Header.Set("Authorization", "Bearer "+tok)
		}
		rec := httptest.NewRecorder()
		h.ServeHTTP(rec, req)
		return rec.Code
	}
	_, _ = st.Ensure("kitchen")
	for _, p := range []string{"/frame/kitchen", "/api/data/source/x", "/api/apps/x/state", "/healthz", "/api/pair/kitchen?nonce=0123456789abcdef"} {
		if c := send("rebind.evil.example", p, ""); c != http.StatusMisdirectedRequest {
			t.Errorf("%s via a foreign Host: %d", p, c)
		}
		if c := send("server.nox:41007", p, ""); c == http.StatusMisdirectedRequest {
			t.Errorf("%s via SERVER_HOSTS: 421", p)
		}
		if c := send("192.168.1.10:41007", p, ""); c == http.StatusMisdirectedRequest {
			t.Errorf("%s via an IP: 421", p)
		}
	}
	if c := send("mcp.example.com", "/api/frames", "tok"); c != 200 {
		t.Errorf("token route under another name: %d", c)
	}
	// Without a server token every route is public, so every route checks the Host.
	open := New(st, Options{Version: "test"})
	h = open.Routes()
	if c := send("rebind.evil.example", "/api/frames", ""); c != http.StatusMisdirectedRequest {
		t.Errorf("no-token server, foreign Host: %d", c)
	}
	if c := send("rebind.evil.example", "/admin", ""); c != http.StatusMisdirectedRequest {
		t.Errorf("no-token admin, foreign Host: %d", c)
	}
	if c := send("localhost:8080", "/api/frames", ""); c != 200 {
		t.Errorf("no-token server via localhost: %d", c)
	}
}

// Data, stream and state answers are readable by sandboxed apps and the file:// boot page
// (Origin "null" / "file://") and same-origin pages, not by other sites.
func TestDataCORS(t *testing.T) {
	s, ts := newTest(t, "")
	if err := s.SetSource("demo", []byte(`{"a":1}`)); err != nil {
		t.Fatal(err)
	}
	for _, p := range []string{"/api/data/source/demo", "/api/apps/none/state", "/api/data/calendar", "/api/data/weather"} {
		for origin, want := range map[string]string{"": "", "null": "null", "file://": "file://", "http://evil.example": ""} {
			req, _ := http.NewRequest("GET", ts.URL+p, nil)
			if origin != "" {
				req.Header.Set("Origin", origin)
			}
			resp, err := http.DefaultClient.Do(req)
			if err != nil {
				t.Fatal(err)
			}
			resp.Body.Close()
			if got := resp.Header.Get("Access-Control-Allow-Origin"); got != want {
				t.Errorf("%s from %q: ACAO %q, want %q", p, origin, got, want)
			}
		}
	}
	// Fonts may keep "*" (the saved boot page and apps load them with CORS).
	resp, err := http.Get(ts.URL + "/fonts/manrope.woff2")
	if err != nil {
		t.Fatal(err)
	}
	resp.Body.Close()
	if resp.Header.Get("Access-Control-Allow-Origin") != "*" {
		t.Errorf("fonts lost their CORS header")
	}
}
