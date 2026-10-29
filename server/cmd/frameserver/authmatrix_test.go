package main

// Route x auth matrix. Every pattern the server registers (api.Server.Patterns plus /mcp) must
// have a row here, so a new route without a decision about its auth fails the build's tests.
//
// Policy (README.md, server/README.md, docs/decisions.md):
//   - /api/frames*, /api/release, /api/artifacts/*, /mcp: Bearer FRAME_TOKEN. Some frame routes
//     also take the frame's own paired token ("frame"), release and artifacts any paired frame's
//     token ("anyFrame").
//   - /admin*: HTTP Basic, any user, password = FRAME_TOKEN. POSTs (and /mcp) refuse cross-origin.
//   - Public on purpose:
//     the dashboard page and its data plane (/frame/{id}, /api/stream/{id}, /api/data/*, beacon,
//     alive): cog loads a plain URL with no token and sendBeacon cannot set headers, so the page
//     and everything it calls carry no credentials. They only read what the page shows anyway,
//     and never create frame records.
//     pairing (/api/pair, /api/pair/{id}, /pair/{id}/accept): a frame without a token asks for
//     one and shows the code it got on its screen; the token is minted only when the owner types
//     that code on /admin/{id}/pair (admin auth) or MCP pair_frame, and handed only to the poll
//     with the request's nonce from the same IP (docs/decisions.md "Pairing"). /pair/{id}/accept
//     is the PAIR button of an older tfctl's local page (cross-origin by nature): it only shows
//     the code to the holder of the nonce.
//   - Every public route also checks the Host header (421 for a name that is not ours).
//     /healthz, and / (a redirect to /admin).
//     apps (/apps/*, /apps-sdk.js, /api/data/source/*, /api/apps/*/state*, emit): MCP-written
//     pages run sandboxed in an opaque origin with no token, like the dashboard; they read
//     sources and app state, and emit (rate limited, runs only hooks the owner set over MCP,
//     and needs the frame's emit key, which only the documents the server serves carry).
//   - Screenshots: the frame uploads with its paired token; reading one needs FRAME_TOKEN.

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"image"
	"image/color"
	"image/png"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/MikeO7/thinframe/server/internal/api"
	"github.com/MikeO7/thinframe/server/internal/artifacts"
	"github.com/MikeO7/thinframe/server/internal/photos"
	"github.com/MikeO7/thinframe/server/internal/store"
)

const (
	token    = "matrix-secret"
	frameTok = "kitchen-frame-token" // paired token of "kitchen"
	hallTok  = "hall-frame-token"    // paired token of "hall"
	nonceP   = "poll-nonce-0123456789"
	nonceA   = "accept-nonce-0123456789"
	secret   = "TOP-SECRET-OUTSIDE-ARTIFACTS"
)

type fakeWeather struct{}

func (fakeWeather) Current() (api.Weather, error) { return api.Weather{TempC: 21, Desc: "Sun"}, nil }

type fakeCal struct{}

func (fakeCal) Upcoming(time.Time, int) ([]api.Event, error) { return nil, nil }

type tenv struct {
	srv *api.Server
	url string
}

func sha(s string) string { h := sha256.Sum256([]byte(s)); return hex.EncodeToString(h[:]) }

