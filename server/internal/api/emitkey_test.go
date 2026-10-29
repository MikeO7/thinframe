package api

import (
	"context"
	"strings"
	"testing"
)

// TestEmitKey: with a server token, a public emit needs the frame's emit key, which only the
// documents the server serves carry (a cross-site page on the LAN could otherwise fire a hook
// such as the car's). Without a token (LAN dev mode) emit stays open.
func TestEmitKey(t *testing.T) {
	s, ts := newTest(t, "tok")
	_, _ = s.Store.Ensure("office")
	_, _ = s.Store.Ensure("hall")
	key := EmitKey("tok", "office")
	if len(key) != 32 || key == EmitKey("tok", "hall") || key == EmitKey("other", "office") || EmitKey("", "office") != "" {
		t.Fatalf("keys %q %q", key, EmitKey("tok", "hall"))
	}
	emit := ts.URL + "/api/frames/office/emit"
	for _, c := range []struct {
		url  string
		want int
	}{
		{emit, 403},
		{emit + "?k=", 403},
		{emit + "?k=" + key[:31], 403},
		{emit + "?k=" + strings.ToUpper(key), 403},
		{emit + "?k=" + EmitKey("tok", "hall"), 403}, // another frame's key
		{emit + "?k=" + key, 200},
		{ts.URL + "/api/frames/hall/emit?k=" + EmitKey("tok", "hall"), 200},
		// No key: no hint whether the frame exists. With its key, an unknown frame is a 404.
		{ts.URL + "/api/frames/stranger/emit", 403},
		{ts.URL + "/api/frames/stranger/emit?k=" + EmitKey("tok", "stranger"), 404},
	} {
		r, b := post(t, c.url, `{"name":"tap"}`)
		if r.StatusCode != c.want {
			t.Errorf("POST %s: %d %q, want %d", strings.TrimPrefix(c.url, ts.URL), r.StatusCode, b, c.want)
		}
		if r.StatusCode == 403 && r.Header.Get("Access-Control-Allow-Origin") != "null" {
			t.Errorf("403 without CORS: an app could not read why")
		}
	}
	if n := len(s.AppEvents("", "", 0)); n != 2 {
		t.Fatalf("%d events recorded, want 2 (refused emits record nothing)", n)
	}
	// The authenticated path (MCP send_to_app and the like) calls Emit directly: no key.
	if res := s.Emit(context.Background(), "office", "", "tap", nil); !res.OK {
		t.Fatalf("server-side emit: %+v", res)
	}

	// No token: open as before.
	d, dts := newTest(t, "")
	_, _ = d.Store.Ensure("office")
	if r, b := post(t, dts.URL+"/api/frames/office/emit", `{"name":"tap"}`); r.StatusCode != 200 {
		t.Fatalf("dev mode emit: %d %q", r.StatusCode, b)
	}
}

// TestEmitKeyInDocuments: the dashboard page and a full-screen app carry the frame's key; no
// other origin may read them (no CORS header) or frame them.
func TestEmitKeyInDocuments(t *testing.T) {
	s, ts := newTest(t, "tok")
	_, _ = s.Store.Ensure("office")
	key := EmitKey("tok", "office")
	r, page := get(t, ts.URL+"/frame/office", nil)
	if r.StatusCode != 200 || !strings.Contains(page, `k: "`+key+`"`) {
		t.Fatalf("page %d without the key", r.StatusCode)
	}
	if r.Header.Get("Access-Control-Allow-Origin") != "" || r.Header.Get("Content-Security-Policy") != "frame-ancestors 'self'" {
		t.Fatalf("page headers %v", r.Header)
	}
	if _, page := get(t, ts.URL+"/frame/hall", nil); !strings.Contains(page, `k: "`+EmitKey("tok", "hall")+`"`) || strings.Contains(page, key) {
		t.Fatal("unknown frame's page: wrong key")
	}

	if _, err := s.PutApp("car", "", "", map[string][]byte{
		"index.html": []byte("<!doctype html><head></head><body>car"),
		"other.html": []byte("<p>x"),
		"app.js":     []byte("1"),
	}); err != nil {
		t.Fatal(err)
	}
	r, doc := get(t, ts.URL+"/apps/car/index.html?frame=office", nil)
	if r.StatusCode != 200 || !strings.Contains(doc, `<head><script src="/apps-sdk.js" data-k="`+key+`"></script></head>`) {
		t.Fatalf("app document %d %q", r.StatusCode, doc)
	}
	csp := r.Header.Get("Content-Security-Policy")
	if r.Header.Get("Access-Control-Allow-Origin") != "" || !strings.HasPrefix(csp, "sandbox allow-scripts") || !strings.Contains(csp, "frame-ancestors 'self'") {
		t.Fatalf("app document headers %v", r.Header)
	}
	etag := r.Header.Get("ETag")
	if r, _ := get(t, ts.URL+"/apps/car/index.html?frame=office", map[string]string{"If-None-Match": etag}); r.StatusCode != 304 || r.Header.Get("Access-Control-Allow-Origin") != "" {
		t.Fatalf("revalidate %d %v", r.StatusCode, r.Header)
	}
	if _, doc := get(t, ts.URL+"/apps/car/other.html?frame=office", nil); !strings.Contains(doc, `data-k="`+key+`"`) {
		t.Fatalf("other page without the key: %q", doc)
	}
	// Without ?frame= (a widget's iframe, a preview): no key, and CORS as before.
	r, doc = get(t, ts.URL+"/apps/car/index.html", nil)
	if strings.Contains(doc, "data-k") || r.Header.Get("Access-Control-Allow-Origin") != "*" || r.Header.Get("ETag") == etag {
		t.Fatalf("keyless document %q %v", doc, r.Header)
	}
	// Assets and the SDK hold no key and keep CORS.
	for _, u := range []string{"/apps/car/app.js?frame=office", "/apps-sdk.js"} {
		if r, b := get(t, ts.URL+u, nil); r.Header.Get("Access-Control-Allow-Origin") != "*" || strings.Contains(b, key) {
			t.Fatalf("%s: %v", u, r.Header)
		}
	}
	// A server without a token injects nothing.
	d, dts := newTest(t, "")
	_, _ = d.PutApp("car", "", "", map[string][]byte{"index.html": []byte("<head></head>")})
	if r, doc := get(t, dts.URL+"/apps/car/index.html?frame=office", nil); strings.Contains(doc, "data-k") || r.Header.Get("Access-Control-Allow-Origin") != "*" {
		t.Fatalf("dev mode document %q %v", doc, r.Header)
	}
	if _, page := get(t, dts.URL+"/frame/office", nil); !strings.Contains(page, `k: ""`) {
		t.Fatal("dev mode page: key not empty")
	}
}
