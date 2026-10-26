package api

import (
	"encoding/json"
	"sync"
)

// sseBuffer is the per-page queue. MaxSources plus a layout's built-in feeds fit with room.
const sseBuffer = 256

// broker fans SSE messages out per frame id. A subscriber that hosts apps (a page with app
// widgets, or an app shown full screen) also gets the app-wide messages: every source change,
// app state, app events and app reloads.
type broker struct {
	mu   sync.Mutex
	subs map[string]map[chan []byte]bool // id -> channel -> hosts apps
}

func newBroker() *broker { return &broker{subs: map[string]map[chan []byte]bool{}} }

func (b *broker) subscribe(id string) (chan []byte, func()) { return b.subscribeAs(id, false) }

func (b *broker) subscribeAs(id string, apps bool) (chan []byte, func()) {
	// Room for a page's whole replay on connect (weather, calendar and every source its widgets
	// read, one message each) while the stream is still flushing the first ones.
	ch := make(chan []byte, sseBuffer)
	b.mu.Lock()
	if b.subs[id] == nil {
		b.subs[id] = map[chan []byte]bool{}
	}
	b.subs[id][ch] = apps
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
		trySend(ch, data)
	}
}

// publishApps sends v to every subscriber that hosts apps, on every frame. frame "" = all.
func (b *broker) publishApps(frame string, v any) {
	data, err := json.Marshal(v)
	if err != nil {
		return
	}
	b.mu.Lock()
	defer b.mu.Unlock()
	for id, m := range b.subs {
		if frame != "" && id != frame {
			continue
		}
		for ch, apps := range m {
			if apps {
				trySend(ch, data)
			}
		}
	}
}

func trySend(ch chan []byte, data []byte) {
	select {
	case ch <- data:
	default: // slow page; drop
	}
}
