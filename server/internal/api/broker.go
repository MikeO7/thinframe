package api

import (
	"encoding/json"
	"sync"
)

// sseBuffer is the per-page queue. MaxSources plus a layout's built-in feeds fit with room.
const sseBuffer = 256

// broker fans SSE messages out per frame id.
type broker struct {
	mu   sync.Mutex
	subs map[string]map[chan []byte]struct{}
}

func newBroker() *broker { return &broker{subs: map[string]map[chan []byte]struct{}{}} }

func (b *broker) subscribe(id string) (chan []byte, func()) {
	// Room for a page's whole replay on connect (weather, calendar and every source its widgets
	// read, one message each) while the stream is still flushing the first ones.
	ch := make(chan []byte, sseBuffer)
	b.mu.Lock()
	if b.subs[id] == nil {
		b.subs[id] = map[chan []byte]struct{}{}
	}
	b.subs[id][ch] = struct{}{}
	b.mu.Unlock()
	return ch, func() {
		b.mu.Lock()
		delete(b.subs[id], ch)
		if len(b.subs[id]) == 0 {
			delete(b.subs, id) // or every id ever streamed keeps an empty map forever
		}
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
