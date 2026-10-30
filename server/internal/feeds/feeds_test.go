package feeds

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"
)

var denver, _ = time.LoadLocation("America/Denver")

// now is 2026-10-30 10:00 in Denver (16:00 UTC).
var now = time.Date(2026, 10, 30, 10, 0, 0, 0, denver)

func imgs(url string) []map[string]string {
	return []map[string]string{{"coverType": "banner", "remoteUrl": "https://b/" + url}, {"coverType": "poster", "remoteUrl": "https://p/" + url}}
}

// fake serves Sonarr, Radarr and open-meteo and records what each was asked.
type fake struct {
	mu   sync.Mutex
	asks []string
	fail string // path prefix to answer 500
}

func (f *fake) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	f.mu.Lock()
	f.asks = append(f.asks, r.URL.Path+"?"+r.URL.RawQuery+" key="+r.Header.Get("X-Api-Key"))
	fail := f.fail
	f.mu.Unlock()
	if fail != "" && strings.HasPrefix(r.URL.Path, fail) {
		http.Error(w, "down", http.StatusInternalServerError)
		return
	}
	var v any
	switch r.URL.Path {
	case "/sonarr/api/v3/calendar":
		v = []map[string]any{
			{"airDateUtc": "2026-10-31T02:00:00Z", "seasonNumber": 4, "episodeNumber": 3, "hasFile": false, "title": "Cease",
				"series": map[string]any{"title": "Tulsa King", "images": imgs("tk"), "runtime": 44}},
			{"airDateUtc": "2026-10-30T07:00:00Z", "seasonNumber": 2, "episodeNumber": 7, "hasFile": true, "runtime": 55, "title": "",
				"series": map[string]any{"title": "MobLand", "images": imgs("mb")}},
			{"airDateUtc": "", "seasonNumber": 1, "episodeNumber": 1},
			{"airDateUtc": "2026-11-02T03:00:00Z", "seasonNumber": 1, "episodeNumber": 2},
		}
	case "/radarr/api/v3/calendar":
		v = []map[string]any{
			{"title": "Dune", "hasFile": true, "images": imgs("dune"),
				"digitalRelease": "2026-10-30T06:00:00Z", "physicalRelease": "2026-11-20T00:00:00Z", "inCinemas": "2026-10-01T00:00:00Z"},
		}
	case "/sonarr/api/v3/history":
		v = map[string]any{"page": 1, "records": []map[string]any{
			{"date": "2026-10-30T15:30:00Z", "seriesId": 1, "series": map[string]any{"title": "MobLand", "images": imgs("mb")},
				"episode": map[string]any{"seasonNumber": 2, "episodeNumber": 7}},
			{"date": "2026-10-30T15:00:00Z", "seriesId": 1, "series": map[string]any{"title": "MobLand"},
				"episode": map[string]any{"seasonNumber": 2, "episodeNumber": 6}},
			{"date": "2026-10-30T08:00:00Z", "seriesId": 2, "series": map[string]any{"title": "Slow Horses"},
				"episode": map[string]any{"seasonNumber": 5, "episodeNumber": 1}},
			{"date": "2026-10-29T20:00:00Z", "seriesId": 3, "series": map[string]any{"title": "A"}, "episode": map[string]any{}},
			{"date": "2026-10-27T20:00:00Z", "seriesId": 4, "series": map[string]any{"title": "B"}, "episode": map[string]any{}},
			{"date": "2026-10-01T20:00:00Z", "seriesId": 5, "series": map[string]any{"title": "C"}, "episode": map[string]any{}},
			{"date": "2026-09-01T20:00:00Z", "seriesId": 6, "series": map[string]any{"title": "D"}, "episode": map[string]any{}},
		}}
	case "/radarr/api/v3/history":
		v = map[string]any{"records": []map[string]any{
			{"date": "2026-10-30T15:59:30.123Z", "movieId": 9, "movie": map[string]any{"title": "Dune", "images": imgs("dune")}},
		}}
	case "/sun":
		v = map[string]any{"daily": map[string]any{"sunrise": []string{"2026-10-30T07:55"}, "sunset": []string{"2026-10-30T18:26"}}}
	default:
		http.NotFound(w, r)
		return
	}
	_ = json.NewEncoder(w).Encode(v)
}