// newEnv is a fully configured server (token, artifacts, photos, weather, calendar) behind the
// same handler main uses.
func newEnv(t *testing.T) *tenv {
	t.Helper()
	data := t.TempDir()
	st, err := store.Open(data)
	if err != nil {
		t.Fatal(err)
	}
	root := t.TempDir()
	art := filepath.Join(root, "artifacts")
	must(t, os.MkdirAll(art, 0o755))
	must(t, os.WriteFile(filepath.Join(art, "tfctl-armv7"), []byte("tfctl"), 0o644))
	must(t, os.WriteFile(filepath.Join(art, ".hidden"), []byte(secret), 0o644))
	must(t, os.WriteFile(filepath.Join(art, "versions.json"), []byte(`{"software":"1","system":"1","files":{"tfctl":"tfctl-armv7"}}`), 0o644))
	must(t, os.WriteFile(filepath.Join(root, "secret.txt"), []byte(secret), 0o644))
	pics := t.TempDir()
	img := image.NewRGBA(image.Rect(0, 0, 8, 8))
	img.Set(1, 1, color.White)
	fh, err := os.Create(filepath.Join(pics, "a.png"))
	must(t, err)
	must(t, png.Encode(fh, img))
	must(t, fh.Close())
	ph, err := photos.New(photos.Folder{Root: pics}, filepath.Join(data, "cache"))
	must(t, err)
	must(t, os.MkdirAll(filepath.Join(data, "assets"), 0o755))
	must(t, os.WriteFile(filepath.Join(data, "assets", "car.png"), []byte("\x89PNG\r\n\x1a\n"), 0o644))
	must(t, os.WriteFile(filepath.Join(data, "assets", "x.svg"), []byte("<svg/>"), 0o644))
	srv := api.New(st, api.Options{Token: token, Version: "test", Artifacts: artifacts.New(art), Photos: ph,
		Weather: fakeWeather{}, Calendar: fakeCal{}})
	for id, tok := range map[string]string{"kitchen": frameTok, "hall": hallTok, "adminf": "", "victim": ""} {
		_, err := st.Update(id, func(f *store.Frame) {
			if tok != "" {
				f.TokenHash = sha(tok)
			}
		})
		must(t, err)
	}
	must(t, srv.SetSource("demo", []byte(`{"items":[]}`)))
	_, err = srv.PutApp("demo", "", "", map[string][]byte{"index.html": []byte("<!doctype html><p>hi")})
	must(t, err)
	ts := httptest.NewServer(handler(srv))
	t.Cleanup(ts.Close)
	e := &tenv{srv: srv, url: ts.URL}
	// Open pairing requests for the poll and accept rows (same source IP as every test request).
	for id, n := range map[string]string{"pairpoll": nonceP, "pairacc": nonceA} {
		code, _, _ := e.send(t, "POST", "/api/pair", `{"id":"`+id+`","nonce":"`+n+`"}`, "application/json", nil)
		if code != http.StatusAccepted {
			t.Fatalf("pair request %s: %d", id, code)
		}
	}
	return e
}

func must(t *testing.T, err error) {
	t.Helper()
	if err != nil {
		t.Fatal(err)
	}
}

var noRedirect = &http.Client{CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}

// send issues one request. path is sent verbatim (no client-side cleaning or re-escaping).
// A text/event-stream answer is not read past its headers.
func (e *tenv) send(t *testing.T, method, path, body, ctype string, set func(*http.Request)) (int, http.Header, string) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, method, e.url, strings.NewReader(body))
	must(t, err)
	p, q, _ := strings.Cut(path, "?")
	req.URL.Opaque, req.URL.RawQuery = p, q
	if ctype != "" {
		req.Header.Set("Content-Type", ctype)
	}
	if set != nil {
		set(req)
	}
	resp, err := noRedirect.Do(req)
	if err != nil {
		t.Fatalf("%s %s: %v", method, path, err)
	}
	defer resp.Body.Close()
	if strings.HasPrefix(resp.Header.Get("Content-Type"), "text/event-stream") {
		return resp.StatusCode, resp.Header, ""
	}
	b, _ := io.ReadAll(io.LimitReader(resp.Body, 1<<20))
	return resp.StatusCode, resp.Header, string(b)
}

type class string

const (
	public   class = "public"   // no credentials needed
	pairing  class = "pairing"  // public on purpose: frames without a token
	bearer   class = "bearer"   // FRAME_TOKEN
	frame    class = "frame"    // FRAME_TOKEN or the paired token of the frame in the path
	anyFrame class = "anyFrame" // FRAME_TOKEN or any paired frame's token
	admin    class = "admin"    // Basic, password FRAME_TOKEN; POST refuses cross-origin
	mcpc     class = "mcp"      // FRAME_TOKEN; POST refuses cross-origin
)

type variant string

const (
	vNone        variant = "none"
	vWrongBearer variant = "wrong-bearer"
	vBearer      variant = "bearer"
	vFrame       variant = "frame-token"       // kitchen's paired token
	vOtherFrame  variant = "other-frame-token" // hall's paired token
	vWrongBasic  variant = "wrong-basic"
	vBasic       variant = "basic"
	vCrossOrigin variant = "cross-origin" // the row's right credential, sent from another site
)

var variants = []variant{vNone, vWrongBearer, vBearer, vFrame, vOtherFrame, vWrongBasic, vBasic, vCrossOrigin}

