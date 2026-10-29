package api

import (
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"strings"
	"testing"
	"time"
)

// pairAsk opens a pairing request and returns the code the frame would show.
func pairAsk(t *testing.T, base, id, nonce string) string {
	t.Helper()
	r, body := do(t, "POST", base+"/api/pair", "", map[string]string{"id": id, "nonce": nonce})
	var got struct{ Code string }
	_ = json.Unmarshal(body, &got)
	if r.StatusCode != 202 || len(got.Code) != 6 {
		t.Fatalf("request %s: %d %s", id, r.StatusCode, body)
	}
	return got.Code
}

// adminPair types code for id on the admin page and returns the error the page shows ("" = ok).
func adminPair(t *testing.T, base, pw, id, code string) (int, string) {
	t.Helper()
	req, _ := http.NewRequest("POST", base+"/admin/"+id+"/pair", strings.NewReader(url.Values{"code": {code}}.Encode()))
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	if pw != "" {
		req.SetBasicAuth("admin", pw)
	}
	resp, err := (&http.Client{CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}).Do(req)
	if err != nil {
		t.Fatal(err)
	}
	resp.Body.Close()
	loc, _ := url.Parse(resp.Header.Get("Location"))
	if loc == nil {
		return resp.StatusCode, ""
	}
	return resp.StatusCode, loc.Query().Get("err")
}

func poll(t *testing.T, base, id, nonce string) (int, string) {
	t.Helper()
	r, body := do(t, "GET", base+"/api/pair/"+id+"?nonce="+nonce, "", nil)
	var got struct{ Token string }
	_ = json.Unmarshal(body, &got)
	return r.StatusCode, got.Token
}