func setup(t *testing.T) (*fake, Config, map[string]json.RawMessage, SetSource) {
	t.Helper()
	f := &fake{}
	srv := httptest.NewServer(f)
	t.Cleanup(srv.Close)
	c := Config{SonarrURL: srv.URL + "/sonarr/", SonarrKey: "sk", RadarrURL: srv.URL + "/radarr", RadarrKey: "rk",
		Lat: 40.4, Lon: -111.8, Loc: denver, SunURL: srv.URL + "/sun"}
	got := map[string]json.RawMessage{}
	var mu sync.Mutex
	set := func(key string, raw []byte) error {
		mu.Lock()
		defer mu.Unlock()
		got[key] = raw
		return nil
	}
	return f, c, got, set
}

func TestRefreshBuildsShowsAndToday(t *testing.T) {
	f, c, got, set := setup(t)
	if err := Refresh(context.Background(), c, now, set); err != nil {
		t.Fatal(err)
	}
	var shows struct {
		Upcoming []map[string]string `json:"upcoming"`
		Added    []map[string]string `json:"added"`
		Updated  string              `json:"updated"`
	}
	if err := json.Unmarshal(got["shows"], &shows); err != nil {
		t.Fatal(err)
	}
	wantUp := []string{
		"2026-10-30 Dune Movie · Digital · ✓ https://p/dune",
		"2026-10-30 MobLand S2·E7 · 1:00 AM · ✓ https://p/mb",
		"2026-10-30 Tulsa King S4·E3 · 8:00 PM https://p/tk",
		"2026-11-01 Episode S1·E2 · 8:00 PM ",
	}
	if len(shows.Upcoming) != len(wantUp) {
		t.Fatalf("upcoming = %v", shows.Upcoming)
	}
	for i, u := range shows.Upcoming {
		if s := u["date"] + " " + u["title"] + " " + u["sub"] + " " + u["image"]; s != wantUp[i] {
			t.Errorf("upcoming[%d] = %q; want %q", i, s, wantUp[i])
		}
	}
	wantAdded := []string{"Dune Movie · 1 min ago", "MobLand S2·E7 · 30 min ago", "Slow Horses S5·E1 · 8 h ago",
		"A S0·E0 · Yesterday", "B S0·E0 · Tue", "C S0·E0 · Oct 1"}
	if len(shows.Added) != len(wantAdded) {
		t.Fatalf("added = %v", shows.Added)
	}
	for i, a := range shows.Added {
		if s := a["title"] + " " + a["when"]; s != wantAdded[i] {
			t.Errorf("added[%d] = %q; want %q", i, s, wantAdded[i])
		}
	}
	if shows.Updated != "2026-10-30T10:00:00-06:00" {
		t.Errorf("updated = %q", shows.Updated)
	}

	var today struct{ Events []Event }
	if err := json.Unmarshal(got["today"], &today); err != nil {
		t.Fatal(err)
	}
	wantToday := []Event{
		{"2026-10-30T01:00:00-06:00", "2026-10-30T01:55:00-06:00", "MobLand S2·E7", "New episode"},
		{"2026-10-30T07:55:00-06:00", "2026-10-30T07:55:00-06:00", "Sunrise", ""},
		{"2026-10-30T09:00:00-06:00", "", "Dune", "Digital release"},
		{"2026-10-30T18:26:00-06:00", "2026-10-30T18:26:00-06:00", "Sunset", ""},
		{"2026-10-30T20:00:00-06:00", "2026-10-30T20:44:00-06:00", "Tulsa King S4·E3", "Cease"},
	}
	if len(today.Events) != len(wantToday) {
		t.Fatalf("today = %+v", today.Events)
	}
	for i, e := range today.Events {
		if e != wantToday[i] {
			t.Errorf("today[%d] = %+v; want %+v", i, e, wantToday[i])
		}
	}

	asks := strings.Join(f.asks, "\n")
	for _, want := range []string{
		"/sonarr/api/v3/calendar?end=2026-11-06T07%3A00%3A00%2B00%3A00&includeSeries=true&start=2026-10-30T06%3A00%3A00%2B00%3A00&unmonitored=false key=sk",
		"/radarr/api/v3/calendar?end=2026-11-06T07%3A00%3A00%2B00%3A00&start=2026-10-30T06%3A00%3A00%2B00%3A00&unmonitored=false key=rk",
		"eventType=3&includeEpisode=true&includeSeries=true&page=1&pageSize=40&sortDirection=descending&sortKey=date key=sk",
		"eventType=3&includeMovie=true&page=1&pageSize=20&sortDirection=descending&sortKey=date key=rk",
		"/sun?daily=sunrise%2Csunset&forecast_days=1&latitude=40.4&longitude=-111.8&timezone=America%2FDenver key=",
	} {
		if !strings.Contains(asks, want) {
			t.Errorf("never asked %s\nasked:\n%s", want, asks)
		}
	}
}

