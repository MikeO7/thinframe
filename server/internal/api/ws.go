package api

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"strings"
	"time"

	"github.com/coder/websocket"

	"github.com/MikeO7/thinframe/server/internal/hub"
	"github.com/MikeO7/thinframe/server/internal/store"
)

// handleWS is the control plane: one socket per frame, frame-initiated.
// Frame → server: hello, heartbeat, ack, pong. Server → frame: config, action, ping.
func (s *Server) handleWS(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	if !store.ValidID(id) {
		http.Error(w, "bad id", http.StatusBadRequest)
		return
	}
	c, err := websocket.Accept(w, r, &websocket.AcceptOptions{InsecureSkipVerify: true}) // LAN; origin is the frame
	if err != nil {
		return
	}
	ctx := r.Context()
	done := s.Hub.Add(ctx, id, c)
	defer done()
	defer c.CloseNow()
	c.SetReadLimit(1 << 16)

	f, err := s.Store.Update(id, func(f *store.Frame) { store.AddEvent(f, "ws", "connected from "+r.RemoteAddr) })
	if err != nil {
		return
	}
	s.pageReset(id)
	// Full state on connect: config, then every unacked action.
	s.pushConfig(f)
	for _, a := range f.Pending {
		s.Hub.Send(id, actionMsg(a))
	}

	pingT := time.NewTicker(30 * time.Second)
	defer pingT.Stop()
	pageT := time.NewTicker(s.pageCheck)
	defer pageT.Stop()
	go func() {
		for {
			select {
			case <-ctx.Done():
				return
			case <-pageT.C:
				s.checkPage(id)
			case <-pingT.C:
				pctx, cancel := context.WithTimeout(ctx, 10*time.Second)
				err := c.Ping(pctx)
				cancel()
				if err != nil {
					c.CloseNow()
					return
				}
			}
		}
	}()

	for {
		_, data, err := c.Read(ctx)
		if err != nil {
			return
		}
		var m struct {
			Type string          `json:"type"`
			ID   string          `json:"id"`
			OK   *bool           `json:"ok"`
			Err  string          `json:"error"`
			Raw  json.RawMessage `json:"-"`
		}
		if json.Unmarshal(data, &m) != nil {
			continue
		}
		switch m.Type {
		case "hello":
			var hb store.Heartbeat
			_ = json.Unmarshal(data, &hb)
			_, _ = s.recordHeartbeat(id, hb)
			s.Hub.Send(id, hub.Msg{"type": "hello", "time_unix": time.Now().Unix(), "tz": localTZ(), "server_version": s.opts.Version})
		case "heartbeat":
			var hb store.Heartbeat
			_ = json.Unmarshal(data, &hb)
			_, _ = s.recordHeartbeat(id, hb)
		case "ack":
			ok := m.OK != nil && *m.OK
			_, _ = s.Store.Update(id, func(f *store.Frame) {
				if !store.Ack(f, m.ID, ok, m.Err) {
					store.AddEvent(f, "ack", fmt.Sprintf("unknown action id %s", m.ID))
				}
			})
		case "event":
			var ev struct {
				Kind, Text string
			}
			_ = json.Unmarshal(data, &ev)
			_, _ = s.Store.Update(id, func(f *store.Frame) { store.AddEvent(f, "frame:"+ev.Kind, ev.Text) })
		}
	}
}

// localTZ returns an IANA zone name for the frames: $TZ, else the /etc/localtime
// symlink target, else UTC. Go's time.Local.String() is "Local" and useless to a frame.
func localTZ() string {
	if tz := os.Getenv("TZ"); tz != "" {
		return tz
	}
	if target, err := os.Readlink("/etc/localtime"); err == nil {
		if i := strings.Index(target, "zoneinfo/"); i >= 0 {
			return target[i+len("zoneinfo/"):]
		}
	}
	return "UTC"
}
