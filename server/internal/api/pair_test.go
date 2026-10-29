package api

import (
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
	"time"
)

func accept(t *testing.T, base, id, nonce string) int {
	resp, err := http.PostForm(base+"/pair/"+id+"/accept", url.Values{"nonce": {nonce}})
	if err != nil {
		t.Fatal(err)
	}
	resp.Body.Close()
	return resp.StatusCode
}

func TestPairOnce(t *testing.T) {
	_, ts := newTest(t, "admin-secret")
	const nonce = "0123456789abcdef0123"
	if r, _ := do(t, "POST", ts.URL+"/api/pair", "", map[string]string{"id": "kitchen", "nonce": nonce}); r.StatusCode != 202 {
		t.Fatalf("request: %d", r.StatusCode)
	}
	if r, _ := do(t, "GET", ts.URL+"/api/pair/kitchen?nonce="+nonce, "", nil); r.StatusCode != 202 {
		t.Fatalf("poll before the button: %d", r.StatusCode)
	}
	if code := accept(t, ts.URL, "kitchen", "wrong-nonce-0000000"); code != 404 {
		t.Fatalf("accept with a wrong nonce: %d", code)
	}
	if r, _ := do(t, "GET", ts.URL+"/api/pair/kitchen?nonce=wrong", "", nil); r.StatusCode != 404 {
		t.Fatalf("poll with a wrong nonce: %d", r.StatusCode)
	}
	if code := accept(t, ts.URL, "kitchen", nonce); code != 200 {
		t.Fatalf("accept: %d", code)
	}
	r, body := do(t, "GET", ts.URL+"/api/pair/kitchen?nonce="+nonce, "", nil)
	var got struct{ Token string }
	_ = json.Unmarshal(body, &got)
	if r.StatusCode != 200 || len(got.Token) != 64 {
		t.Fatalf("pickup: %d %s", r.StatusCode, body)
	}
	if r, _ := do(t, "GET", ts.URL+"/api/pair/kitchen?nonce="+nonce, "", nil); r.StatusCode != 404 {
		t.Fatalf("token handed out twice: %d", r.StatusCode)
	}
	// The token works for this frame's own routes and nothing else.
	if r, _ := do(t, "GET", ts.URL+"/api/frames/kitchen/config", got.Token, nil); r.StatusCode != 200 {
		t.Fatalf("own config: %d", r.StatusCode)
	}
	if r, _ := do(t, "GET", ts.URL+"/api/release", got.Token, nil); r.StatusCode != 200 {
		t.Fatalf("release: %d", r.StatusCode)
	}
	for _, c := range []struct{ m, p string }{{"GET", "/api/frames/other/config"}, {"GET", "/api/frames"}, {"PUT", "/api/frames/kitchen/layout"}, {"DELETE", "/api/frames/kitchen"}} {
		if r, _ := do(t, c.m, ts.URL+c.p, got.Token, nil); r.StatusCode != 401 {
			t.Fatalf("%s %s with a frame token: %d", c.m, c.p, r.StatusCode)
		}
	}
	// Once paired, the id refuses new requests until the admin forgets it.
	if r, _ := do(t, "POST", ts.URL+"/api/pair", "", map[string]string{"id": "kitchen", "nonce": "fedcba9876543210fedc"}); r.StatusCode != 409 {
		t.Fatalf("second request: %d", r.StatusCode)
	}
	req, _ := http.NewRequest("POST", ts.URL+"/admin/kitchen/unpair", nil)
	req.SetBasicAuth("admin", "admin-secret")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	resp.Body.Close()
	if resp.StatusCode != 200 { // 303 to /admin, followed with the same credentials
		t.Fatalf("unpair: %d", resp.StatusCode)
	}
	if r, _ := do(t, "GET", ts.URL+"/api/frames/kitchen/config", got.Token, nil); r.StatusCode != 401 {
		t.Fatalf("token after unpair: %d", r.StatusCode)
	}
	if r, _ := do(t, "POST", ts.URL+"/api/pair", "", map[string]string{"id": "kitchen", "nonce": "fedcba9876543210fedc"}); r.StatusCode != 202 {
		t.Fatalf("request after unpair: %d", r.StatusCode)
	}
}

func TestPairRejectsBadInputAndExpires(t *testing.T) {
	s, ts := newTest(t, "")
	for _, in := range []map[string]string{{"id": "kitchen", "nonce": "short"}, {"id": "../x", "nonce": "0123456789abcdef"}, {}} {
		if r, _ := do(t, "POST", ts.URL+"/api/pair", "", in); r.StatusCode != 400 {
			t.Fatalf("%v: %d", in, r.StatusCode)
		}
	}
	const nonce = "0123456789abcdef"
	do(t, "POST", ts.URL+"/api/pair", "", map[string]string{"id": "hall", "nonce": nonce})
	s.mu.Lock()
	s.pair.open["hall"].created = time.Now().Add(-pairTTL - time.Second)
	s.mu.Unlock()
	if code := accept(t, ts.URL, "hall", nonce); code != 404 {
		t.Fatalf("accept after expiry: %d", code)
	}
	r, body := do(t, "GET", ts.URL+"/admin", "", nil)
	if r.StatusCode != 200 || strings.Contains(string(body), "forget pairing") {
		t.Fatalf("admin shows an unpaired frame as paired")
	}
}

// from sends one pairing call to h as if from ip. JSON body for /api/pair, form for accept.
func from(h http.Handler, ip, method, path, body string) int {
	req := httptest.NewRequest(method, path, strings.NewReader(body))
	req.Host = "frames.local" // httptest's example.com is not a LAN name (hosts.go)
	req.RemoteAddr = ip + ":40000"
	if strings.HasSuffix(path, "/accept") {
		req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	}
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, req)
	return rec.Code
}

