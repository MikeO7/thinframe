package api

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/coder/websocket"

	"github.com/MikeO7/thinframe/server/internal/artifacts"
	"github.com/MikeO7/thinframe/server/internal/store"
)

// fakeOpenMeteo stands in for api.open-meteo.com; no test touches the network.
func fakeOpenMeteo(t *testing.T, status int, body string) (*httptest.Server, *atomic.Int32, *atomic.Value) {
	t.Helper()
	var hits atomic.Int32
	var query atomic.Value
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		hits.Add(1)
		query.Store(r.URL.RawQuery)
		w.WriteHeader(status)
		_, _ = w.Write([]byte(body))
	}))
	t.Cleanup(ts.Close)
	return ts, &hits, &query
}

func TestOpenMeteoFetchAndCache(t *testing.T) {
	ts, hits, query := fakeOpenMeteo(t, 200, `{"current":{"temperature_2m":-2.6,"weather_code":71}}`)
	o := &OpenMeteo{Lat: 52.52, Lon: 13.405, BaseURL: ts.URL}
	wx, err := o.Current()
	if err != nil || wx.TempC != -2.6 || wx.Desc != "Light snow" {
		t.Fatalf("got %+v, %v", wx, err)
	}
	q, _ := query.Load().(string)
	for _, want := range []string{"latitude=52.5200", "longitude=13.4050", "current=temperature_2m,weather_code"} {
		if !strings.Contains(q, want) {
			t.Errorf("query %q lacks %q", q, want)
		}
	}
	if _, err := o.Current(); err != nil || hits.Load() != 1 {
		t.Fatalf("second call within 10 min should be cached: hits=%d err=%v", hits.Load(), err)
	}
}

func TestOpenMeteoErrorsKeepLastReading(t *testing.T) {
	ok, _, _ := fakeOpenMeteo(t, 200, `{"current":{"temperature_2m":20,"weather_code":99}}`)
	o := &OpenMeteo{BaseURL: ok.URL}
	if wx, err := o.Current(); err != nil || wx.Desc != "code 99" {
		t.Fatalf("unknown code: %+v %v", wx, err)
	}
	for _, c := range []struct {
		status int
		body   string
	}{{503, "busy"}, {200, "not json"}, {200, `{}`}} {
		bad, _, _ := fakeOpenMeteo(t, c.status, c.body)
		o.BaseURL, o.feed.next = bad.URL, time.Time{} // expire the cache and the retry backoff
		wx, err := o.Current()
		if err == nil || wx.TempC != 20 {
			t.Errorf("%d %q: want error and the last reading, got %+v %v", c.status, c.body, wx, err)
		}
	}
	empty := &OpenMeteo{BaseURL: "http://127.0.0.1:1"} // nothing listens: connection refused
	if wx, err := empty.Current(); err == nil || wx.Desc != "" {
		t.Fatalf("unreachable: %+v %v", wx, err)
	}
}

func TestICSFetchAndCache(t *testing.T) {
	var hits atomic.Int32
	status := atomic.Int32{}
	status.Store(200)
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		hits.Add(1)
		w.WriteHeader(int(status.Load()))
		_, _ = w.Write([]byte(sampleICS))
	}))
	defer ts.Close()
	c := &ICS{URL: ts.URL, Loc: time.UTC}
	now := time.Date(2026, 10, 20, 12, 0, 0, 0, time.UTC)
	evs, err := c.Upcoming(now, 3)
	if err != nil || len(evs) != 3 || evs[0].Summary != "Dinner, with Sam" {
		t.Fatalf("got %+v, %v", evs, err)
	}
	if _, err := c.Upcoming(now, 3); err != nil || hits.Load() != 1 {
		t.Fatalf("not cached: hits=%d err=%v", hits.Load(), err)
	}
	status.Store(404)
	c.feed.next = time.Time{}
	evs, err = c.Upcoming(now, 3)
	if err == nil || len(evs) != 3 {
		t.Fatalf("404 should return the cached events and an error: %d %v", len(evs), err)
	}
}

