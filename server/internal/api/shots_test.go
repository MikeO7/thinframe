package api

import (
	"context"
	"net/http"
	"strings"
	"testing"

	"github.com/coder/websocket"
)

// A screenshot request whose caller gives up (timeout, client gone) leaves no waiter behind.
func TestScreenshotWaiterRemovedWhenCallerLeaves(t *testing.T) {
	s, ts := newTest(t, "tok")
	url := "ws" + strings.TrimPrefix(ts.URL, "http") + "/api/frames/sim/ws"
	c, _, err := websocket.Dial(context.Background(), url, &websocket.DialOptions{HTTPHeader: http.Header{"Authorization": {"Bearer tok"}}})
	if err != nil {
		t.Fatal(err)
	}
	defer closeAndWait(t, s, "sim", c)
	readMsg(t, c) // config
	for range 3 {
		ctx, cancel := context.WithCancel(context.Background())
		go func() {
			readMsg(t, c) // the screenshot action, never answered
			cancel()
		}()
		if _, err := s.Screenshot(ctx, "sim"); err == nil {
			t.Fatal("a cancelled screenshot succeeded")
		}
	}
	s.shots.mu.Lock()
	n := len(s.shots.m["sim"])
	s.shots.mu.Unlock()
	if n != 0 {
		t.Fatalf("%d screenshot waiters left behind", n)
	}
}