func TestPairWithCode(t *testing.T) {
	s, ts := newTest(t, "admin-secret")
	const nonce = "0123456789abcdef0123"
	code := pairAsk(t, ts.URL, "kitchen", nonce)
	if c, _ := poll(t, ts.URL, "kitchen", nonce); c != 202 {
		t.Fatalf("poll before the code: %d", c)
	}
	if _, err := s.Store.Get("kitchen"); err == nil {
		t.Fatal("a pairing request wrote a frame record before the pickup")
	}
	if c, _ := poll(t, ts.URL, "kitchen", "wrong-nonce-0000000"); c != 404 {
		t.Fatalf("poll with a wrong nonce: %d", c)
	}
	wrong := "000000"
	if code == wrong {
		wrong = "111111"
	}
	if _, e := adminPair(t, ts.URL, "admin-secret", "kitchen", wrong); !strings.Contains(e, "wrong code") {
		t.Fatalf("wrong code: %q", e)
	}
	if c, _ := adminPair(t, ts.URL, "", "kitchen", code); c != 401 {
		t.Fatalf("accept without admin credentials: %d", c)
	}
	if c, e := adminPair(t, ts.URL, "admin-secret", "kitchen", code[:3]+" "+code[3:]); c != 303 || e != "" {
		t.Fatalf("accept: %d %q", c, e)
	}
	c, tok := poll(t, ts.URL, "kitchen", nonce)
	if c != 200 || len(tok) != 64 {
		t.Fatalf("pickup: %d %q", c, tok)
	}
	if c, _ := poll(t, ts.URL, "kitchen", nonce); c != 404 {
		t.Fatalf("token handed out twice: %d", c)
	}
	f, err := s.Store.Get("kitchen")
	if err != nil || f.TokenHash != hashHex(tok) || f.PairedAt == nil {
		t.Fatalf("record after pickup: %v %+v", err, f)
	}
	// The token works for this frame's own routes and nothing else.
	if r, _ := do(t, "GET", ts.URL+"/api/frames/kitchen/config", tok, nil); r.StatusCode != 200 {
		t.Fatalf("own config: %d", r.StatusCode)
	}
	if r, _ := do(t, "GET", ts.URL+"/api/release", tok, nil); r.StatusCode != 200 {
		t.Fatalf("release: %d", r.StatusCode)
	}
	for _, c := range []struct{ m, p string }{{"GET", "/api/frames/other/config"}, {"GET", "/api/frames"}, {"PUT", "/api/frames/kitchen/layout"}, {"DELETE", "/api/frames/kitchen"}} {
		if r, _ := do(t, c.m, ts.URL+c.p, tok, nil); r.StatusCode != 401 {
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
	if r, _ := do(t, "GET", ts.URL+"/api/frames/kitchen/config", tok, nil); r.StatusCode != 401 {
		t.Fatalf("token after unpair: %d", r.StatusCode)
	}
	pairAsk(t, ts.URL, "kitchen", "fedcba9876543210fedc")
}

// The finding this file fixes: a LAN host on its own (request, the old button, poll) never gets
// a token, and while it squats an id the real frame still pairs with the code on its screen.
func TestPairNeedsTheCodeFromTheFrame(t *testing.T) {
	s, _ := newTest(t, "admin-secret")
	h := s.Routes()
	const frame, attacker = "192.168.1.50", "192.168.1.66"
	const fNonce, aNonce = "frame-nonce-0123456789", "attacker-nonce-012345"
	// The attacker asks first, presses the old PAIR route itself and polls: nothing.
	if c, _ := from(h, attacker, "POST", "/api/pair", pairBody("kitchen", aNonce)); c != 202 {
		t.Fatalf("attacker request: %d", c)
	}
	if c, _ := from(h, attacker, "POST", "/pair/kitchen/accept", "nonce="+aNonce); c != 200 {
		t.Fatalf("attacker's own code page: %d", c)
	}
	for range 3 {
		if c, _ := from(h, attacker, "GET", "/api/pair/kitchen?nonce="+aNonce, ""); c != 202 {
			t.Fatalf("attacker poll: %d", c)
		}
	}
	// It cannot type a code either: that needs the admin password.
	if c, _ := from(h, attacker, "POST", "/admin/kitchen/pair", "code=123456"); c != 401 {
		t.Fatalf("attacker typing a code: %d", c)
	}
	if _, err := s.Store.Get("kitchen"); err == nil {
		t.Fatal("unaccepted requests wrote a frame record")
	}
	// The real frame can still ask (no 409 while the squatter's request is open)...
	c, body := from(h, frame, "POST", "/api/pair", pairBody("kitchen", fNonce))
	var got struct{ Code string }
	_ = json.Unmarshal([]byte(body), &got)
	if c != 202 || len(got.Code) != 6 {
		t.Fatalf("frame request beside a squatter: %d %s", c, body)
	}
	// ...the owner types the code on its screen, and only the frame gets the token.
	if err := s.AcceptPairing("kitchen", got.Code); err != nil {
		t.Fatal(err)
	}
	if c, _ := from(h, attacker, "GET", "/api/pair/kitchen?nonce="+aNonce, ""); c != 404 {
		t.Fatalf("squatter after the frame was accepted: %d", c)
	}
	if c, _ := from(h, attacker, "GET", "/api/pair/kitchen?nonce="+fNonce, ""); c != 404 {
		t.Fatalf("frame's nonce from another IP: %d", c)
	}
	c, body = from(h, frame, "GET", "/api/pair/kitchen?nonce="+fNonce, "")
	var tok struct{ Token string }
	_ = json.Unmarshal([]byte(body), &tok)
	if c != 200 || len(tok.Token) != 64 {
		t.Fatalf("frame pickup: %d %s", c, body)
	}
}

func TestPairWrongCodesDropTheRequest(t *testing.T) {
	s, ts := newTest(t, "")
	const nonce = "0123456789abcdef0123"
	code := pairAsk(t, ts.URL, "hall", nonce)
	wrong := fmt.Sprintf("%06d", (atoi(code)+1)%1_000_000)
	for i := 1; i < pairMaxTries; i++ {
		if err := s.AcceptPairing("hall", wrong); err == nil || !strings.Contains(err.Error(), "tries left") {
			t.Fatalf("wrong code %d: %v", i, err)
		}
	}
	if err := s.AcceptPairing("hall", wrong); err == nil || !strings.Contains(err.Error(), "new one") {
		t.Fatalf("last wrong code: %v", err)
	}
	if err := s.AcceptPairing("hall", code); err == nil {
		t.Fatal("the right code after too many wrong ones was accepted")
	}
	if c, _ := poll(t, ts.URL, "hall", nonce); c != 404 {
		t.Fatalf("poll after the request was dropped: %d", c)
	}
	// The frame asks again and gets a fresh code, which works.
	code = pairAsk(t, ts.URL, "hall", nonce+"x")
	if err := s.AcceptPairing("hall", code); err != nil {
		t.Fatal(err)
	}
	if c, tok := poll(t, ts.URL, "hall", nonce+"x"); c != 200 || tok == "" {
		t.Fatalf("pickup: %d", c)
	}
	if err := s.AcceptPairing("nobody", "123456"); err == nil || !strings.Contains(err.Error(), "waiting") {
		t.Fatalf("no request: %v", err)
	}
}

func atoi(s string) int { var n int; _, _ = fmt.Sscanf(s, "%d", &n); return n }

func TestPairRejectsBadInputAndExpires(t *testing.T) {
	s, ts := newTest(t, "")
	for _, in := range []map[string]string{{"id": "kitchen", "nonce": "short"}, {"id": "../x", "nonce": "0123456789abcdef"}, {}} {
		if r, _ := do(t, "POST", ts.URL+"/api/pair", "", in); r.StatusCode != 400 {
			t.Fatalf("%v: %d", in, r.StatusCode)
		}
	}
	const nonce = "0123456789abcdef"
	code := pairAsk(t, ts.URL, "hall", nonce)
	s.mu.Lock()
	s.pair.open[hashHex(nonce)].created = time.Now().Add(-pairTTL - time.Second)
	s.mu.Unlock()
	if err := s.AcceptPairing("hall", code); err == nil {
		t.Fatal("accepted an expired request")
	}
	if c, _ := poll(t, ts.URL, "hall", nonce); c != 404 {
		t.Fatalf("poll after expiry: %d", c)
	}
	// Abandoned requests leave nothing on disk.
	if len(s.Store.List()) != 0 {
		t.Fatalf("frames after expired requests: %d", len(s.Store.List()))
	}
	if ents, _ := os.ReadDir(s.Store.Dir()); len(ents) != 0 {
		t.Fatalf("files in the store: %d", len(ents))
	}
	r, body := do(t, "GET", ts.URL+"/admin", "", nil)
	if r.StatusCode != 200 || strings.Contains(string(body), "forget pairing") {
		t.Fatalf("admin shows an unpaired frame as paired")
	}
}

// The admin page lists waiting frames, never their codes.
func TestAdminListsWaitingFramesWithoutCodes(t *testing.T) {
	_, ts := newTest(t, "")
	code := pairAsk(t, ts.URL, "porch", "0123456789abcdef0123")
	_, body := do(t, "GET", ts.URL+"/admin", "", nil)
	if !strings.Contains(string(body), `action="/admin/porch/pair"`) || strings.Contains(string(body), code) {
		t.Fatalf("admin page: waiting form missing or code shown")
	}
}

// from sends one pairing call to h as if from ip. JSON body for /api/pair, form otherwise.
func from(h http.Handler, ip, method, path, body string) (int, string) {
	req := httptest.NewRequest(method, path, strings.NewReader(body))
	req.Host = "frames.local"
	req.RemoteAddr = ip + ":40000"
	if !strings.HasPrefix(path, "/api/pair") {
		req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	}
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, req)
	return rec.Code, rec.Body.String()
}

func pairBody(id, nonce string) string { return fmt.Sprintf(`{"id":%q,"nonce":%q}`, id, nonce) }

func codeOf(t *testing.T, body string) string {
	t.Helper()
	var got struct{ Code string }
	if err := json.Unmarshal([]byte(body), &got); err != nil || len(got.Code) != 6 {
		t.Fatalf("no code in %s", body)
	}
	return got.Code
}

// Another LAN host can neither collect nor replace a frame's request.
func TestPairBoundToSourceIP(t *testing.T) {
	s, _ := newTest(t, "")
	h := s.Routes()
	const frame, other = "192.168.1.50", "192.168.1.66"
	const nonce, nonce2 = "0123456789abcdef0123", "fedcba9876543210fedc"
	c, body := from(h, frame, "POST", "/api/pair", pairBody("kitchen", nonce))
	if c != 202 {
		t.Fatalf("request: %d", c)
	}
	first := codeOf(t, body)
	// Another IP opens its own request beside it; the frame's stays.
	if c, _ := from(h, other, "POST", "/api/pair", pairBody("kitchen", nonce2)); c != 202 {
		t.Fatalf("other IP: %d", c)
	}
	if c, _ := from(h, other, "POST", "/pair/kitchen/accept", "nonce="+nonce); c != 404 {
		t.Fatalf("code page for the frame's nonce from another IP: %d", c)
	}
	// A rebooted frame (same IP, new nonce) replaces its own request; the old nonce is dead.
	const nonce3 = "abcdefabcdef0123456789"
	c, body = from(h, frame, "POST", "/api/pair", pairBody("kitchen", nonce3))
	if c != 202 {
		t.Fatalf("same IP replace: %d", c)
	}
	code := codeOf(t, body)
	if c, _ := from(h, frame, "GET", "/api/pair/kitchen?nonce="+nonce, ""); c != 404 {
		t.Fatalf("poll with the replaced nonce: %d", c)
	}
	if first != code {
		if err := s.AcceptPairing("kitchen", first); err == nil {
			t.Fatal("the replaced request's code still pairs")
		}
	}
	if err := s.AcceptPairing("kitchen", code); err != nil {
		t.Fatal(err)
	}
	if c, _ := from(h, other, "GET", "/api/pair/kitchen?nonce="+nonce3, ""); c != 404 {
		t.Fatalf("token handed to another IP: %d", c)
	}
	if c, _ := from(h, frame, "GET", "/api/pair/kitchen?nonce="+nonce3, ""); c != 200 {
		t.Fatalf("pickup from the frame: %d", c)
	}
}

func TestPairCapPerIP(t *testing.T) {
	s, _ := newTest(t, "")
	h := s.Routes()
	const nonce = "0123456789abcdef0123"
	for i := range pairMaxIP {
		if c, _ := from(h, "10.0.0.9", "POST", "/api/pair", pairBody(fmt.Sprintf("f%d", i), fmt.Sprintf("%s-%d", nonce, i))); c != 202 {
			t.Fatalf("request %d: %d", i, c)
		}
	}
	if c, _ := from(h, "10.0.0.9", "POST", "/api/pair", pairBody("one-more", nonce+"-x")); c != 429 {
		t.Fatalf("over the per-IP cap: %d", c)
	}
	if c, _ := from(h, "10.0.0.9", "POST", "/api/pair", pairBody("f0", "fedcba9876543210fedc")); c != 202 {
		t.Fatalf("replacing its own request at the cap: %d", c)
	}
	if c, _ := from(h, "10.0.0.10", "POST", "/api/pair", pairBody("one-more", nonce+"-y")); c != 202 {
		t.Fatalf("another IP under the cap: %d", c)
	}
	// The global cap still holds across many IPs.
	for i := pairMaxIP + 1; i < pairMaxOpen; i++ {
		if c, _ := from(h, fmt.Sprintf("10.1.0.%d", i), "POST", "/api/pair", pairBody(fmt.Sprintf("g%d", i), fmt.Sprintf("%s-g%d", nonce, i))); c != 202 {
			t.Fatalf("global fill %d: %d", i, c)
		}
	}
	if c, _ := from(h, "10.2.0.1", "POST", "/api/pair", pairBody("last", nonce+"-z")); c != 429 {
		t.Fatalf("over the global cap: %d", c)
	}
	if len(s.Store.List()) != 0 {
		t.Fatal("open requests wrote frame records")
	}
}

// A tfctl from before pairing codes shows a PAIR button that posts its nonce: the answer is
// the code, on the frame, for the holder of the nonce only.
func TestOldPairButtonShowsTheCode(t *testing.T) {
	s, _ := newTest(t, "")
	h := s.Routes()
	const ip, nonce = "192.168.1.50", "0123456789abcdef0123"
	_, body := from(h, ip, "POST", "/api/pair", pairBody("kitchen", nonce))
	code := codeOf(t, body)
	c, page := from(h, ip, "POST", "/pair/kitchen/accept", "nonce="+nonce)
	if c != 200 || !strings.Contains(page, code[:3]+" "+code[3:]) {
		t.Fatalf("code page: %d %s", c, page)
	}
	if c, page := from(h, ip, "POST", "/pair/kitchen/accept", "nonce=wrong-nonce-0123456"); c != 404 || strings.Contains(page, code[:3]) {
		t.Fatalf("code page with a wrong nonce: %d", c)
	}
	if c, _ := from(h, ip, "GET", "/api/pair/kitchen?nonce="+nonce, ""); c != 202 {
		t.Fatalf("the old button paired something: %d", c)
	}
}

// A frame flashed as "Office" pairs and runs as "office" (foldIDs).
func TestPairMixedCaseID(t *testing.T) {
	s, ts := newTest(t, "admin-secret")
	const nonce = "0123456789abcdef0123"
	code := pairAsk(t, ts.URL, "Office", nonce)
	if err := s.AcceptPairing("Office", code); err != nil {
		t.Fatal(err)
	}
	c, tok := poll(t, ts.URL, "Office", nonce)
	if c != 200 || len(tok) != 64 {
		t.Fatalf("pickup: %d", c)
	}
	for _, p := range []string{"/api/frames/Office/config", "/api/frames/office/config"} {
		if r, _ := do(t, "GET", ts.URL+p, tok, nil); r.StatusCode != 200 {
			t.Fatalf("%s: %d", p, r.StatusCode)
		}
	}
	if r, _ := do(t, "GET", ts.URL+"/api/frames/office", "admin-secret", nil); r.StatusCode != 200 {
		t.Fatalf("stored as office: %d", r.StatusCode)
	}
}
