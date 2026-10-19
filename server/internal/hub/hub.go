// Package hub tracks the one control WebSocket each frame keeps open, and lets
// REST, admin and MCP push to it. Messages are JSON objects with a "type" field.
package hub

import (
	"context"
	"encoding/json"
	"sync"
	"time"

	"github.com/coder/websocket"
)

// Msg is any control-plane message.
type Msg map[string]any

type conn struct {
	ws   *websocket.Conn
	send chan []byte
}

// Hub is safe for concurrent use.
type Hub struct {
	mu    sync.RWMutex
	conns map[string]*conn
	// OnClose is called when a frame disconnects (after Remove).
	OnClose func(id string)
}

// New returns an empty hub.
func New() *Hub { return &Hub{conns: map[string]*conn{}} }

// Add registers a frame connection, replacing any previous one, and starts the writer.
// It returns a function the reader loop must call when the socket dies.
func (h *Hub) Add(ctx context.Context, id string, ws *websocket.Conn) (done func()) {
	c := &conn{ws: ws, send: make(chan []byte, 32)}
	h.mu.Lock()
	if old, ok := h.conns[id]; ok {
		close(old.send)
		_ = old.ws.Close(websocket.StatusPolicyViolation, "replaced by a newer connection")
	}
	h.conns[id] = c
	h.mu.Unlock()
	go func() {
		for b := range c.send {
			wctx, cancel := context.WithTimeout(ctx, 10*time.Second)
			err := ws.Write(wctx, websocket.MessageText, b)
			cancel()
			if err != nil {
				return
			}
		}
	}()
	return func() {
		h.mu.Lock()
		cur, ok := h.conns[id]
		mine := ok && cur == c
		h.mu.Unlock()
		if !mine {
			return
		}
		// OnClose runs while the frame still counts as online, so callers that
		// wait for Online(id) == false know all bookkeeping has finished.
		if h.OnClose != nil {
			h.OnClose(id)
		}
		h.mu.Lock()
		if cur, ok := h.conns[id]; ok && cur == c {
			delete(h.conns, id)
			close(c.send)
		}
		h.mu.Unlock()
	}
}

// Send queues a message for a frame. False when the frame is offline.
func (h *Hub) Send(id string, m Msg) bool {
	b, err := json.Marshal(m)
	if err != nil {
		return false
	}
	h.mu.RLock()
	c, ok := h.conns[id]
	h.mu.RUnlock()
	if !ok {
		return false
	}
	select {
	case c.send <- b:
		return true
	default:
		return false // slow consumer; the frame will resync on reconnect
	}
}

// Online reports whether a frame has a live socket.
func (h *Hub) Online(id string) bool {
	h.mu.RLock()
	defer h.mu.RUnlock()
	_, ok := h.conns[id]
	return ok
}

// OnlineIDs lists connected frames.
func (h *Hub) OnlineIDs() []string {
	h.mu.RLock()
	defer h.mu.RUnlock()
	out := make([]string, 0, len(h.conns))
	for id := range h.conns {
		out = append(out, id)
	}
	return out
}
