package api

import (
	"bytes"
	"log"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"testing"
)

// The container HEALTHCHECK probes /healthz every 30s; only failed probes reach the log.
func TestLogRequestsSkipsSuccessfulHealthz(t *testing.T) {
	var buf bytes.Buffer
	log.SetOutput(&buf)
	t.Cleanup(func() { log.SetOutput(os.Stderr) })
	status := http.StatusOK
	h := logRequests(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(status) }))

	h.ServeHTTP(httptest.NewRecorder(), httptest.NewRequest("GET", "/healthz", nil))
	if buf.Len() != 0 {
		t.Fatalf("logged a healthy probe: %q", buf.String())
	}
	status = http.StatusServiceUnavailable
	h.ServeHTTP(httptest.NewRecorder(), httptest.NewRequest("GET", "/healthz", nil))
	status = http.StatusOK
	h.ServeHTTP(httptest.NewRecorder(), httptest.NewRequest("GET", "/admin", nil))
	if out := buf.String(); !strings.Contains(out, "GET /healthz 503") || !strings.Contains(out, "GET /admin 200") {
		t.Fatalf("log = %q", out)
	}
}