func (v variant) apply(c class) func(*http.Request) {
	return func(r *http.Request) {
		switch v {
		case vWrongBearer:
			r.Header.Set("Authorization", "Bearer not-the-token")
		case vBearer:
			r.Header.Set("Authorization", "Bearer "+token)
		case vFrame:
			r.Header.Set("Authorization", "Bearer "+frameTok)
		case vOtherFrame:
			r.Header.Set("Authorization", "Bearer "+hallTok)
		case vWrongBasic:
			r.SetBasicAuth("admin", "not-the-token")
		case vBasic:
			r.SetBasicAuth("anyone", token)
		case vCrossOrigin:
			if c == admin {
				r.SetBasicAuth("anyone", token)
			} else {
				r.Header.Set("Authorization", "Bearer "+token)
			}
			r.Header.Set("Origin", "http://evil.example")
			r.Header.Set("Sec-Fetch-Site", "cross-site")
		}
	}
}

type row struct {
	pattern string // registered pattern this row covers; "" = a path nothing should answer
	method  string
	path    string
	ghost   string // same route with an unknown id: unauthenticated answers must match path's
	class   class
	ok      int // status with the right credentials
	body    string
	ctype   string
	prep    func(*tenv) // before every request (restore state a successful request consumed)
}

// want is the status for a variant: ok with accepted credentials, 401 without, 403 cross-origin
// on the routes that refuse it.
func (r row) want(v variant) int {
	accepted := map[class][]variant{
		bearer:   {vBearer, vCrossOrigin},
		frame:    {vBearer, vFrame, vCrossOrigin},
		anyFrame: {vBearer, vFrame, vOtherFrame, vCrossOrigin},
		admin:    {vBasic, vCrossOrigin},
		mcpc:     {vBearer, vCrossOrigin},
	}
	switch {
	case r.class == public || r.class == pairing:
		return r.ok
	case v == vCrossOrigin && r.method == "POST" && (r.class == admin || r.class == mcpc):
		return http.StatusForbidden
	case slices.Contains(accepted[r.class], v):
		return r.ok
	}
	return http.StatusUnauthorized
}

const (
	form    = "application/x-www-form-urlencoded"
	jsonT   = "application/json"
	initRPC = `{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"matrix","version":"0"}}}`
)

