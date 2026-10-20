package artifacts

import (
	"crypto/sha256"
	"encoding/hex"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func setup(t *testing.T) (*Dir, http.Handler) {
	t.Helper()
	dir := t.TempDir()
	must := func(name, body string) {
		if err := os.WriteFile(filepath.Join(dir, name), []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	must("tfctl-armv7", "binary")
	must("versions.json", `{"software":"1.2","system":"2026.10.a","files":{"tfctl":"tfctl-armv7"}}`)
	must(".hidden", "x")
	d := New(dir)
	mux := http.NewServeMux()
	mux.Handle("GET /api/artifacts/{name}", d.Handler())
	return d, mux
}

func get(h http.Handler, path string) *httptest.ResponseRecorder {
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, httptest.NewRequest("GET", path, nil))
	return rec
}

func TestVersions(t *testing.T) {
	d, _ := setup(t)
	v := d.Versions()
	if v.Software != "1.2" || v.System != "2026.10.a" || v.Files["tfctl"] != "tfctl-armv7" {
		t.Fatalf("versions: %+v", v)
	}
	if got := New(filepath.Join(t.TempDir(), "missing")).Versions(); got.Software != "" {
		t.Fatalf("missing dir should give zero value, got %+v", got)
	}
}

func TestSumAndSidecar(t *testing.T) {
	d, h := setup(t)
	want := sha256.Sum256([]byte("binary"))
	wantHex := hex.EncodeToString(want[:])
	got, err := d.Sum("tfctl-armv7")
	if err != nil || got != wantHex {
		t.Fatalf("sum = %q, %v", got, err)
	}
	rec := get(h, "/api/artifacts/tfctl-armv7.sha256")
	if rec.Code != 200 || strings.TrimSpace(rec.Body.String()) != wantHex+"  tfctl-armv7" {
		t.Fatalf("sidecar: %d %q", rec.Code, rec.Body.String())
	}
}

func TestServeAndRefuse(t *testing.T) {
	_, h := setup(t)
	if rec := get(h, "/api/artifacts/tfctl-armv7"); rec.Code != 200 || rec.Body.String() != "binary" {
		t.Fatalf("serve: %d %q", rec.Code, rec.Body.String())
	}
	for _, p := range []string{"/api/artifacts/nope", "/api/artifacts/.hidden", "/api/artifacts/nope.sha256", "/api/artifacts/..%2Fetc%2Fpasswd"} {
		if rec := get(h, p); rec.Code != 404 {
			t.Errorf("%s: code %d, want 404", p, rec.Code)
		}
	}
}
