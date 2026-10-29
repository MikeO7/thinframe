package api

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net/http"
	"net/url"
	"os"
	"slices"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/MikeO7/thinframe/server/internal/store"
)

// Hooks and polls connect apps to outside APIs without putting a secret in a page.
//
// A hook is a named HTTP request the server makes when an app emits an event of that name
// (a button press): Home Assistant, a webhook, anything with an HTTP API. Its URL and headers
// (tokens) stay in DATA_DIR/hooks.json; the app only knows the name.
//
// A poll fetches a URL every N seconds and stores the answer as a source, so widgets and apps
// get live data (thinframe.on) from APIs that cannot push.
//
// Both are set over MCP (FRAME_TOKEN), never from a page.

// Limits.
const (
	MaxHooks       = 64
	MaxPolls       = 32
	maxHookBody    = 16 << 10
	maxHookAnswer  = 256 << 10
	maxHookHeaders = 20
	minPollEvery   = 15
)

// Hook is a request run by an emit of the same name. In URL and Body, {{frame}}, {{app}} and
// {{name}} are replaced by those ids and {{data}} by the event's data as JSON. An empty Body
// (for methods with one) sends {"frame","app","name","data"}.
type Hook struct {
	Method     string            `json:"method,omitempty"`
	URL        string            `json:"url"`
	Headers    map[string]string `json:"headers,omitempty"`
	Body       string            `json:"body,omitempty"`
	TimeoutS   int               `json:"timeout_s,omitempty"`
	ReturnBody bool              `json:"return_body,omitempty"`
}

// Poll fetches URL every EveryS seconds into the source of the same key. Pick is an optional
// dotted path into the JSON answer ("data.items", "list.0"). A JSON object is stored as is;
// any other JSON is stored as {"value": ...} and non-JSON text as {"text": ...}.
type Poll struct {
	URL     string            `json:"url"`
	EveryS  int               `json:"every_s"`
	Headers map[string]string `json:"headers,omitempty"`
	Pick    string            `json:"pick,omitempty"`
}

// PollStatus is a poll with its last outcome (headers redacted).
type PollStatus struct {
	Key     string    `json:"key"`
	Poll    Poll      `json:"poll"`
	LastAt  time.Time `json:"last_at,omitzero"`
	LastOK  time.Time `json:"last_ok,omitzero"`
	LastErr string    `json:"last_error,omitempty"`
}

// jsonSet is a small map persisted as one JSON file (mode 600: it holds tokens).
type jsonSet[T any] struct {
	mu   sync.RWMutex
	path string
	m    map[string]T
}

func loadSet[T any](path string) *jsonSet[T] {
	s := &jsonSet[T]{path: path, m: map[string]T{}}
	b, err := os.ReadFile(path)
	if err == nil {
		err = json.Unmarshal(b, &s.m)
	}
	if err != nil && !os.IsNotExist(err) {
		// The next change would replace the file with what is in memory: set it aside first,
		// so its entries (and their tokens) can be recovered by hand.
		s.m = map[string]T{}
		bad := fmt.Sprintf("%s.corrupt-%s", path, time.Now().UTC().Format("20060102T150405Z"))
		if rerr := os.Rename(path, bad); rerr != nil {
			log.Printf("ERROR: %s is unreadable (%v) and could not be moved aside (%v); starting empty", path, err, rerr)
		} else {
			log.Printf("ERROR: %s is unreadable (%v); moved to %s, starting empty", path, err, bad)
		}
	}
	return s
}

func (s *jsonSet[T]) get(k string) (T, bool) {
	s.mu.RLock()
	defer s.mu.RUnlock()
	v, ok := s.m[k]
	return v, ok
}

func (s *jsonSet[T]) keys() []string {
	s.mu.RLock()
	defer s.mu.RUnlock()
	out := make([]string, 0, len(s.m))
	for k := range s.m {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}

func (s *jsonSet[T]) put(k string, v T, limit int) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, ok := s.m[k]; !ok && len(s.m) >= limit {
		return fmt.Errorf("%w: limit %d", ErrInvalid, limit)
	}
	next := maps(s.m)
	next[k] = v
	if err := s.save(next); err != nil {
		return err
	}
	s.m = next
	return nil
}

func (s *jsonSet[T]) del(k string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, ok := s.m[k]; !ok {
		return false
	}
	next := maps(s.m)
	delete(next, k)
	if s.save(next) != nil {
		return false
	}
	s.m = next
	return true
}

func (s *jsonSet[T]) save(m map[string]T) error {
	b, err := json.MarshalIndent(m, "", "  ")
	if err != nil {
		return err
	}
	return writeDurable(s.path, b)
}

func maps[T any](m map[string]T) map[string]T {
	out := make(map[string]T, len(m)+1)
	for k, v := range m {
		out[k] = v
	}
	return out
}

