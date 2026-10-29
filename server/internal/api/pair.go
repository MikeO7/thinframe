package api

// Pairing with a code (TASK 16, docs/decisions.md "Pairing"). A frame without a token asks to
// pair (POST /api/pair with a random nonce, its request secret). The answer carries a 6-digit
// code that the frame shows on its own screen and nowhere else. The owner types that code on the
// admin page (or tells MCP pair_frame); only then does the server mint a per-frame token, which
// it hands once to the next poll that holds the request's nonce (GET /api/pair/{id}?nonce=).
//
// Anyone on the LAN can open a request, but they only learn the code of their own request; the
// code that gets typed in is the one on the real frame's screen, so the token goes to the real
// frame. A wrong code counts against every open request for that id, and pairMaxTries wrong
// codes drop them (the frame then asks again and shows a new code). Several requests for one id
// may be open at once (from different source IPs), so a squatter cannot keep the real frame
// from asking. A paired id refuses new requests until the admin forgets it. The frame record is
// written only when the frame picks its token up, so requests that are never accepted leave
// nothing on disk.

import (
	"crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"html/template"
	"io"
	"math/big"
	"net"
	"net/http"
	"sort"
	"strings"
	"time"

	"github.com/MikeO7/thinframe/server/internal/store"
)

const (
	pairTTL      = 15 * time.Minute
	pairMaxOpen  = 32
	pairMaxIP    = 4    // open requests from one source IP
	pairMaxTries = 5    // wrong codes for one id before its open requests are dropped
	maxPairForm  = 4096 // the old PAIR button's form: one nonce
)

type pairReq struct {
	id      string
	ip      string // source IP of the request; the poll must come from it
	code    string // 6 digits; only the requester (the frame's screen) ever sees it
	created time.Time
	tries   int    // wrong codes typed for this id while the request was open
	token   string // set once the code was entered, cleared when the frame picks it up
}

type pairings struct {
	open map[string]*pairReq // by SHA-256 of the request's nonce
}

// PairWaiting is an open pairing request as the admin page and MCP show it. The code is not in
// it: it must come from the frame's screen.
type PairWaiting struct {
	ID       string    `json:"id"`
	IP       string    `json:"ip"`
	Since    time.Time `json:"since"`
	Accepted bool      `json:"accepted"` // code entered, the frame has not picked its token up yet
}

func hashHex(s string) string { h := sha256.Sum256([]byte(s)); return hex.EncodeToString(h[:]) }

// sourceIP is the host part of r.RemoteAddr. Proxy headers are ignored on purpose.
func sourceIP(r *http.Request) string {
	if h, _, err := net.SplitHostPort(r.RemoteAddr); err == nil {
		return h
	}
	return r.RemoteAddr
}

func sameHash(a, b string) bool { return subtle.ConstantTimeCompare([]byte(a), []byte(b)) == 1 }

// prunePairs drops expired requests. Caller holds s.mu.
func (s *Server) prunePairs() {
	for k, p := range s.pair.open {
		if time.Since(p.created) > pairTTL {
			delete(s.pair.open, k)
		}
	}
}

// pairByNonce is the live request with this nonce for frame id from ip, or nil. Caller holds s.mu.
func (s *Server) pairByNonce(id, nonce, ip string) (string, *pairReq) {
	s.prunePairs()
	k := hashHex(nonce)
	p := s.pair.open[k]
	if p == nil || p.id != id || p.ip != ip {
		return "", nil
	}
	return k, p
}

// dropPairs forgets every open request for id. Caller holds s.mu.
func (s *Server) dropPairs(id string) {
	for k, p := range s.pair.open {
		if p.id == id {
			delete(s.pair.open, k)
		}
	}
}

// newPairCode is 6 random digits not used by another open request. Caller holds s.mu.
func (s *Server) newPairCode() (string, error) {
	for {
		n, err := rand.Int(rand.Reader, big.NewInt(1_000_000))
		if err != nil {
			return "", err
		}
		c := fmt.Sprintf("%06d", n.Int64())
		taken := false
		for _, p := range s.pair.open {
			taken = taken || p.code == c
		}
		if !taken {
			return c, nil
		}
	}
}

func (s *Server) paired(id string) bool {
	f, err := s.Store.Get(id)
	return err == nil && f.TokenHash != ""
}

