package hub

import (
	"context"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"

	"github.com/coder/websocket"
)

// dial opens a server-side websocket for the hub and a client end to keep it alive.
func dial(t *testing.T) *websocket.Conn {
	t.Helper()
	got := make(chan *websocket.Conn, 1)
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		c, err := websocket.Accept(w, r, nil)
		if err != nil {
			t.Error(err)
			return
		}
		got <- c
		<-r.Context().Done()
	}))
	t.Cleanup(ts.Close)
	c, _, err := websocket.Dial(context.Background(), "ws"+strings.TrimPrefix(ts.URL, "http"), nil)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = c.CloseNow() })
	// Drain the client end so the hub's close handshake completes promptly.
	go func() {
		for {
			if _, _, err := c.Read(context.Background()); err != nil {
				return
			}
		}
	}()
	return <-got
}

// A frame reconnecting while something sends to it must not panic the process:
// Add closes the old send channel, and Send must never write to a closed one.
func TestSendDuringReconnectDoesNotPanic(t *testing.T) {
	h := New()
	ctx := context.Background()
	var wg sync.WaitGroup
	stop := make(chan struct{})
	wg.Add(1)
	go func() {
		defer wg.Done()
		for {
			select {
			case <-stop:
				return
			default:
				h.Send("f", Msg{"type": "ping"})
			}
		}
	}()
	for i := 0; i < 40; i++ {
		done := h.Add(ctx, "f", dial(t))
		if i%2 == 1 {
			done()
		}
	}
	close(stop)
	wg.Wait()
}