func redact(h map[string]string) map[string]string {
	if len(h) == 0 {
		return nil
	}
	out := map[string]string{}
	for k := range h {
		out[k] = "(set)"
	}
	return out
}

func checkURL(raw string) error {
	u, err := url.Parse(raw)
	if err != nil || (u.Scheme != "http" && u.Scheme != "https") || u.Host == "" {
		return fmt.Errorf("%w: url must be an absolute http:// or https:// URL, got %q", ErrInvalid, raw)
	}
	return nil
}

func checkHeaders(h map[string]string) error {
	if len(h) > maxHookHeaders {
		return fmt.Errorf("%w: at most %d headers", ErrInvalid, maxHookHeaders)
	}
	for k, v := range h {
		if k == "" || strings.ContainsAny(k, " :\r\n") || strings.ContainsAny(v, "\r\n") || len(k)+len(v) > 4096 {
			return fmt.Errorf("%w: bad header %q", ErrInvalid, k)
		}
	}
	return nil
}

var hookMethods = []string{"GET", "POST", "PUT", "PATCH", "DELETE"}

// SetHook stores a hook under name (an emit name).
func (s *Server) SetHook(name string, h Hook) error {
	if !store.ValidID(name) {
		return fmt.Errorf("%w: hook name %q (lowercase letters, digits, - and _)", ErrInvalid, name)
	}
	h.Method = strings.ToUpper(h.Method)
	if h.Method == "" {
		h.Method = "POST"
	}
	if !slices.Contains(hookMethods, h.Method) {
		return fmt.Errorf("%w: method must be one of %s", ErrInvalid, strings.Join(hookMethods, ", "))
	}
	if err := checkURL(strings.NewReplacer("{{frame}}", "f", "{{app}}", "a", "{{name}}", "n").Replace(h.URL)); err != nil {
		return err
	}
	if err := checkHeaders(h.Headers); err != nil {
		return err
	}
	if len(h.Body) > maxHookBody {
		return fmt.Errorf("%w: body template is limited to %d bytes", ErrInvalid, maxHookBody)
	}
	if h.TimeoutS < 0 || h.TimeoutS > 30 {
		return fmt.Errorf("%w: timeout_s must be 0..30", ErrInvalid)
	}
	return s.hooks.put(name, h, MaxHooks)
}

// DeleteHook forgets a hook.
func (s *Server) DeleteHook(name string) bool { return s.hooks.del(name) }

// Hooks lists the hooks with header values redacted.
func (s *Server) Hooks() map[string]Hook {
	out := map[string]Hook{}
	for _, k := range s.hooks.keys() {
		h, _ := s.hooks.get(k)
		h.Headers = redact(h.Headers)
		out[k] = h
	}
	return out
}

type hookVars struct {
	frame, app, name string
	data             json.RawMessage
}

var hookClient = &http.Client{CheckRedirect: func(_ *http.Request, via []*http.Request) error {
	if len(via) >= 3 {
		return http.ErrUseLastResponse
	}
	return nil
}}

func (s *Server) runHook(ctx context.Context, h Hook, v hookVars) EmitResult {
	data := v.data
	if len(data) == 0 {
		data = json.RawMessage("null")
	}
	rep := strings.NewReplacer("{{frame}}", v.frame, "{{app}}", v.app, "{{name}}", v.name)
	u := rep.Replace(h.URL)
	var body io.Reader
	if h.Method != "GET" && h.Method != "DELETE" || h.Body != "" {
		b := h.Body
		if b == "" {
			def, _ := json.Marshal(map[string]any{"frame": v.frame, "app": v.app, "name": v.name, "data": data})
			b = string(def)
		} else {
			b = strings.ReplaceAll(rep.Replace(b), "{{data}}", string(data))
		}
		body = strings.NewReader(b)
	}
	timeout := time.Duration(h.TimeoutS) * time.Second
	if timeout == 0 {
		timeout = 10 * time.Second
	}
	ctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, h.Method, u, body)
	if err != nil {
		return EmitResult{Error: err.Error()}
	}
	if body != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	for k, val := range h.Headers {
		req.Header.Set(k, val)
	}
	resp, err := hookClient.Do(req)
	if err != nil {
		return EmitResult{Error: err.Error()}
	}
	defer resp.Body.Close()
	res := EmitResult{OK: resp.StatusCode < 400, Status: resp.StatusCode}
	if h.ReturnBody {
		b, _ := io.ReadAll(io.LimitReader(resp.Body, maxHookAnswer))
		if json.Valid(b) {
			res.Body = b
		} else {
			res.Body, _ = json.Marshal(string(b))
		}
	}
	return res
}

// ---- polls ----------------------------------------------------------------------------------

type pollRun struct {
	mu   sync.Mutex
	last map[string]PollStatus
	busy map[string]bool
}

