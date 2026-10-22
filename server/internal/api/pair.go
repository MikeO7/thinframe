package api

// One-press pairing (TASK 16). A frame without a token asks to pair (POST /api/pair with a random
// nonce only it knows) and shows a local page with a big PAIR button. The button posts the nonce
// back from the frame's own browser (POST /pair/{id}/accept), which proves someone is standing at
// the frame. The server then mints a per-frame token, stores only its hash, and hands it over once
// to the frame's next poll (GET /api/pair/{id}?nonce=). A paired id refuses new requests until the
// admin page forgets it, so pairing happens once.

import (
	"crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/hex"
	"encoding/json"
	"html/template"
	"io"
	"net/http"
	"time"

	"github.com/MikeO7/thinframe/server/internal/store"
)

const (
	pairTTL     = 15 * time.Minute
	pairMaxOpen = 32
)

type pairReq struct {
	nonceHash string
	created   time.Time
	token     string // set once accepted, cleared when the frame picks it up
}

type pairings struct {
	open map[string]*pairReq // by frame id
}

func hashHex(s string) string { h := sha256.Sum256([]byte(s)); return hex.EncodeToString(h[:]) }

func sameHash(a, b string) bool { return subtle.ConstantTimeCompare([]byte(a), []byte(b)) == 1 }

// pending returns the live request for id; expired ones are dropped. Caller holds s.mu.
func (s *Server) pending(id string) *pairReq {
	p := s.pair.open[id]
	if p != nil && time.Since(p.created) > pairTTL {
		delete(s.pair.open, id)
		return nil
	}
	return p
}

// handlePairRequest: POST /api/pair {"id","nonce"}. 202 = waiting for the button, 409 = paired already.
func (s *Server) handlePairRequest(w http.ResponseWriter, r *http.Request) {
	var in struct {
		ID    string `json:"id"`
		Nonce string `json:"nonce"`
	}
	if err := json.NewDecoder(io.LimitReader(r.Body, 4096)).Decode(&in); err != nil || !store.ValidID(in.ID) || len(in.Nonce) < 16 || len(in.Nonce) > 128 {
		http.Error(w, "need id and a nonce of 16..128 chars", http.StatusBadRequest)
		return
	}
	if f, err := s.Store.Get(in.ID); err == nil && f.TokenHash != "" {
		writeJSON(w, http.StatusConflict, map[string]any{"error": "already paired; forget it on the admin page to pair again"})
		return
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	for id := range s.pair.open {
		s.pending(id) // drop expired
	}
	if s.pair.open[in.ID] == nil && len(s.pair.open) >= pairMaxOpen {
		http.Error(w, "too many open pairing requests", http.StatusTooManyRequests)
		return
	}
	s.pair.open[in.ID] = &pairReq{nonceHash: hashHex(in.Nonce), created: time.Now()}
	writeJSON(w, http.StatusAccepted, map[string]any{"state": "waiting", "server_version": s.opts.Version})
}

// handlePairPoll: GET /api/pair/{id}?nonce=. 200 {"token"} exactly once, 202 waiting, 404 unknown.
func (s *Server) handlePairPoll(w http.ResponseWriter, r *http.Request) {
	id, nonce := r.PathValue("id"), r.URL.Query().Get("nonce")
	s.mu.Lock()
	defer s.mu.Unlock()
	p := s.pending(id)
	if p == nil || !sameHash(p.nonceHash, hashHex(nonce)) {
		http.Error(w, "no such pairing request", http.StatusNotFound)
		return
	}
	if p.token == "" {
		writeJSON(w, http.StatusAccepted, map[string]any{"state": "waiting"})
		return
	}
	delete(s.pair.open, id)
	writeJSON(w, http.StatusOK, map[string]any{"state": "paired", "token": p.token})
}

var pairDone = template.Must(template.New("paired").Parse(`<!doctype html><html><head><meta charset="utf-8"><title>thinframe</title>
<style>html,body{margin:0;height:100%;background:#f4dcaa;color:#5b3a1e;font:700 64px Georgia,serif;display:flex;align-items:center;justify-content:center;text-align:center}
p{margin:0}small{display:block;font-size:30px;color:#b8452a;margin-top:.5em}</style></head>
<body><p>{{.Title}}<small>{{.Detail}}</small></p></body></html>`))

// handlePairAccept: POST /pair/{id}/accept (form nonce=), sent by the PAIR button on the frame.
func (s *Server) handlePairAccept(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	nonce := r.FormValue("nonce")
	page := func(code int, title, detail string) {
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		w.WriteHeader(code)
		_ = pairDone.Execute(w, map[string]string{"Title": title, "Detail": detail})
	}
	s.mu.Lock()
	p := s.pending(id)
	if p == nil || !sameHash(p.nonceHash, hashHex(nonce)) {
		s.mu.Unlock()
		page(http.StatusNotFound, "This request expired", "The frame will ask again in a moment.")
		return
	}
	if p.token != "" {
		s.mu.Unlock()
		page(http.StatusOK, "Paired", "Loading your dashboard")
		return
	}
	b := make([]byte, 32)
	if _, err := rand.Read(b); err != nil {
		s.mu.Unlock()
		http.Error(w, "no randomness", http.StatusInternalServerError)
		return
	}
	tok := hex.EncodeToString(b)
	s.mu.Unlock()
	if _, err := s.Store.Ensure(id); err != nil {
		writeErr(w, err)
		return
	}
	paired := false
	_, err := s.Store.Update(id, func(f *store.Frame) {
		if f.TokenHash != "" {
			return
		}
		now := time.Now().UTC()
		f.TokenHash, f.PairedAt, paired = hashHex(tok), &now, true
		store.AddEvent(f, "pair", "paired from the frame's button")
	})
	if err != nil {
		writeErr(w, err)
		return
	}
	if !paired {
		page(http.StatusConflict, "Already paired", "Forget this frame on the admin page to pair again.")
		return
	}
	s.mu.Lock()
	// Only the request that was accepted gets the token. A newer request for the
	// same id (a different nonce) may have replaced it while the store was updated.
	if s.pair.open[id] == p {
		p.token = tok
	}
	s.mu.Unlock()
	page(http.StatusOK, "Paired", "Loading your dashboard")
}

// Unpair forgets a frame's token; the frame shows the PAIR button again on its next connect.
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
