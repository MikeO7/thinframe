package api

import (
	"encoding/json"
	"net/http"
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
	resp, err := http.PostForm(ts.URL+"/admin/kitchen/unpair", nil)
	if err != nil {
		t.Fatal(err)
	}
	resp.Body.Close()
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
