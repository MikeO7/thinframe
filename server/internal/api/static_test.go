package api

import (
	"compress/gzip"
	"io"
	"net/http"
	"regexp"
	"strings"
	"testing"

	"github.com/MikeO7/thinframe/server/internal/widgets"
)

// raw GETs url with the headers as given: no transparent gzip from the client.
func raw(t *testing.T, url string, hdr map[string]string) (*http.Response, []byte) {
	t.Helper()
	req, _ := http.NewRequest("GET", url, nil)
	for k, v := range hdr {
		req.Header.Set(k, v)
	}
	tr := &http.Transport{DisableCompression: true}
	defer tr.CloseIdleConnections()
	resp, err := (&http.Client{Transport: tr}).Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	b, _ := io.ReadAll(resp.Body)
	return resp, b
}

func gunzip(t *testing.T, b []byte) []byte {
	t.Helper()
	zr, err := gzip.NewReader(strings.NewReader(string(b)))
	if err != nil {
		t.Fatal(err)
	}
	out, err := io.ReadAll(zr)
	if err != nil {
		t.Fatal(err)
	}
	return out
}

var linkRe = regexp.MustCompile(`<link rel="stylesheet" href="/static/(page-[0-9a-f]{12}\.css)">`)
var scriptRe = regexp.MustCompile(`<script src="/static/(page-[0-9a-f]{12}\.js)"></script>`)

// The browser gets a small page that links its CSS and JS at hashed names, cached for good;
// tfctl, saving the page for a boot with no server, gets them inline.
func TestPageLinksStaticAssets(t *testing.T) {
	_, ts := newTest(t, "")
	resp, page := raw(t, ts.URL+"/frame/office", nil)
	css, js := linkRe.FindSubmatch(page), scriptRe.FindSubmatch(page)
	if resp.StatusCode != 200 || css == nil || js == nil || strings.Contains(string(page), "<style>") {
		t.Fatalf("page does not link its assets: %d\n%.600s", resp.StatusCode, page)
	}
	if len(page) > 20000 {
		t.Errorf("linked page is %d bytes", len(page))
	}
	if resp.Header.Get("Cache-Control") != "no-store" {
		t.Errorf("page Cache-Control %q", resp.Header.Get("Cache-Control"))
	}
	for _, name := range []string{string(css[1]), string(js[1])} {
		r, b := raw(t, ts.URL+"/static/"+name, nil)
		want, ct, _ := widgets.Asset(name)
		if r.StatusCode != 200 || string(b) != string(want) || r.Header.Get("Content-Type") != ct {
			t.Fatalf("%s: %d %q", name, r.StatusCode, r.Header.Get("Content-Type"))
		}
		if r.Header.Get("Cache-Control") != "public, max-age=31536000, immutable" {
			t.Errorf("%s: Cache-Control %q", name, r.Header.Get("Cache-Control"))
		}
	}
	if r, _ := raw(t, ts.URL+"/static/page-000000000000.css", nil); r.StatusCode != 404 {
		t.Errorf("unknown asset: %d", r.StatusCode)
	}

	for _, hdr := range []map[string]string{{"User-Agent": "tfctl"}, nil} {
		u := ts.URL + "/frame/office"
		if hdr == nil {
			u += "?inline=1"
		}
		_, saved := raw(t, u, hdr)
		if !strings.Contains(string(saved), "<style>") || linkRe.Match(saved) || scriptRe.Match(saved) ||
			!strings.Contains(string(saved), "function paintClock") {
			t.Errorf("inline page (%v) lacks its CSS or JS", hdr)
		}
	}
}

func TestGzip(t *testing.T) {
	_, ts := newTest(t, "")
	_, plain := raw(t, ts.URL+"/frame/office", nil)
	resp, z := raw(t, ts.URL+"/frame/office", map[string]string{"Accept-Encoding": "gzip, deflate"})
	if resp.Header.Get("Content-Encoding") != "gzip" || !strings.Contains(resp.Header.Get("Vary"), "Accept-Encoding") {
		t.Fatalf("page not gzipped: %v", resp.Header)
	}
	if got := gunzip(t, z); string(got) != string(plain) || len(z) >= len(plain) {
		t.Fatalf("gzip body: %d -> %d bytes, equal %v", len(plain), len(z), string(got) == string(plain))
	}
	name := widgets.ScriptName()
	resp, z = raw(t, ts.URL+"/static/"+name, map[string]string{"Accept-Encoding": "gzip"})
	want, _, _ := widgets.Asset(name)
	if resp.Header.Get("Content-Encoding") != "gzip" || string(gunzip(t, z)) != string(want) {
		t.Fatalf("script not gzipped: %v", resp.Header)
	}
	// Refused, absent, a 404 and a PNG: plain.
	for _, c := range []struct{ path, ae string }{
		{"/frame/office", "gzip;q=0"}, {"/frame/office", ""}, {"/frame/office", "br"},
		{"/static/nope", "gzip"}, {"/fonts/manrope.woff2", "gzip"},
	} {
		if r, _ := raw(t, ts.URL+c.path, map[string]string{"Accept-Encoding": c.ae}); r.Header.Get("Content-Encoding") != "" {
			t.Errorf("%s with %q: Content-Encoding %q", c.path, c.ae, r.Header.Get("Content-Encoding"))
		}
	}
}

func TestAcceptsGzip(t *testing.T) {
	for h, want := range map[string]bool{"gzip": true, "deflate, gzip;q=1.0": true, "*": true, "GZIP": true,
		"gzip;q=0": false, "gzip; q=0.000": false, "gzip;q=0.5": true, "": false, "identity": false, "br, deflate": false} {
		if acceptsGzip(h) != want {
			t.Errorf("acceptsGzip(%q) = %v", h, !want)
		}
	}
}
