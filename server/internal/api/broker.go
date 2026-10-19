package api

import (
	"encoding/json"
	"sync"
)

// broker fans SSE messages out per frame id.
type broker struct {
	mu   sync.Mutex
	subs map[string]map[chan []byte]struct{}
}

func newBroker() *broker { return &broker{subs: map[string]map[chan []byte]struct{}{}} }

func (b *broker) subscribe(id string) (chan []byte, func()) {
	ch := make(chan []byte, 16)
	b.mu.Lock()
	if b.subs[id] == nil {
		b.subs[id] = map[chan []byte]struct{}{}
	}
	b.subs[id][ch] = struct{}{}
	b.mu.Unlock()
	return ch, func() {
		b.mu.Lock()
		delete(b.subs[id], ch)
		b.mu.Unlock()
	}
}

func (b *broker) publish(id string, v any) {
	data, err := json.Marshal(v)
	if err != nil {
		return
	}
	b.mu.Lock()
	defer b.mu.Unlock()
	for ch := range b.subs[id] {
		select {
		case ch <- data:
		default: // slow page; drop
		}
	}
}