func rows() []row {
	layout, _ := json.Marshal(store.DefaultLayout())
	ensure := func(id string) func(*tenv) { return func(e *tenv) { _, _ = e.srv.Store.Ensure(id) } }
	report := func(id string) func(*tenv) {
		return func(e *tenv) { _, _ = e.srv.SaveReport(id, []byte("==== tfctl\nmatrix\n")) }
	}
	return []row{
		// public
		{pattern: "GET /", method: "GET", path: "/", class: public, ok: http.StatusFound},
		{pattern: "GET /healthz", method: "GET", path: "/healthz", class: public, ok: 200},
		{pattern: "GET /frame/{id}", method: "GET", path: "/frame/kitchen", class: public, ok: 200},
		{pattern: "GET /api/stream/{id}", method: "GET", path: "/api/stream/kitchen", class: public, ok: 200},
		{pattern: "GET /api/data/photo/next", method: "GET", path: "/api/data/photo/next?frame=kitchen", class: public, ok: 200},
		{pattern: "GET /api/data/weather", method: "GET", path: "/api/data/weather", class: public, ok: 200},
		{pattern: "GET /api/data/calendar", method: "GET", path: "/api/data/calendar", class: public, ok: 200},
		{pattern: "GET /api/data/asset/{name}", method: "GET", path: "/api/data/asset/car.png", class: public, ok: 200},
		{pattern: "GET /api/data/img", method: "GET", path: "/api/data/img?u=https://example.com/x.jpg&w=96", class: public, ok: 404},
		{method: "GET", path: "/api/data/asset/x.svg", class: public, ok: 400},
		{pattern: "GET /fonts/{name}", method: "GET", path: "/fonts/manrope.woff2", class: public, ok: 200},
		{method: "GET", path: "/fonts/..%2Fwidgets.go", class: public, ok: 404},
		// Not configured in this matrix (no PROTECT_URL): public, and 404.
		{pattern: "GET /api/data/camera/{id}", method: "GET", path: "/api/data/camera/c1", class: public, ok: 404},
		{method: "GET", path: "/api/data/asset/..%2Fframes%2Fkitchen.json", class: public, ok: 400},
		{pattern: "POST /api/frames/{id}/beacon", method: "POST", path: "/api/frames/kitchen/beacon", class: public, ok: 204, body: `{"latency_ms":12}`, ctype: "text/plain"},
		{pattern: "POST /api/frames/{id}/alive", method: "POST", path: "/api/frames/kitchen/alive", class: public, ok: 204},
		// apps
		{pattern: "GET /apps/{name}/{path...}", method: "GET", path: "/apps/demo/index.html", class: public, ok: 200},
		{method: "GET", path: "/apps/demo/", class: public, ok: 200},
		{method: "GET", path: "/apps/demo/..%2Fmeta.json", class: public, ok: 404},
		{method: "GET", path: "/apps/demo/.hidden.js", class: public, ok: 404},
		{method: "GET", path: "/apps/nope/index.html", class: public, ok: 404},
		{pattern: "GET /apps-sdk.js", method: "GET", path: "/apps-sdk.js", class: public, ok: 200},
		{pattern: "GET /api/data/source/{key}", method: "GET", path: "/api/data/source/demo", class: public, ok: 200},
		{pattern: "POST /api/frames/{id}/emit", method: "POST", path: "/api/frames/kitchen/emit?k=" + api.EmitKey(token, "kitchen"), class: public, ok: 200, body: `{"app":"demo","name":"tap","data":1}`, ctype: "text/plain"},
		{method: "POST", path: "/api/frames/kitchen/emit", class: public, ok: 403, body: `{"name":"tap"}`, ctype: "text/plain"},
		{method: "POST", path: "/api/frames/kitchen/emit?k=" + api.EmitKey(token, "hall"), class: public, ok: 403, body: `{"name":"tap"}`, ctype: "text/plain"},
		{method: "POST", path: "/api/frames/stranger/emit?k=" + api.EmitKey(token, "stranger"), class: public, ok: 404, body: `{"name":"tap"}`, ctype: "text/plain"},
		{pattern: "GET /api/apps/{name}/state", method: "GET", path: "/api/apps/demo/state", class: public, ok: 200},
		{pattern: "POST /api/apps/{name}/state/{key}", method: "POST", path: "/api/apps/demo/state/count", class: public, ok: 204, body: `3`, ctype: "text/plain"},
		// pairing
		{pattern: "POST /api/pair", method: "POST", path: "/api/pair", class: pairing, ok: 202, body: `{"id":"pairreq","nonce":"req-nonce-0123456789"}`, ctype: jsonT},
		{pattern: "GET /api/pair/{id}", method: "GET", path: "/api/pair/pairpoll?nonce=" + nonceP, class: pairing, ok: 202},
		{pattern: "POST /pair/{id}/accept", method: "POST", path: "/pair/pairacc/accept", class: pairing, ok: 200, body: "nonce=" + nonceA, ctype: form},
		// bearer
		{pattern: "GET /api/frames", method: "GET", path: "/api/frames", class: bearer, ok: 200},
		{pattern: "GET /api/frames/{id}", method: "GET", path: "/api/frames/kitchen", ghost: "/api/frames/ghost", class: frame, ok: 200},
		{pattern: "DELETE /api/frames/{id}", method: "DELETE", path: "/api/frames/victim", ghost: "/api/frames/ghost", class: bearer, ok: 204, prep: ensure("victim")},
		{pattern: "GET /api/frames/{id}/config", method: "GET", path: "/api/frames/kitchen/config", ghost: "/api/frames/ghost/config", class: frame, ok: 200},
		{pattern: "PUT /api/frames/{id}/config", method: "PUT", path: "/api/frames/kitchen/config", ghost: "/api/frames/ghost/config", class: bearer, ok: 200, body: `{"brightness":40}`, ctype: jsonT},
		{pattern: "PUT /api/frames/{id}/layout", method: "PUT", path: "/api/frames/kitchen/layout", ghost: "/api/frames/ghost/layout", class: bearer, ok: 200, body: string(layout), ctype: jsonT},
		{pattern: "POST /api/frames/{id}/heartbeat", method: "POST", path: "/api/frames/kitchen/heartbeat", ghost: "/api/frames/ghost/heartbeat", class: frame, ok: 200, body: `{"ip":"10.0.0.2"}`, ctype: jsonT},
		{pattern: "POST /api/frames/{id}/actions", method: "POST", path: "/api/frames/kitchen/actions", ghost: "/api/frames/ghost/actions", class: bearer, ok: 202, body: `{"name":"reload-page"}`, ctype: jsonT},
		{pattern: "POST /api/frames/{id}/report", method: "POST", path: "/api/frames/kitchen/report", ghost: "/api/frames/ghost/report", class: frame, ok: 201, body: "==== tfctl\nreport\n", ctype: "text/plain"},
		{pattern: "GET /api/frames/{id}/reports", method: "GET", path: "/api/frames/kitchen/reports", ghost: "/api/frames/ghost/reports", class: bearer, ok: 200},
		{pattern: "GET /api/frames/{id}/reports/{name}", method: "GET", path: "/api/frames/kitchen/reports/latest", ghost: "/api/frames/ghost/reports/latest", class: bearer, ok: 200, prep: report("kitchen")},
		// A plain GET is not an upgrade: 426 once authorized (TestWebSocketAuth dials for real).
		{pattern: "GET /api/frames/{id}/ws", method: "GET", path: "/api/frames/kitchen/ws", ghost: "/api/frames/ghost/ws", class: frame, ok: http.StatusUpgradeRequired},
		{pattern: "POST /api/frames/{id}/screenshot", method: "POST", path: "/api/frames/kitchen/screenshot?w=2&h=1", ghost: "/api/frames/ghost/screenshot?w=2&h=1", class: frame, ok: 201, body: "\x00\x01\x02\x03\x04\x05", ctype: "application/octet-stream"},
		{pattern: "GET /api/frames/{id}/screenshot", method: "GET", path: "/api/frames/kitchen/screenshot", ghost: "/api/frames/ghost/screenshot", class: bearer, ok: 200},
		{pattern: "GET /api/sources", method: "GET", path: "/api/sources", class: bearer, ok: 200},
		{pattern: "GET /api/sources/{key}", method: "GET", path: "/api/sources/demo", ghost: "/api/sources/ghost", class: bearer, ok: 200},
		{pattern: "PUT /api/sources/{key}", method: "PUT", path: "/api/sources/demo", class: bearer, ok: 204, body: `{"items":[{"label":"a"}]}`, ctype: jsonT},
		{pattern: "DELETE /api/sources/{key}", method: "DELETE", path: "/api/sources/gone", ghost: "/api/sources/ghost", class: bearer, ok: 204,
			prep: func(e *tenv) { _ = e.srv.SetSource("gone", []byte(`{}`)) }},
		{pattern: "GET /api/release", method: "GET", path: "/api/release", class: anyFrame, ok: 200},
		{pattern: "GET /api/artifacts/{name}", method: "GET", path: "/api/artifacts/tfctl-armv7", ghost: "/api/artifacts/nope", class: anyFrame, ok: 200},
		// admin
		{pattern: "GET /admin", method: "GET", path: "/admin", class: admin, ok: 200},
		{pattern: "GET /admin/{id}/reports/{name}", method: "GET", path: "/admin/kitchen/reports/latest", ghost: "/admin/ghost/reports/latest", class: admin, ok: 200, prep: report("kitchen")},
		{pattern: "POST /admin/{id}/config", method: "POST", path: "/admin/adminf/config", ghost: "/admin/ghost/config", class: admin, ok: 303, body: "rotation=1&brightness=50&name=A&screen_off=&screen_on=&url=&tags=", ctype: form},
		{pattern: "POST /admin/{id}/layout", method: "POST", path: "/admin/adminf/layout", ghost: "/admin/ghost/layout", class: admin, ok: 303, body: "layout=" + url.QueryEscape(string(layout)), ctype: form},
		{pattern: "POST /admin/{id}/action", method: "POST", path: "/admin/adminf/action", ghost: "/admin/ghost/action", class: admin, ok: 303, body: "name=reload-page", ctype: form},
		{pattern: "POST /admin/{id}/unpair", method: "POST", path: "/admin/adminf/unpair", ghost: "/admin/ghost/unpair", class: admin, ok: 303, ctype: form},
		{pattern: "POST /admin/{id}/pair", method: "POST", path: "/admin/adminf/pair", ghost: "/admin/ghost/pair", class: admin, ok: 303, body: "code=000000", ctype: form},
		// mcp (stateless streamable HTTP: POST carries JSON-RPC, GET has no stream to offer)
		{pattern: "/mcp", method: "POST", path: "/mcp", class: mcpc, ok: 200, body: initRPC, ctype: jsonT},
		{pattern: "/mcp", method: "GET", path: "/mcp", class: mcpc, ok: http.StatusMethodNotAllowed},
		// Nothing else answers: unregistered paths and methods get 404/405 without credentials.
		{method: "GET", path: "/admin/adminf/config", class: public, ok: 404},
		{method: "GET", path: "/static/page.js", class: public, ok: 404},
		{method: "GET", path: "/metrics", class: public, ok: 404},
		{method: "GET", path: "/mcp/x", class: public, ok: 404},
		{method: "POST", path: "/api/frames", class: public, ok: 405},
		{method: "POST", path: "/admin", class: public, ok: 405},
	}
}