// SetPoll stores a poll; it runs within a few seconds and then every EveryS.
func (s *Server) SetPoll(key string, p Poll) error {
	if !store.ValidID(key) {
		return fmt.Errorf("%w: poll key %q (it names the source it fills)", ErrInvalid, key)
	}
	if err := checkURL(p.URL); err != nil {
		return err
	}
	if err := checkHeaders(p.Headers); err != nil {
		return err
	}
	if p.EveryS < minPollEvery || p.EveryS > 86400 {
		return fmt.Errorf("%w: every_s must be %d..86400", ErrInvalid, minPollEvery)
	}
	if len(p.Pick) > 200 {
		return fmt.Errorf("%w: pick is too long", ErrInvalid)
	}
	if err := s.polls.put(key, p, MaxPolls); err != nil {
		return err
	}
	s.pollRun.mu.Lock()
	delete(s.pollRun.last, key) // due now
	s.pollRun.mu.Unlock()
	return nil
}

// DeletePoll stops a poll. The source keeps its last value.
func (s *Server) DeletePoll(key string) bool { return s.polls.del(key) }

// Polls lists polls with their last outcome.
func (s *Server) Polls() []PollStatus {
	s.pollRun.mu.Lock()
	defer s.pollRun.mu.Unlock()
	out := []PollStatus{}
	for _, k := range s.polls.keys() {
		p, _ := s.polls.get(k)
		st := s.pollRun.last[k]
		p.Headers = redact(p.Headers)
		st.Key, st.Poll = k, p
		out = append(out, st)
	}
	return out
}

// RunPolls runs due polls until ctx ends.
func (s *Server) RunPolls(ctx context.Context) {
	t := time.NewTicker(s.pollTick)
	defer t.Stop()
	for {
		s.pollDue(ctx)
		select {
		case <-ctx.Done():
			return
		case <-t.C:
		}
	}
}

func (s *Server) pollDue(ctx context.Context) {
	now := time.Now()
	for _, k := range s.polls.keys() {
		p, ok := s.polls.get(k)
		if !ok {
			continue
		}
		s.pollRun.mu.Lock()
		st := s.pollRun.last[k]
		due := !s.pollRun.busy[k] && now.Sub(st.LastAt) >= time.Duration(p.EveryS)*time.Second
		if due {
			s.pollRun.busy[k] = true
		}
		s.pollRun.mu.Unlock()
		if due {
			go s.pollOnce(ctx, k, p)
		}
	}
}

func (s *Server) pollOnce(ctx context.Context, key string, p Poll) {
	err := s.fetchPoll(ctx, key, p)
	s.pollRun.mu.Lock()
	defer s.pollRun.mu.Unlock()
	st := s.pollRun.last[key]
	st.LastAt, st.LastErr = time.Now().UTC(), ""
	if err != nil {
		st.LastErr = err.Error()
	} else {
		st.LastOK = st.LastAt
	}
	s.pollRun.last[key] = st
	delete(s.pollRun.busy, key)
}

func (s *Server) fetchPoll(ctx context.Context, key string, p Poll) error {
	ctx, cancel := context.WithTimeout(ctx, 20*time.Second)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, "GET", p.URL, nil)
	if err != nil {
		return err
	}
	req.Header.Set("Accept", "application/json, */*")
	for k, v := range p.Headers {
		req.Header.Set(k, v)
	}
	resp, err := hookClient.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode >= 400 {
		return fmt.Errorf("HTTP %d", resp.StatusCode)
	}
	b, err := io.ReadAll(io.LimitReader(resp.Body, 4*MaxSource))
	if err != nil {
		return err
	}
	obj, err := pollObject(b, p.Pick)
	if err != nil {
		return err
	}
	return s.SetSource(key, obj)
}

// pollObject turns an answer into a source object (see Poll).
func pollObject(b []byte, pick string) ([]byte, error) {
	var v any
	if err := json.Unmarshal(b, &v); err != nil {
		if pick != "" {
			return nil, fmt.Errorf("answer is not JSON, cannot pick %q", pick)
		}
		t := string(b)
		if len(t) > MaxSource/2 {
			t = t[:MaxSource/2]
		}
		v, pick = map[string]any{"text": t}, ""
	}
	if pick != "" {
		for _, part := range strings.Split(pick, ".") {
			switch c := v.(type) {
			case map[string]any:
				v = c[part]
			case []any:
				i, err := strconv.Atoi(part)
				if err != nil || i < 0 || i >= len(c) {
					return nil, fmt.Errorf("pick %q: no index %q", pick, part)
				}
				v = c[i]
			default:
				return nil, fmt.Errorf("pick %q: nothing at %q", pick, part)
			}
		}
	}
	if _, ok := v.(map[string]any); !ok {
		v = map[string]any{"value": v}
	}
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(v); err != nil {
		return nil, err
	}
	return bytes.TrimSpace(buf.Bytes()), nil
}
