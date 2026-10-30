package main

import (
	"bytes"
	"net"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

func TestHealthcheckReportsServerState(t *testing.T) {
	healthy := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/healthz" {
			http.NotFound(w, r)
			return
		}
		_, _ = w.Write([]byte("ok"))
	}))
	defer healthy.Close()
	broken := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		http.Error(w, "down", http.StatusServiceUnavailable)
	}))
	defer broken.Close()
	hung := httptest.NewServer(http.HandlerFunc(func(_ http.ResponseWriter, r *http.Request) {
		<-r.Context().Done()
	}))
	defer hung.Close()
	closed, _ := net.Listen("tcp", "127.0.0.1:0")
	closedPort := portOf(t, "http://"+closed.Addr().String())
	_ = closed.Close()

	for _, tc := range []struct {
		name, port, want string
		code             int
	}{
		{"healthy", portOf(t, healthy.URL), "healthy", 0},
		{"error status", portOf(t, broken.URL), "answered 503", 1},
		{"nothing listening", closedPort, "unhealthy", 1},
	} {
		var out bytes.Buffer
		if code := healthcheck(tc.port, &out); code != tc.code || !strings.Contains(out.String(), tc.want) {
			t.Errorf("%s: healthcheck = %d %q; want %d %q", tc.name, code, out.String(), tc.code, tc.want)
		}
	}
	if err := probe(hung.URL+"/healthz", 50*time.Millisecond); err == nil {
		t.Error("hung server: probe succeeded")
	}
}

func TestListenPortDefaultsToDiscoveryPort(t *testing.T) {
	t.Setenv("PORT", "")
	if p := listenPort(); p != "41007" {
		t.Fatalf("listenPort() = %q", p)
	}
	t.Setenv("PORT", "8080")
	if p := listenPort(); p != "8080" {
		t.Fatalf("listenPort() = %q", p)
	}
}

func portOf(t *testing.T, url string) string {
	t.Helper()
	_, port, err := net.SplitHostPort(strings.TrimPrefix(url, "http://"))
	if err != nil {
		t.Fatal(err)
	}
	return port
}