func TestRouteAuthMatrix(t *testing.T) {
	e := newEnv(t)
	all := rows()
	covered := map[string]bool{}
	for _, r := range all {
		covered[r.pattern] = true
	}
	for _, p := range append(e.srv.Patterns(), "/mcp") {
		if !covered[p] {
			t.Errorf("route %q has no row in the auth matrix: decide its auth and add one", p)
		}
	}
	for _, r := range all {
		for _, v := range variants {
			t.Run(r.method+" "+r.path+" "+string(v), func(t *testing.T) {
				if r.prep != nil {
					r.prep(e)
				}
				set := v.apply(r.class)
				if r.pattern == "/mcp" {
					set = func(req *http.Request) {
						v.apply(r.class)(req)
						req.Header.Set("Accept", "application/json, text/event-stream")
					}
				}
				code, hdr, body := e.send(t, r.method, r.path, r.body, r.ctype, set)
				want := r.want(v)
				if code != want {
					t.Fatalf("status %d, want %d (%s): %.200s", code, want, r.class, body)
				}
				if code == http.StatusUnauthorized {
					scheme := "Bearer"
					if r.class == admin {
						scheme = "Basic"
					}
					if !strings.HasPrefix(hdr.Get("WWW-Authenticate"), scheme) {
						t.Fatalf("401 without a %s challenge: %q", scheme, hdr.Get("WWW-Authenticate"))
					}
				}
				// An unauthenticated caller learns nothing about which ids exist.
				if r.ghost != "" && want == http.StatusUnauthorized {
					gcode, _, gbody := e.send(t, r.method, r.ghost, r.body, r.ctype, set)
					if gcode != code || gbody != body {
						t.Fatalf("unknown id answers %d %q, known id %d %q", gcode, gbody, code, body)
					}
				}
			})
		}
	}
	// Refused requests changed nothing: kitchen still has its token and its config.
	f, err := e.srv.Store.Get("kitchen")
	if err != nil || f.TokenHash != sha(frameTok) {
		t.Fatalf("kitchen lost its pairing: %v", err)
	}
}

