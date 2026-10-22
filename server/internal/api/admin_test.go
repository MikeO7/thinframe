package api

import (
	"net/http"
	"strings"
	"testing"
)

// adminDo sends an admin request without following redirects; user "" means no Basic auth.
func adminDo(t *testing.T, method, url, user, pass, form string, hdr map[string]string) *http.Response {
	t.Helper()
	req, _ := http.NewRequest(method, url, strings.NewReader(form))
	if form != "" {
		req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	}
	if user != "" {
		req.SetBasicAuth(user, pass)
	}
	for k, v := range hdr {
		req.Header.Set(k, v)
	}
	cl := http.Client{CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}
	resp, err := cl.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	resp.Body.Close()
	return resp
}

func TestAdminAuth(t *testing.T) {
	s, ts := newTest(t, "secret")
	_, _ = s.Store.Ensure("kitchen")
	posts := []struct{ path, form string }{
		{"/admin/kitchen/config", "rotation=2&brightness=50&name=K&screen_off=&screen_on=&url=&tags="},
		{"/admin/kitchen/layout", "layout=" + `{"cols":4,"rows":3,"widgets":[]}`},
		{"/admin/kitchen/action", "name=reload-page"},
		{"/admin/kitchen/unpair", ""},
	}
	if r := adminDo(t, "GET", ts.URL+"/admin", "", "", "", nil); r.StatusCode != 401 || r.Header.Get("WWW-Authenticate") != `Basic realm="thinframe"` {
		t.Fatalf("GET /admin without auth: %d %q", r.StatusCode, r.Header.Get("WWW-Authenticate"))
	}
	if r := adminDo(t, "GET", ts.URL+"/admin", "x", "wrong", "", nil); r.StatusCode != 401 {
		t.Fatalf("GET /admin wrong password: %d", r.StatusCode)
	}
	// A bearer token is not admin auth: the browser page uses Basic only.
	if r := adminDo(t, "GET", ts.URL+"/admin", "", "", "", map[string]string{"Authorization": "Bearer secret"}); r.StatusCode != 401 {
		t.Fatalf("GET /admin with bearer: %d", r.StatusCode)
	}
	for _, p := range posts {
		if r := adminDo(t, "POST", ts.URL+p.path, "", "", p.form, nil); r.StatusCode != 401 {
			t.Fatalf("POST %s without auth: %d", p.path, r.StatusCode)
		}
	}
	if f, _ := s.Store.Get("kitchen"); f.Config.Rotation != 3 || len(f.Pending) != 0 {
		t.Fatalf("unauthenticated post changed the frame: rotation %d, %d pending", f.Config.Rotation, len(f.Pending))
	}
	if r := adminDo(t, "GET", ts.URL+"/admin", "anyone", "secret", "", nil); r.StatusCode != 200 {
		t.Fatalf("GET /admin with Basic: %d", r.StatusCode)
	}
	for _, p := range posts {
		if r := adminDo(t, "POST", ts.URL+p.path, "anyone", "secret", p.form, nil); r.StatusCode != 303 || strings.Contains(r.Header.Get("Location"), "err=") {
			t.Fatalf("POST %s with Basic: %d %s", p.path, r.StatusCode, r.Header.Get("Location"))
		}
	}
	if f, _ := s.Store.Get("kitchen"); f.Config.Rotation != 2 || len(f.Pending) != 1 {
		t.Fatalf("authenticated posts did not apply: rotation %d, %d pending", f.Config.Rotation, len(f.Pending))
	}
}

func TestAdminNoTokenOpen(t *testing.T) {
	s, ts := newTest(t, "")
	_, _ = s.Store.Ensure("kitchen")
	if r := adminDo(t, "GET", ts.URL+"/admin", "", "", "", nil); r.StatusCode != 200 {
		t.Fatalf("GET /admin: %d", r.StatusCode)
	}
	if r := adminDo(t, "POST", ts.URL+"/admin/kitchen/action", "", "", "name=reload-page", map[string]string{"Sec-Fetch-Site": "same-origin"}); r.StatusCode != 303 {
		t.Fatalf("POST action: %d", r.StatusCode)
	}
}

func TestAdminRefusesCrossOrigin(t *testing.T) {
	for _, token := range []string{"", "secret"} {
		s, ts := newTest(t, token)
		_, _ = s.Store.Ensure("kitchen")
		if r := adminDo(t, "POST", ts.URL+"/admin/kitchen/action", "u", token, "name=reboot", map[string]string{"Sec-Fetch-Site": "cross-site"}); r.StatusCode != 403 {
			t.Fatalf("token %q: cross-site post: %d", token, r.StatusCode)
		}
		if r := adminDo(t, "POST", ts.URL+"/admin/kitchen/action", "u", token, "name=reboot", map[string]string{"Origin": "http://evil.example"}); r.StatusCode != 403 {
			t.Fatalf("token %q: foreign Origin post: %d", token, r.StatusCode)
		}
		if f, _ := s.Store.Get("kitchen"); len(f.Pending) != 0 {
			t.Fatalf("token %q: cross-site post queued %v", token, f.Pending)
		}
	}
}

func TestAdminFormBodyLimit(t *testing.T) {
	s, ts := newTest(t, "")
	_, _ = s.Store.Ensure("kitchen")
	big := "name=reload-page&arg=" + strings.Repeat("a", 1<<20)
	for _, p := range []string{"/admin/kitchen/config", "/admin/kitchen/layout", "/admin/kitchen/action"} {
		if r := adminDo(t, "POST", ts.URL+p, "", "", big, nil); r.StatusCode != 413 {
			t.Fatalf("1 MB form to %s: %d", p, r.StatusCode)
		}
	}
	if f, _ := s.Store.Get("kitchen"); len(f.Pending) != 0 {
		t.Fatalf("oversized form queued %v", f.Pending)
	}
}

func TestSetConfigAllOrNothing(t *testing.T) {
	s, ts := newTest(t, "")
	_, _ = s.Store.Ensure("kitchen")
	resp, body := do(t, "PUT", ts.URL+"/api/frames/kitchen/config", "", map[string]any{"url": "http://x", "rotation": 9, "name": "Bad"})
	if resp.StatusCode != 400 {
		t.Fatalf("want 400, got %d %s", resp.StatusCode, body)
	}
	f, _ := s.Store.Get("kitchen")
	if f.Config.URL != "" || f.Name == "Bad" || f.Config.Rotation != 3 {
		t.Fatalf("invalid patch partly saved: url %q name %q rotation %d", f.Config.URL, f.Name, f.Config.Rotation)
	}
}
