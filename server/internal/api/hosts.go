package api

// Host allowlist and CORS for the public routes (docs/decisions.md "Public routes: Host and
// CORS"). The page plane, the apps' endpoints and pairing carry no credentials, so a web page
// on another site could read them by rebinding its own DNS name to this server's LAN address;
// the browser then sends that name as Host. The public routes answer only to names a LAN
// server is reached by. Token-protected routes skip the check: without the token there is
// nothing to read, and an MCP client behind an unusual name keeps working.

import (
	"log"
	"net"
	"net/http"
	"os"
	"strings"
)

// hostSet is the extra names (SERVER_HOSTS) plus the container's own hostname.
type hostSet map[string]bool

// localSuffixes are names that never resolve on the public internet.
var localSuffixes = []string{".local", ".lan", ".home.arpa", ".internal", ".localhost"}

func normHost(h string) string { return strings.TrimSuffix(strings.ToLower(strings.TrimSpace(h)), ".") }

func newHostSet(extra []string) hostSet {
	hs := hostSet{}
	for _, h := range extra {
		if h = normHost(h); h != "" {
			hs[h] = true
		}
	}
	if name, err := os.Hostname(); err == nil && name != "" {
		hs[normHost(name)] = true
	}
	return hs
}

// ParseHosts splits SERVER_HOSTS ("server.nox, frames.example.com").
func ParseHosts(v string) []string {
	var out []string
	for h := range strings.SplitSeq(v, ",") {
		if h = normHost(h); h != "" {
			out = append(out, h)
		}
	}
	return out
}

// allowed: an IP literal, localhost, a single-label name, a local suffix, or a listed name.
// An empty Host (HTTP/1.0) has no name to rebind.
func (hs hostSet) allowed(hostport string) bool {
	h := hostport
	if host, _, err := net.SplitHostPort(hostport); err == nil {
		h = host
	}
	h = normHost(strings.TrimSuffix(strings.TrimPrefix(h, "["), "]"))
	if h == "" || h == "localhost" || net.ParseIP(h) != nil || !strings.Contains(h, ".") || hs[h] {
		return true
	}
	for _, suf := range localSuffixes {
		if strings.HasSuffix(h, suf) {
			return true
		}
	}
	return false
}

// hostOK answers 421 and logs when the Host is not one of ours.
func (s *Server) hostOK(w http.ResponseWriter, r *http.Request) bool {
	if s.hosts.allowed(r.Host) {
		return true
	}
	log.Printf("host check: refused Host %q from %s for %s %s (add the name to SERVER_HOSTS if frames use it)",
		r.Host, sourceIP(r), r.Method, r.URL.Path)
	http.Error(w, "unknown Host: add this name to the server's SERVER_HOSTS", http.StatusMisdirectedRequest)
	return false
}

func (s *Server) checkHost(next http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if s.hostOK(w, r) {
			next(w, r)
		}
	}
}

// appCORS lets the documents that need a cross-origin read of a data, stream or state endpoint
// have it, and nobody else: a sandboxed app (opaque origin, Origin "null") and the frame's saved
// boot page (file://, "null" or "file://" depending on the engine). Same-origin requests need
// no header. Other sites get none, so a browser keeps their pages from reading the answer.
// Limit: a page on another site can still read through a sandboxed iframe of its own, which
// sends "null" too. Closing that needs a key on reads, as emit has (docs/decisions.md).
func appCORS(w http.ResponseWriter, r *http.Request) {
	w.Header().Add("Vary", "Origin")
	switch o := r.Header.Get("Origin"); o {
	case "null", "file://":
		w.Header().Set("Access-Control-Allow-Origin", o)
	}
}

// ParseTokens reads MCP_TOKENS: tokens separated by commas, spaces or newlines. Tokens shorter
// than 32 characters are refused, so a typo cannot open /mcp to a guessable key.
func ParseTokens(v string) (tokens []string, refused int) {
	for _, t := range strings.FieldsFunc(v, func(r rune) bool { return r == ',' || r == ' ' || r == '\n' || r == '\t' || r == '\r' }) {
		if len(t) < 32 {
			refused++
			continue
		}
		tokens = append(tokens, t)
	}
	return tokens, refused
}