// TestPublicRoutesDoNotCreateFrames: the public page plane never writes a frame record.
func TestPublicRoutesDoNotCreateFrames(t *testing.T) {
	e := newEnv(t)
	before := len(e.srv.Store.List())
	for _, p := range []struct{ method, path, body string }{
		{"GET", "/frame/stranger", ""},
		{"GET", "/api/stream/stranger", ""},
		{"GET", "/api/data/photo/next?frame=stranger", ""},
		{"POST", "/api/frames/stranger/beacon", `{"latency_ms":5}`},
		{"POST", "/api/frames/stranger/alive", ""},
		{"POST", "/api/frames/stranger/emit?k=" + api.EmitKey(token, "stranger"), `{"name":"tap"}`},
		{"GET", "/apps/stranger/index.html", ""},
		{"GET", "/api/pair/stranger?nonce=0123456789abcdef", ""},
		{"POST", "/pair/stranger/accept", "nonce=0123456789abcdef"},
	} {
		code, _, _ := e.send(t, p.method, p.path, p.body, form, nil)
		if code >= 500 {
			t.Errorf("%s %s: %d", p.method, p.path, code)
		}
	}
	if _, err := e.srv.Store.Get("stranger"); err == nil || len(e.srv.Store.List()) != before {
		t.Fatalf("a public request created a frame record")
	}
	// Invalid ids are refused, not a 500.
	if code, _, _ := e.send(t, "GET", "/frame/not.valid", "", "", nil); code != 404 {
		t.Fatalf("GET /frame/not.valid: %d", code)
	}
}
