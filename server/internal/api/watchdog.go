package api

import (
	"fmt"
	"net/http"
	"sync"
	"time"

	"github.com/MikeO7/thinframe/server/internal/store"
)

// pageWatch is the hung-page check (TASK 9). The dashboard pings /alive from a JS timer, which
// stops when the WebProcess hangs, while cog's network process keeps the SSE socket open. A page
// that pinged during the current agent connection and then stays quiet for pageStale gets one
// restart-kiosk; the next restart needs a fresh ping first. Frames showing another URL (or the
// local status page) never ping and are left alone.
type pageWatch struct {
	mu   sync.Mutex
	last map[string]time.Time
}

// handleAlive takes the page's ping (no auth: sendBeacon cannot set headers; a forged ping can
// only delay a restart). Only known frames are remembered, so pings for made-up ids cannot grow
// the map; the answer is the same either way.
func (s *Server) handleAlive(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	if !store.ValidID(id) {
		http.Error(w, "bad frame id", http.StatusBadRequest)
		return
	}
	if _, err := s.Store.Get(id); err == nil {
		s.page.mu.Lock()
		s.page.last[id] = time.Now()
		s.page.mu.Unlock()
	}
	w.WriteHeader(http.StatusNoContent)
}

// pageReset forgets pings from before this agent connection.
func (s *Server) pageReset(id string) {
	s.page.mu.Lock()
	delete(s.page.last, id)
	s.page.mu.Unlock()
}

// checkPage restarts cog on a frame whose page stopped pinging; true when it did.
func (s *Server) checkPage(id string) bool {
	s.page.mu.Lock()
	t, ok := s.page.last[id]
	stale := ok && time.Since(t) >= s.pageStale
	if stale {
		delete(s.page.last, id)
	}
	s.page.mu.Unlock()
	if !stale {
		return false
	}
	_, _ = s.Store.Update(id, func(f *store.Frame) {
		store.AddEvent(f, "page", fmt.Sprintf("no ping for %s, restarting cog", time.Since(t).Round(time.Second)))
	})
	_, err := s.Act(id, "restart-kiosk", nil)
	return err == nil
}
