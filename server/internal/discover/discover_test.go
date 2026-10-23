package discover

import (
	"context"
	"net"
	"testing"
	"time"
)

func start(t *testing.T) (string, context.CancelFunc) {
	t.Helper()
	// Find a free port first: Serve listens itself.
	pc, err := net.ListenPacket("udp4", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	addr := pc.LocalAddr().String()
	pc.Close()
	ctx, cancel := context.WithCancel(context.Background())
	go func() {
		if err := Serve(ctx, addr, "8080", testKeys); err != nil {
			t.Errorf("serve: %v", err)
		}
	}()
	time.Sleep(50 * time.Millisecond)
	return addr, cancel
}

func ask(t *testing.T, addr, msg string, wait time.Duration) (string, net.Addr) {
	t.Helper()
	c, err := net.Dial("udp4", addr)
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	if _, err := c.Write([]byte(msg)); err != nil {
		t.Fatal(err)
	}
	c.SetReadDeadline(time.Now().Add(wait))
	buf := make([]byte, 256)
	n, err := c.Read(buf)
	if err != nil {
		return "", nil
	}
	return string(buf[:n]), c.RemoteAddr()
}

func TestProbeGetsThePort(t *testing.T) {
	addr, cancel := start(t)
	defer cancel()
	got, from := ask(t, addr, "thinframe-discover 1\n", time.Second)
	if got != "thinframe-server 1 8080\n" {
		t.Fatalf("reply %q", got)
	}
	if from.String() != addr {
		t.Fatalf("reply from %s, want %s", from, addr)
	}
}

func TestOtherDatagramsGetNoReply(t *testing.T) {
	addr, cancel := start(t)
	defer cancel()
	for _, m := range []string{"", "hello", "thinframe-discover 2", "thinframe-server 1 8080"} {
		if got, _ := ask(t, addr, m, 150*time.Millisecond); got != "" {
			t.Fatalf("%q answered with %q", m, got)
		}
	}
	// still alive after the noise
	if got, _ := ask(t, addr, "thinframe-discover 1", time.Second); got == "" {
		t.Fatal("no reply after unrelated datagrams")
	}
}

func TestStopsWithContext(t *testing.T) {
	addr, cancel := start(t)
	cancel()
	time.Sleep(50 * time.Millisecond)
	if got, _ := ask(t, addr, "thinframe-discover 1", 150*time.Millisecond); got != "" {
		t.Fatalf("answered after cancel: %q", got)
	}
}

// testKeys: the frame "office" has the token "t0ken".
func testKeys(id string) []string {
	if id == "office" {
		return []string{HashToken("t0ken")}
	}
	return nil
}

// The same vector is in tfctl discover.rs: both sides must agree byte for byte.
const vector = "26dbd77d3662d486572c0fc1ac939b856f215d26840f62600ac4a4efdb781374"

func TestProofVector(t *testing.T) {
	if got := Proof(HashToken("t0ken"), "office", "0123456789abcdef", "8080"); got != vector {
		t.Fatalf("proof %s", got)
	}
}

func TestProbeWithNonceGetsAProof(t *testing.T) {
	addr, cancel := start(t)
	defer cancel()
	got, _ := ask(t, addr, "thinframe-discover 1 Office 0123456789abcdef\n", time.Second)
	if got != "thinframe-server 1 8080 "+vector+"\n" {
		t.Fatalf("reply %q", got)
	}
	// Unknown frame: the port, no proof. Short nonce: no proof either.
	for _, m := range []string{"thinframe-discover 1 kitchen 0123456789abcdef", "thinframe-discover 1 office short"} {
		if got, _ := ask(t, addr, m, time.Second); got != "thinframe-server 1 8080\n" {
			t.Fatalf("%q: reply %q", m, got)
		}
	}
	// Three fields is not a probe.
	if got, _ := ask(t, addr, "thinframe-discover 1 office", 150*time.Millisecond); got != "" {
		t.Fatalf("answered %q", got)
	}
}