// A dead upstream is asked once per retry window (1, 2, 4 ... 10 minutes), and the last
// good reading is served in between.
func TestOpenMeteoFailureBackoff(t *testing.T) {
	var hits atomic.Int32
	var fail atomic.Bool
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		hits.Add(1)
		if fail.Load() {
			w.WriteHeader(503)
			return
		}
		_, _ = w.Write([]byte(`{"current":{"temperature_2m":7,"weather_code":3}}`))
	}))
	defer ts.Close()
	clock := time.Date(2026, 10, 22, 12, 0, 0, 0, time.UTC)
	o := &OpenMeteo{BaseURL: ts.URL}
	o.feed.clock = func() time.Time { return clock }
	if _, err := o.Current(); err != nil {
		t.Fatal(err)
	}
	fail.Store(true)
	clock = clock.Add(10 * time.Minute) // cache expired
	want := int32(1)
	for _, wait := range []time.Duration{time.Minute, 2 * time.Minute, 4 * time.Minute, 8 * time.Minute, 10 * time.Minute, 10 * time.Minute} {
		want++
		for range 5 {
			if wx, err := o.Current(); err == nil || wx.Desc != "Overcast" {
				t.Fatalf("want the last reading and an error, got %+v %v", wx, err)
			}
		}
		clock = clock.Add(wait - time.Second)
		if _, _ = o.Current(); hits.Load() != want {
			t.Fatalf("before the %v window ended: hits=%d, want %d", wait, hits.Load(), want)
		}
		clock = clock.Add(time.Second)
	}
	fail.Store(false)
	if wx, err := o.Current(); err != nil || hits.Load() != want+1 || wx.Desc != "Overcast" {
		t.Fatalf("recovery: %+v %v hits=%d", wx, err, hits.Load())
	}
}

// While a fetch is slow, other callers neither start a second one nor queue behind it
// once a cached value exists; before the first value they wait for that one fetch.
func TestICSSingleFlight(t *testing.T) {
	var hits atomic.Int32
	release := make(chan struct{})
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		hits.Add(1)
		<-release
		_, _ = w.Write([]byte(sampleICS))
	}))
	defer ts.Close()
	c := &ICS{URL: ts.URL, Loc: time.UTC}
	now := time.Date(2026, 10, 20, 12, 0, 0, 0, time.UTC)
	waitHit := func(n int32) {
		t.Helper()
		for deadline := time.Now().Add(5 * time.Second); hits.Load() < n; time.Sleep(time.Millisecond) {
			if time.Now().After(deadline) {
				t.Fatalf("upstream never hit %d times", n)
			}
		}
	}
	var wg sync.WaitGroup
	for range 10 {
		wg.Go(func() {
			if evs, err := c.Upcoming(now, 3); err != nil || len(evs) != 3 {
				t.Errorf("first fetch: %d %v", len(evs), err)
			}
		})
	}
	waitHit(1)
	time.Sleep(20 * time.Millisecond) // give the others time to (wrongly) start fetches
	release <- struct{}{}
	wg.Wait()
	if hits.Load() != 1 {
		t.Fatalf("cold start: %d fetches, want 1", hits.Load())
	}

	c.feed.mu.Lock()
	c.feed.next = time.Time{} // expire
	c.feed.mu.Unlock()
	slow := make(chan struct{})
	go func() { _, _ = c.Upcoming(now, 3); close(slow) }()
	waitHit(2)
	for range 10 { // the fetch is blocked upstream; these must return from cache at once
		if evs, err := c.Upcoming(now, 3); err != nil || len(evs) != 3 {
			t.Fatalf("during refresh: %d %v", len(evs), err)
		}
	}
	release <- struct{}{}
	<-slow
	if hits.Load() != 2 {
		t.Fatalf("refresh: %d fetches, want 2", hits.Load())
	}
}