func TestMovieAtMidnightShowsAtNine(t *testing.T) {
	f := &fake{}
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/radarr/api/v3/calendar" {
			_, _ = w.Write([]byte(`[{"inCinemas":"2026-10-30T06:00:00Z"}]`))
			return
		}
		f.ServeHTTP(w, r)
	}))
	defer srv.Close()
	_, c, got, set := setup(t)
	c.RadarrURL, c.SunURL = srv.URL+"/radarr", srv.URL+"/sun"
	if err := Refresh(context.Background(), c, now, set); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(got["today"]), `{"start":"2026-10-30T09:00:00-06:00","end":"","title":"Movie","where":"In theaters release"}`) {
		t.Fatalf("today = %s", got["today"])
	}
}

func TestRefreshReportsEachUpstream(t *testing.T) {
	for _, tc := range []struct{ fail, want string }{
		{"/sonarr/api/v3/calendar", "sonarr calendar: HTTP 500"},
		{"/radarr/api/v3/calendar", "radarr calendar: HTTP 500"},
		{"/sonarr/api/v3/history", "sonarr history: HTTP 500"},
		{"/radarr/api/v3/history", "radarr history: HTTP 500"},
		{"/sun", "sun times: HTTP 500"},
	} {
		f, c, got, set := setup(t)
		f.fail = tc.fail
		err := Refresh(context.Background(), c, now, set)
		if err == nil || err.Error() != tc.want {
			t.Errorf("fail %s: err = %v; want %q", tc.fail, err, tc.want)
		}
		if _, ok := got["today"]; ok {
			t.Errorf("fail %s: today stored anyway", tc.fail)
		}
	}
}

func TestRedactKeepsKeysOutOfErrors(t *testing.T) {
	c := Config{SonarrURL: "http://127.0.0.1:1/sk-secret-value", SonarrKey: "sk-secret-value", RadarrURL: "x", RadarrKey: "y", Loc: denver}
	err := Refresh(context.Background(), c, now, func(string, []byte) error { return nil })
	if err == nil || strings.Contains(err.Error(), "sk-secret-value") {
		t.Fatalf("err = %v", err)
	}
}

func TestEnabledNeedsEverySetting(t *testing.T) {
	full := Config{SonarrURL: "a", SonarrKey: "b", RadarrURL: "c", RadarrKey: "d"}
	if !full.Enabled() {
		t.Fatal("full config disabled")
	}
	for _, c := range []Config{{}, {SonarrURL: "a", SonarrKey: "b", RadarrURL: "c"}} {
		if c.Enabled() {
			t.Errorf("%+v enabled", c)
		}
	}
}

func TestRunStoresThenStops(t *testing.T) {
	_, c, got, set := setup(t)
	ctx, cancel := context.WithCancel(context.Background())
	stored := make(chan struct{}, 4)
	done := make(chan struct{})
	go func() {
		Run(ctx, c, time.Hour, func(k string, b []byte) error {
			err := set(k, b)
			stored <- struct{}{}
			return err
		})
		close(done)
	}()
	<-stored
	<-stored
	cancel()
	<-done
	if len(got) != 2 {
		t.Fatalf("stored %v", got)
	}
}
