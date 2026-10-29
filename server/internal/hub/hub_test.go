package hub

import (
	"context"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/coder/websocket"
)

// dial opens a server-side websocket for the hub and a client end to keep it alive.
func dial(t *testing.T) *websocket.Conn { return dialConn(t, true) }

// dialConn is dial; with drain false the client never reads, like a frame that vanished, so a
// close handshake on the server end waits out its timeout.
func dialConn(t *testing.T, drain bool) *websocket.Conn {
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
	if !drain {
		return <-got
	}
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

// A frame reconnecting over a dead socket must not freeze the hub: closing the old socket
// waits for a close frame that never comes, and that wait must not hold the hub's lock.
func TestReconnectDoesNotBlockHub(t *testing.T) {
	h := New()
	ctx := context.Background()
	h.Add(ctx, "f", dialConn(t, false))
	h.Add(ctx, "g", dial(t))
	start := time.Now()
	h.Add(ctx, "f", dial(t)) // replaces the silent socket
	h.Online("g")
	h.Send("g", Msg{"type": "ping"})
	if d := time.Since(start); d > time.Second {
		t.Fatalf("reconnect stalled the hub for %s", d)
	}
}