// Folded lines are joined in linear time and capped; the event count is capped.
func TestParseICSHostileFeed(t *testing.T) {
	var b strings.Builder
	b.WriteString("BEGIN:VEVENT\r\nDTSTART:20261023T090000Z\r\nSUMMARY:x")
	for range 200_000 {
		b.WriteString("\r\n yyyyyyyyyy")
	}
	b.WriteString("\r\nEND:VEVENT\r\n")
	start := time.Now()
	evs, err := ParseICS(strings.NewReader(b.String()), time.UTC)
	if d := time.Since(start); d > time.Second {
		t.Errorf("200k continuation lines took %v", d)
	}
	if err != nil || len(evs) != 1 || len(evs[0].Summary) > maxICSLine {
		t.Fatalf("got %d events (summary %d bytes), %v", len(evs), len(evs[0].Summary), err)
	}

	b.Reset()
	for range maxICSEvents + 500 {
		b.WriteString("BEGIN:VEVENT\r\nDTSTART:20261023T090000Z\r\nSUMMARY:e\r\nEND:VEVENT\r\n")
	}
	if evs, err := ParseICS(strings.NewReader(b.String()), time.UTC); err != nil || len(evs) != maxICSEvents {
		t.Fatalf("event cap: %d, %v", len(evs), err)
	}
}

// TZID is honoured: 09:00 New York (EDT, UTC-4) is 14:00 in London (BST, UTC+1).
// Unknown zones fall back to the configured location.
func TestParseICSTZID(t *testing.T) {
	london, err := time.LoadLocation("Europe/London")
	if err != nil {
		t.Skip("no tzdata:", err)
	}
	feed := "BEGIN:VEVENT\r\nDTSTART;TZID=America/New_York:20261023T090000\r\nSUMMARY:NY\r\nEND:VEVENT\r\n" +
		"BEGIN:VEVENT\r\nDTSTART;VALUE=DATE-TIME;TZID=\"America/New_York\":20261023T100000\r\nSUMMARY:Quoted\r\nEND:VEVENT\r\n" +
		"BEGIN:VEVENT\r\nDTSTART;TZID=Eastern Standard Time:20261023T090000\r\nSUMMARY:Windows\r\nEND:VEVENT\r\n"
	evs, err := ParseICS(strings.NewReader(feed), london)
	if err != nil || len(evs) != 3 {
		t.Fatalf("got %d events, %v", len(evs), err)
	}
	if want := time.Date(2026, 10, 23, 13, 0, 0, 0, time.UTC); !evs[0].Start.Equal(want) || evs[0].Start.Hour() != 14 {
		t.Errorf("NY: %v, want %v (14:00 London)", evs[0].Start, want)
	}
	if want := time.Date(2026, 10, 23, 14, 0, 0, 0, time.UTC); !evs[1].Start.Equal(want) {
		t.Errorf("quoted: %v, want %v", evs[1].Start, want)
	}
	if want := time.Date(2026, 10, 23, 9, 0, 0, 0, london); !evs[2].Start.Equal(want) {
		t.Errorf("unknown zone: %v, want %v", evs[2].Start, want)
	}
}

// A timed event that is over no longer shows, even today; one still running does.
func TestPickDropsFinishedEvents(t *testing.T) {
	feed := "BEGIN:VEVENT\r\nDTSTART:20261022T080000Z\r\nSUMMARY:Breakfast\r\nEND:VEVENT\r\n" +
		"BEGIN:VEVENT\r\nDTSTART:20261022T110000Z\r\nDTEND:20261022T130000Z\r\nSUMMARY:Workshop\r\nEND:VEVENT\r\n" +
		"BEGIN:VEVENT\r\nDTSTART:20261022T100000Z\r\nDTEND:20261022T113000Z\r\nSUMMARY:Standup\r\nEND:VEVENT\r\n" +
		"BEGIN:VEVENT\r\nDTSTART;VALUE=DATE:20261022\r\nSUMMARY:Holiday\r\nEND:VEVENT\r\n" +
		"BEGIN:VEVENT\r\nDTSTART:20261022T180000Z\r\nSUMMARY:Dinner\r\nEND:VEVENT\r\n"
	evs, err := ParseICS(strings.NewReader(feed), time.UTC)
	if err != nil {
		t.Fatal(err)
	}
	now := time.Date(2026, 10, 22, 12, 0, 0, 0, time.UTC)
	var got []string
	for _, e := range pick(evs, now, 10) {
		got = append(got, e.Summary)
	}
	if strings.Join(got, ",") != "Holiday,Workshop,Dinner" {
		t.Fatalf("pick at noon: %v", got)
	}
}

