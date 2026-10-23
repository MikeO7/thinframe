// Package discover answers the frame's LAN probe, so a frame finds this server on its own when it
// was flashed without a server address, or when the server's address changed (tfctl
// discover.rs). One UDP port. A probe is
//
//	thinframe-discover 1 [<frame id> <nonce>]
//
// and the reply is
//
//	thinframe-server 1 <http port> [<proof>...]
//
// The frame builds http://<reply source>:<port> from the datagram's source address; the server's
// own address is never guessed, the kernel fills it in on the way out.
//
// A frame that holds a token must not hand it to whoever answers a broadcast, so it sends its id
// and a fresh nonce and only moves to a server that proves it knows the token: each proof is
// hex(HMAC-SHA256(key, "thinframe-discover|<id>|<nonce>|<port>")) with key = the hex SHA-256 of
// a token the server accepts for that frame (its paired token, whose hash is all the server
// stores, and FRAME_TOKEN). A frame with no token yet takes a reply without proof; nothing is
// trusted until the PAIR button on its screen is pressed (api/pair.go).
//
// Broadcasts reach a server on the host's network (a native process, or a container with host
// networking). A container behind a bridge with a published port does not see them.
package discover

import (
	"context"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/hex"
	"log"
	"net"
	"strings"
)

// Port is the UDP port frames probe (also the default in tfctl).
const Port = 41007

const (
	probe = "thinframe-discover 1"
	reply = "thinframe-server 1 "
)

// Keys returns the token hashes (hex SHA-256) the server accepts for frame id; nil when none.
type Keys func(id string) []string

// Serve answers probes on addr (":41007") with httpPort until ctx ends. It returns the listen
// error at once, nil after ctx is done. keys may be nil (no proofs).
func Serve(ctx context.Context, addr, httpPort string, keys Keys) error {
	pc, err := net.ListenPacket("udp4", addr)
	if err != nil {
		return err
	}
	log.Printf("discovery: answering frames on udp %s with port %s", pc.LocalAddr(), httpPort)
	go func() {
		<-ctx.Done()
		pc.Close()
	}()
	buf := make([]byte, 256)
	for {
		n, from, err := pc.ReadFrom(buf)
		if err != nil {
			if ctx.Err() != nil {
				return nil
			}
			log.Printf("discovery: read: %v", err)
			continue
		}
		msg, ok := Answer(buf[:n], httpPort, keys)
		if !ok {
			continue
		}
		if _, err := pc.WriteTo([]byte(msg), from); err != nil {
			log.Printf("discovery: reply to %s: %v", from, err)
		}
	}
}

// Answer is the reply to one datagram, or false when it is not a probe.
func Answer(b []byte, httpPort string, keys Keys) (string, bool) {
	f := strings.Fields(string(b))
	if len(f) < 2 || f[0]+" "+f[1] != probe || (len(f) != 2 && len(f) != 4) {
		return "", false
	}
	out := reply + httpPort
	if len(f) == 4 && keys != nil {
		id, nonce := strings.ToLower(f[2]), f[3]
		if len(id) <= 32 && len(nonce) >= 16 && len(nonce) <= 64 {
			for _, k := range keys(id) {
				out += " " + Proof(k, id, nonce, httpPort)
			}
		}
	}
	return out + "\n", true
}

// Proof is the MAC a server sends for one token hash (see the package comment).
func Proof(tokenHash, id, nonce, httpPort string) string {
	m := hmac.New(sha256.New, []byte(tokenHash))
	m.Write([]byte("thinframe-discover|" + id + "|" + nonce + "|" + httpPort))
	return hex.EncodeToString(m.Sum(nil))
}

// HashToken is the hex SHA-256 of a token, the form keys come in.
func HashToken(t string) string { h := sha256.Sum256([]byte(t)); return hex.EncodeToString(h[:]) }
