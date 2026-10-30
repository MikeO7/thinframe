package api

import (
	"compress/gzip"
	"net/http"
	"strings"
	"sync"
)

// gzipped compresses a handler's response when the client accepts gzip and the response is
// text, JSON, JavaScript or a TTF font with status 200. The dashboard page is about 100 KB of
// HTML, CSS and JS; gzip takes it to about a fifth. tfctl sends no Accept-Encoding, so what
// it fetches (the saved page, config) stays plain. Not for the event stream or the sockets.
func gzipped(next http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		w.Header().Add("Vary", "Accept-Encoding")
		if r.Method == http.MethodHead || !acceptsGzip(r.Header.Get("Accept-Encoding")) {
			next(w, r)
			return
		}
		g := &gzipWriter{ResponseWriter: w}
		defer g.close()
		next(g, r)
	}
}

// acceptsGzip reads an Accept-Encoding header: gzip (or *) listed and not with q=0.
func acceptsGzip(h string) bool {
	for _, part := range strings.Split(h, ",") {
		coding, params, _ := strings.Cut(strings.TrimSpace(part), ";")
		if c := strings.ToLower(strings.TrimSpace(coding)); c != "gzip" && c != "*" {
			continue
		}
		q := strings.ReplaceAll(strings.ToLower(params), " ", "")
		if q == "q=0" || strings.HasPrefix(q, "q=0.") && strings.Trim(q[4:], "0") == "" {
			return false
		}
		return true
	}
	return false
}

func compressible(ct string) bool {
	ct, _, _ = strings.Cut(strings.ToLower(ct), ";")
	switch strings.TrimSpace(ct) {
	case "text/html", "text/css", "text/javascript", "application/javascript", "application/json",
		"text/plain", "image/svg+xml", "font/ttf":
		return true
	}
	return false
}

var gzipPool = sync.Pool{New: func() any { return gzip.NewWriter(nil) }}

// gzipWriter decides on the first WriteHeader or Write whether to compress, from the status
// and the Content-Type the handler set.
type gzipWriter struct {
	http.ResponseWriter
	gz      *gzip.Writer
	decided bool
}

func (g *gzipWriter) WriteHeader(code int) {
	if !g.decided {
		g.decided = true
		h := g.Header()
		if code == http.StatusOK && h.Get("Content-Encoding") == "" && compressible(h.Get("Content-Type")) {
			h.Set("Content-Encoding", "gzip")
			h.Del("Content-Length")
			g.gz = gzipPool.Get().(*gzip.Writer)
			g.gz.Reset(g.ResponseWriter)
		}
	}
	g.ResponseWriter.WriteHeader(code)
}

func (g *gzipWriter) Write(b []byte) (int, error) {
	if !g.decided {
		if g.Header().Get("Content-Type") == "" {
			g.Header().Set("Content-Type", http.DetectContentType(b))
		}
		g.WriteHeader(http.StatusOK)
	}
	if g.gz != nil {
		return g.gz.Write(b)
	}
	return g.ResponseWriter.Write(b)
}

func (g *gzipWriter) Flush() {
	if g.gz != nil {
		_ = g.gz.Flush()
	}
	_ = http.NewResponseController(g.ResponseWriter).Flush()
}

func (g *gzipWriter) Unwrap() http.ResponseWriter { return g.ResponseWriter }

func (g *gzipWriter) close() {
	if g.gz == nil {
		return
	}
	_ = g.gz.Close()
	g.gz.Reset(nil)
	gzipPool.Put(g.gz)
	g.gz = nil
}