type fakeWeather struct {
	mu    sync.Mutex
	calls int
	wx    Weather
	err   error
}

func (f *fakeWeather) Current() (Weather, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.calls++
	return f.wx, f.err
}

func (f *fakeWeather) n() int { f.mu.Lock(); defer f.mu.Unlock(); return f.calls }

type fakeCalendar struct {
	evs []Event
	err error
}

func (f fakeCalendar) Upcoming(time.Time, int) ([]Event, error) { return f.evs, f.err }

func newDataServer(t *testing.T, opts Options) (*Server, *httptest.Server) {
	t.Helper()
	st, err := store.Open(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	s := New(st, opts)
	ts := httptest.NewServer(s.Routes())
	t.Cleanup(ts.Close)
	return s, ts
}

func getJSON(t *testing.T, url string) (int, map[string]any) {
	t.Helper()
	resp, body := do(t, "GET", url, "", nil)
	var m map[string]any
	_ = json.Unmarshal(body, &m)
	return resp.StatusCode, m
}

func TestDataEndpoints(t *testing.T) {
	_, off := newDataServer(t, Options{Token: "tok"})
	for _, p := range []string{"/api/data/weather", "/api/data/calendar"} {
		if code, _ := getJSON(t, off.URL+p); code != 404 {
			t.Errorf("%s unconfigured: %d, want 404", p, code)
		}
	}

	start := time.Now().Add(2 * time.Hour)
	_, ts := newDataServer(t, Options{Token: "tok",
		Weather:  &fakeWeather{wx: Weather{TempC: -2.6, Desc: "Light snow"}},
		Calendar: fakeCalendar{evs: []Event{{Start: start, Summary: "Dinner"}}}})
	// Public like the page itself: no token needed.
	code, m := getJSON(t, ts.URL+"/api/data/weather")
	if code != 200 || m["temp"] != "-3" || m["desc"] != "light snow" {
		t.Fatalf("weather: %d %v", code, m)
	}
	code, m = getJSON(t, ts.URL+"/api/data/calendar")
	if l1, _ := m["l1"].(string); code != 200 || !strings.Contains(l1, "Dinner") || m["l2"] != "" {
		t.Fatalf("calendar: %d %v", code, m)
	}

	_, down := newDataServer(t, Options{Weather: &fakeWeather{err: errors.New("down")}, Calendar: fakeCalendar{err: errors.New("down")}})
	if _, m := getJSON(t, down.URL+"/api/data/weather"); m["desc"] != "weather unavailable" {
		t.Errorf("weather down: %v", m)
	}
	if _, m := getJSON(t, down.URL+"/api/data/calendar"); m["l1"] != "calendar unavailable" {
		t.Errorf("calendar down: %v", m)
	}
	_, none := newDataServer(t, Options{Calendar: fakeCalendar{}})
	if _, m := getJSON(t, none.URL+"/api/data/calendar"); m["l1"] != "nothing coming up" {
		t.Errorf("calendar empty: %v", m)
	}
}

// The page gets weather and calendar over SSE on connect and again on every refresh tick.
func TestSSEPushesAndRefreshesData(t *testing.T) {
	wx := &fakeWeather{wx: Weather{TempC: 18.4, Desc: "Overcast"}}
	s, ts := newDataServer(t, Options{Weather: wx, Calendar: fakeCalendar{evs: []Event{{Start: time.Now().Add(time.Hour), Summary: "Run"}}}})
	s.dataEvery = 50 * time.Millisecond
	if _, err := s.Store.Ensure("hall"); err != nil {
		t.Fatal(err)
	}
	if _, err := s.SetLayout("hall", store.Layout{Cols: 4, Rows: 3, Widgets: []store.Widget{
		{ID: "wx", Type: "weather", X: 0, Y: 0, W: 2, H: 1},
		{ID: "cal", Type: "calendar", X: 0, Y: 1, W: 2, H: 1},
	}}); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	req, _ := http.NewRequestWithContext(ctx, "GET", ts.URL+"/api/stream/hall", nil)
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	sc := bufio.NewScanner(resp.Body)
	seen := map[string]int{}
	for sc.Scan() && (seen["wx"] < 2 || seen["cal"] < 1) {
		line, ok := strings.CutPrefix(sc.Text(), "data: ")
		if !ok {
			continue
		}
		var m struct {
			Widget string            `json:"widget"`
			Data   map[string]string `json:"data"`
		}
		_ = json.Unmarshal([]byte(line), &m)
		switch m.Widget {
		case "wx":
			if m.Data["temp"] != "18" || m.Data["desc"] != "overcast" {
				t.Fatalf("weather event %s", line)
			}
		case "cal":
			if !strings.Contains(m.Data["l1"], "Run") {
				t.Fatalf("calendar event %s", line)
			}
		}
		seen[m.Widget]++
	}
	if seen["wx"] < 2 || seen["cal"] < 1 || wx.n() < 2 {
		t.Fatalf("seen %v, weather calls %d (want a refresh after connect)", seen, wx.n())
	}
}

func dialWS(t *testing.T, ts *httptest.Server, id string) *websocket.Conn {
	t.Helper()
	c, _, err := websocket.Dial(context.Background(), "ws"+strings.TrimPrefix(ts.URL, "http")+"/api/frames/"+id+"/ws", nil)
	if err != nil {
		t.Fatal(err)
	}
	return c
}

func TestHelloTimeAndTZ(t *testing.T) {
	t.Setenv("TZ", "Europe/Berlin")
	if got := localTZ(); got != "Europe/Berlin" {
		t.Fatalf("localTZ = %q", got)
	}
	s, ts := newTest(t, "")
	c := dialWS(t, ts, "tz")
	defer closeAndWait(t, s, "tz", c)
	_ = readMsg(t, c) // config
	send(t, c, map[string]any{"type": "hello", "tfctl_version": "0.1"})
	m := readMsg(t, c)
	tu, _ := m["time_unix"].(float64)
	if m["type"] != "hello" || m["tz"] != "Europe/Berlin" || m["server_version"] != "test" {
		t.Fatalf("hello reply %v", m)
	}
	if d := time.Since(time.Unix(int64(tu), 0)); d < -2*time.Second || d > 2*time.Second {
		t.Fatalf("time_unix %v is %v off", tu, d)
	}
}

// S6 server side: config announces software_version and per-file sums; the files and
// their sidecars are served behind the token.
func TestConfigCarriesArtifactVersions(t *testing.T) {
	dir := t.TempDir()
	for name, body := range map[string]string{
		"tfctl-armv7":   "binary",
		"versions.json": `{"software":"0.4.1","system":"2026.10.a","files":{"tfctl":"tfctl-armv7"}}`,
	} {
		if err := os.WriteFile(filepath.Join(dir, name), []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	_, ts := newDataServer(t, Options{Token: "tok", Artifacts: artifacts.New(dir)})
	resp, body := do(t, "GET", ts.URL+"/api/frames/k1/config", "tok", nil)
	var cfg map[string]any
	_ = json.Unmarshal(body, &cfg)
	const sum = "9a3a45d01531a20e89ac6ae10b0b0beb0492acd7216a368aa062d1a5fecaf9cd" // sha256("binary")
	arts, _ := cfg["artifacts"].(map[string]any)
	if resp.StatusCode != 200 || cfg["software_version"] != "0.4.1" || cfg["system_version"] != "2026.10.a" || arts["tfctl"] != "tfctl-armv7:"+sum {
		t.Fatalf("config %d %s", resp.StatusCode, body)
	}
	if resp, _ := do(t, "GET", ts.URL+"/api/artifacts/tfctl-armv7", "", nil); resp.StatusCode != 401 {
		t.Fatalf("artifact without token: %d", resp.StatusCode)
	}
	resp, body = do(t, "GET", ts.URL+"/api/artifacts/tfctl-armv7.sha256", "tok", nil)
	if resp.StatusCode != 200 || string(body) != sum+"  tfctl-armv7\n" {
		t.Fatalf("sidecar %d %q", resp.StatusCode, body)
	}
	if resp, body = do(t, "GET", ts.URL+"/api/artifacts/tfctl-armv7", "tok", nil); resp.StatusCode != 200 || string(body) != "binary" {
		t.Fatalf("artifact %d %q", resp.StatusCode, body)
	}
}