func pairBody(id, nonce string) string { return fmt.Sprintf(`{"id":%q,"nonce":%q}`, id, nonce) }

// Another LAN host can neither accept, collect nor replace a frame's open request.
func TestPairBoundToSourceIP(t *testing.T) {
	s, _ := newTest(t, "")
	h := s.Routes()
	const frame, other = "192.168.1.50", "192.168.1.66"
	const nonce, nonce2 = "0123456789abcdef0123", "fedcba9876543210fedc"
	if c := from(h, frame, "POST", "/api/pair", pairBody("kitchen", nonce)); c != 202 {
		t.Fatalf("request: %d", c)
	}
	if c := from(h, other, "POST", "/api/pair", pairBody("kitchen", nonce2)); c != 409 {
		t.Fatalf("other IP replaced an open request: %d", c)
	}
	if c := from(h, other, "POST", "/pair/kitchen/accept", "nonce="+nonce); c != 404 {
		t.Fatalf("accept from another IP: %d", c)
	}
	if c := from(h, other, "GET", "/api/pair/kitchen?nonce="+nonce, ""); c != 404 {
		t.Fatalf("poll from another IP: %d", c)
	}
	// A rebooted frame (same IP, new nonce) replaces its own request; the old nonce is dead.
	if c := from(h, frame, "POST", "/api/pair", pairBody("kitchen", nonce2)); c != 202 {
		t.Fatalf("same IP replace: %d", c)
	}
	if c := from(h, frame, "POST", "/pair/kitchen/accept", "nonce="+nonce); c != 404 {
		t.Fatalf("accept with the replaced nonce: %d", c)
	}
	if c := from(h, frame, "POST", "/pair/kitchen/accept", "nonce="+nonce2); c != 200 {
		t.Fatalf("accept from the frame: %d", c)
	}
	if c := from(h, other, "GET", "/api/pair/kitchen?nonce="+nonce2, ""); c != 404 {
		t.Fatalf("token handed to another IP: %d", c)
	}
	if c := from(h, frame, "GET", "/api/pair/kitchen?nonce="+nonce2, ""); c != 200 {
		t.Fatalf("pickup from the frame: %d", c)
	}
	// An expired request from one IP no longer blocks another.
	from(h, frame, "POST", "/api/pair", pairBody("hall", nonce))
	s.mu.Lock()
	s.pair.open["hall"].created = time.Now().Add(-pairTTL - time.Second)
	s.mu.Unlock()
	if c := from(h, other, "POST", "/api/pair", pairBody("hall", nonce2)); c != 202 {
		t.Fatalf("other IP after expiry: %d", c)
	}
}

func TestPairCapPerIP(t *testing.T) {
	s, _ := newTest(t, "")
	h := s.Routes()
	const nonce = "0123456789abcdef0123"
	for i := range pairMaxIP {
		if c := from(h, "10.0.0.9", "POST", "/api/pair", pairBody(fmt.Sprintf("f%d", i), nonce)); c != 202 {
			t.Fatalf("request %d: %d", i, c)
		}
	}
	if c := from(h, "10.0.0.9", "POST", "/api/pair", pairBody("one-more", nonce)); c != 429 {
		t.Fatalf("over the per-IP cap: %d", c)
	}
	if c := from(h, "10.0.0.9", "POST", "/api/pair", pairBody("f0", "fedcba9876543210fedc")); c != 202 {
		t.Fatalf("replacing its own request at the cap: %d", c)
	}
	if c := from(h, "10.0.0.10", "POST", "/api/pair", pairBody("one-more", nonce)); c != 202 {
		t.Fatalf("another IP under the cap: %d", c)
	}
	// The global cap still holds across many IPs.
	for i := pairMaxIP + 1; i < pairMaxOpen; i++ {
		if c := from(h, fmt.Sprintf("10.1.0.%d", i), "POST", "/api/pair", pairBody(fmt.Sprintf("g%d", i), nonce)); c != 202 {
			t.Fatalf("global fill %d: %d", i, c)
		}
	}
	if c := from(h, "10.2.0.1", "POST", "/api/pair", pairBody("last", nonce)); c != 429 {
		t.Fatalf("over the global cap: %d", c)
	}
}

// A frame flashed as "Office" pairs and runs as "office" (foldIDs).
func TestPairMixedCaseID(t *testing.T) {
	_, ts := newTest(t, "admin-secret")
	const nonce = "0123456789abcdef0123"
	if r, _ := do(t, "POST", ts.URL+"/api/pair", "", map[string]string{"id": "Office", "nonce": nonce}); r.StatusCode != 202 {
		t.Fatalf("request: %d", r.StatusCode)
	}
	if code := accept(t, ts.URL, "Office", nonce); code != 200 {
		t.Fatalf("accept: %d", code)
	}
	r, body := do(t, "GET", ts.URL+"/api/pair/Office?nonce="+nonce, "", nil)
	var got struct{ Token string }
	_ = json.Unmarshal(body, &got)
	if r.StatusCode != 200 || len(got.Token) != 64 {
		t.Fatalf("pickup: %d %s", r.StatusCode, body)
	}
	for _, p := range []string{"/api/frames/Office/config", "/api/frames/office/config"} {
		if r, _ := do(t, "GET", ts.URL+p, got.Token, nil); r.StatusCode != 200 {
			t.Fatalf("%s: %d", p, r.StatusCode)
		}
	}
	if r, _ := do(t, "GET", ts.URL+"/api/frames/office", "admin-secret", nil); r.StatusCode != 200 {
		t.Fatalf("stored as office: %d", r.StatusCode)
	}
}