// handlePairRequest: POST /api/pair {"id","nonce"}. 202 {"code"} = show the code and poll,
// 409 = paired already.
func (s *Server) handlePairRequest(w http.ResponseWriter, r *http.Request) {
	var in struct {
		ID    string `json:"id"`
		Nonce string `json:"nonce"`
	}
	err := json.NewDecoder(io.LimitReader(r.Body, 4096)).Decode(&in)
	in.ID = strings.ToLower(in.ID) // the same frame as in the path (foldIDs)
	if err != nil || !store.ValidID(in.ID) || len(in.Nonce) < 16 || len(in.Nonce) > 128 {
		http.Error(w, "need id and a nonce of 16..128 chars", http.StatusBadRequest)
		return
	}
	if s.paired(in.ID) {
		writeJSON(w, http.StatusConflict, map[string]any{"error": "already paired; forget it on the admin page to pair again"})
		return
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	s.prunePairs()
	ip, key := sourceIP(r), hashHex(in.Nonce)
	// A rebooted frame (same IP, same id, new nonce) replaces its own request; so does a repeat.
	for k, p := range s.pair.open {
		if k == key || p.ip == ip && p.id == in.ID {
			delete(s.pair.open, k)
		}
	}
	fromIP := 0
	for _, p := range s.pair.open {
		if p.ip == ip {
			fromIP++
		}
	}
	if len(s.pair.open) >= pairMaxOpen || fromIP >= pairMaxIP {
		http.Error(w, "too many open pairing requests", http.StatusTooManyRequests)
		return
	}
	code, err := s.newPairCode()
	if err != nil {
		http.Error(w, "no randomness", http.StatusInternalServerError)
		return
	}
	s.pair.open[key] = &pairReq{id: in.ID, ip: ip, code: code, created: time.Now()}
	writeJSON(w, http.StatusAccepted, map[string]any{"state": "waiting", "code": code,
		"expires_in": int(pairTTL / time.Second), "server_version": s.opts.Version})
}

// handlePairPoll: GET /api/pair/{id}?nonce=. 200 {"token"} exactly once, 202 waiting, 404 unknown,
// 409 the id was paired some other way meanwhile.
func (s *Server) handlePairPoll(w http.ResponseWriter, r *http.Request) {
	id, nonce := r.PathValue("id"), r.URL.Query().Get("nonce")
	s.mu.Lock()
	k, p := s.pairByNonce(id, nonce, sourceIP(r))
	switch {
	case p == nil:
		s.mu.Unlock()
		http.Error(w, "no such pairing request", http.StatusNotFound)
		return
	case p.token == "":
		s.mu.Unlock()
		writeJSON(w, http.StatusAccepted, map[string]any{"state": "waiting"})
		return
	}
	delete(s.pair.open, k)
	s.mu.Unlock()
	// The frame record is written now (Update creates it), not before: an abandoned request
	// leaves nothing behind.
	ok := false
	_, err := s.Store.Update(id, func(f *store.Frame) {
		if f.TokenHash != "" {
			return
		}
		now := time.Now().UTC()
		f.TokenHash, f.PairedAt, ok = hashHex(p.token), &now, true
		store.AddEvent(f, "pair", "paired with the code on its screen, from "+p.ip)
	})
	if err != nil {
		writeErr(w, err)
		return
	}
	if !ok {
		writeJSON(w, http.StatusConflict, map[string]any{"error": "already paired; forget it on the admin page to pair again"})
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"state": "paired", "token": p.token})
}

// normCode keeps the digits of what the owner typed ("482 913", "482-913").
func normCode(c string) string {
	var b strings.Builder
	for _, r := range c {
		if r >= '0' && r <= '9' {
			b.WriteRune(r)
		}
	}
	return b.String()
}

// AcceptPairing is the owner typing the code shown on frame id's screen (admin page, MCP). The
// frame picks its token up on its next poll, within a few seconds.
func (s *Server) AcceptPairing(id, code string) error {
	id, code = strings.ToLower(id), normCode(code)
	if !store.ValidID(id) {
		return store.ErrInvalidID
	}
	if s.paired(id) {
		return fmt.Errorf("%w: %s is paired already; forget its pairing first", ErrInvalid, id)
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	s.prunePairs()
	var match *pairReq
	n := 0
	for _, p := range s.pair.open {
		if p.id != id {
			continue
		}
		n++
		if len(code) == 6 && subtle.ConstantTimeCompare([]byte(p.code), []byte(code)) == 1 {
			match = p
		}
	}
	if n == 0 {
		return fmt.Errorf("%w: no frame %s is waiting to pair; it shows a code when it is", ErrInvalid, id)
	}
	if match == nil {
		left := pairMaxTries
		for k, p := range s.pair.open {
			if p.id == id {
				p.tries++
				left = min(left, pairMaxTries-p.tries)
				if p.tries >= pairMaxTries {
					delete(s.pair.open, k)
				}
			}
		}
		if left <= 0 {
			return fmt.Errorf("%w: wrong code too often; the frame will show a new one", ErrInvalid)
		}
		return fmt.Errorf("%w: wrong code for %s (%d tries left)", ErrInvalid, id, left)
	}
	if match.token == "" {
		b := make([]byte, 32)
		if _, err := rand.Read(b); err != nil {
			return err
		}
		match.token = hex.EncodeToString(b)
	}
	// Only the request whose code was typed in may collect a token.
	for k, p := range s.pair.open {
		if p.id == id && p != match {
			delete(s.pair.open, k)
		}
	}
	return nil
}

// PairsWaiting lists the open pairing requests, oldest first.
func (s *Server) PairsWaiting() []PairWaiting {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.prunePairs()
	out := []PairWaiting{}
	for _, p := range s.pair.open {
		out = append(out, PairWaiting{ID: p.id, IP: p.ip, Since: p.created, Accepted: p.token != ""})
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Since.Before(out[j].Since) })
	return out
}

func (s *Server) handleAdminPair(w http.ResponseWriter, r *http.Request) {
	s.adminRedirect(w, r, s.AcceptPairing(r.PathValue("id"), r.FormValue("code")))
}

var pairCodePage = template.Must(template.New("code").Parse(`<!doctype html><html><head><meta charset="utf-8"><title>thinframe</title>
<style>html,body{margin:0;height:100%;background:#f4dcaa;color:#5b3a1e;font:700 48px Georgia,serif;display:flex;align-items:center;justify-content:center;text-align:center}
p{margin:0}b{display:block;font-size:120px;letter-spacing:.1em;color:#b8452a;margin:.2em 0}small{display:block;font-size:30px}</style></head>
<body><p>{{.Title}}{{with .Code}}<b>{{.}}</b>{{end}}<small>{{.Detail}}</small></p></body></html>`))

// handlePairShowCode: POST /pair/{id}/accept (form nonce=), the PAIR button of a tfctl from
// before pairing codes. It no longer pairs anything: it shows the request's code on the frame,
// to the holder of the nonce only, so an older frame can still be paired from the admin page.
func (s *Server) handlePairShowCode(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	r.Body = http.MaxBytesReader(w, r.Body, maxPairForm)
	if err := r.ParseForm(); err != nil {
		code := http.StatusBadRequest
		if _, big := errors.AsType[*http.MaxBytesError](err); big {
			code = http.StatusRequestEntityTooLarge
		}
		http.Error(w, "bad pairing form", code)
		return
	}
	page := func(code int, title, pin, detail string) {
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		w.Header().Set("Cache-Control", "no-store")
		w.WriteHeader(code)
		_ = pairCodePage.Execute(w, map[string]string{"Title": title, "Code": pin, "Detail": detail})
	}
	s.mu.Lock()
	_, p := s.pairByNonce(id, r.FormValue("nonce"), sourceIP(r))
	var pin string
	if p != nil {
		pin = p.code[:3] + " " + p.code[3:]
	}
	s.mu.Unlock()
	if p == nil {
		page(http.StatusNotFound, "This request expired", "", "The frame will ask again in a moment.")
		return
	}
	page(http.StatusOK, "Pairing code", pin, "Enter it for "+id+" on the server's admin page.")
}

// Unpair forgets a frame's token; the frame asks to pair again (a new code) on its next connect.
func (s *Server) Unpair(id string) (store.Frame, error) {
	f, err := s.Store.Update(id, func(f *store.Frame) {
		f.TokenHash, f.PairedAt = "", nil
		store.AddEvent(f, "pair", "pairing forgotten")
	})
	if err == nil {
		s.Hub.Kick(id, "pairing forgotten")
	}
	return f, err
}

func (s *Server) handleAdminUnpair(w http.ResponseWriter, r *http.Request) {
	_, err := s.Unpair(r.PathValue("id"))
	s.adminRedirect(w, r, err)
}

// frameTokenOK: the bearer token is the paired token of frame id, or of any frame when id is "".
func (s *Server) frameTokenOK(r *http.Request, id string) bool {
	t, ok := bearer(r)
	if !ok {
		return false
	}
	h := hashHex(t)
	if id != "" {
		f, err := s.Store.Get(id)
		return err == nil && f.TokenHash != "" && sameHash(f.TokenHash, h)
	}
	for _, f := range s.Store.List() {
		if f.TokenHash != "" && sameHash(f.TokenHash, h) {
			return true
		}
	}
	return false
}
