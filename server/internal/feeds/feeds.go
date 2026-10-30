// Package feeds builds the sources "shows" and "today" from Sonarr, Radarr and open-meteo
// inside the server, so no outside scheduler has to push them. It is a port of the Hermes
// script thinframe_feeds.py; the JSON it writes has the same shape.
//
//	shows  upcoming: Sonarr and Radarr calendar for the next 7 days; added: the newest
//	       imports, one card per show or movie, at most 6
//	today  episodes and movies releasing today, plus sunrise and sunset
package feeds

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net/http"
	"net/url"
	"sort"
	"strings"
	"time"
)

// Config is where the feeds come from. Both *arr services are required.
type Config struct {
	SonarrURL, SonarrKey string
	RadarrURL, RadarrKey string
	Lat, Lon             float64        // sunrise and sunset
	Loc                  *time.Location // times on the frame; nil = UTC
	SunURL               string         // empty = open-meteo; tests point it elsewhere
	Client               *http.Client   // nil = 20 s timeout
}

// Enabled reports whether every *arr setting is present.
func (c Config) Enabled() bool {
	return c.SonarrURL != "" && c.SonarrKey != "" && c.RadarrURL != "" && c.RadarrKey != ""
}

// SetSource stores one source (api.Server.SetSource).
type SetSource func(key string, raw []byte) error

// Run refreshes both sources now and then every interval until ctx ends. It logs when the
// refresh starts failing and when it works again, not on every attempt.
func Run(ctx context.Context, c Config, every time.Duration, set SetSource) {
	failing := ""
	for {
		err := Refresh(ctx, c, time.Now(), set)
		if ctx.Err() != nil {
			return
		}
		switch msg := errText(err); {
		case msg != "" && msg != failing:
			log.Printf("feeds: shows/today failed: %s", msg)
		case msg == "" && failing != "":
			log.Printf("feeds: shows/today working again")
		}
		failing = errText(err)
		select {
		case <-ctx.Done():
			return
		case <-time.After(every):
		}
	}
}

func errText(err error) string {
	if err == nil {
		return ""
	}
	s := err.Error()
	if len(s) > 200 {
		s = s[:200]
	}
	return s
}

// Refresh builds and stores "shows" and then "today" as of now.
func Refresh(ctx context.Context, c Config, now time.Time, set SetSource) error {
	if c.Loc == nil {
		c.Loc = time.UTC
	}
	if c.Client == nil {
		c.Client = &http.Client{Timeout: 20 * time.Second}
	}
	f := fetcher{c: c, ctx: ctx, now: now.In(c.Loc)}
	shows, today, err := f.build()
	if err != nil {
		return err
	}
	if err := put(set, "shows", shows); err != nil {
		return err
	}
	sun, err := f.sun()
	if err != nil {
		return err
	}
	today = append(today, sun...)
	sort.SliceStable(today, func(i, j int) bool { return today[i].Start < today[j].Start })
	return put(set, "today", map[string]any{"events": today})
}

func put(set SetSource, key string, v any) error {
	b, err := json.Marshal(v)
	if err != nil {
		return err
	}
	if err := set(key, b); err != nil {
		return fmt.Errorf("store %s: %w", key, err)
	}
	return nil
}

type fetcher struct {
	c   Config
	ctx context.Context
	now time.Time
}

// Upcoming is one calendar card.
type Upcoming struct {
	Date  string `json:"date"`
	Title string `json:"title"`
	Sub   string `json:"sub"`
	Image string `json:"image"`
	t     time.Time
}

// Added is one recently imported show or movie.
type Added struct {
	Title string `json:"title"`
	When  string `json:"when"`
	Image string `json:"image"`
	key   string
	t     time.Time
}

// Event is one line of the today widget.
type Event struct {
	Start string `json:"start"`
	End   string `json:"end"`
	Title string `json:"title"`
	Where string `json:"where"`
}

type image struct {
	CoverType string `json:"coverType"`
	RemoteURL string `json:"remoteUrl"`
}

type series struct {
	Title   string  `json:"title"`
	Images  []image `json:"images"`
	Runtime int     `json:"runtime"`
}

type episode struct {
	AirDateUTC    string  `json:"airDateUtc"`
	SeasonNumber  int     `json:"seasonNumber"`
	EpisodeNumber int     `json:"episodeNumber"`
	HasFile       bool    `json:"hasFile"`
	Runtime       int     `json:"runtime"`
	Title         string  `json:"title"`
	Series        *series `json:"series"`
}

type movie struct {
	Title           string  `json:"title"`
	Images          []image `json:"images"`
	HasFile         bool    `json:"hasFile"`
	DigitalRelease  string  `json:"digitalRelease"`
	PhysicalRelease string  `json:"physicalRelease"`
	InCinemas       string  `json:"inCinemas"`
}

type historyRecord struct {
	Date     string   `json:"date"`
	SeriesID int      `json:"seriesId"`
	MovieID  int      `json:"movieId"`
	Series   *series  `json:"series"`
	Episode  *episode `json:"episode"`
	Movie    *movie   `json:"movie"`
}

func (f fetcher) build() (map[string]any, []Event, error) {
	start := time.Date(f.now.Year(), f.now.Month(), f.now.Day(), 0, 0, 0, 0, f.c.Loc)
	end := start.AddDate(0, 0, 7)
	rng := url.Values{"start": {utcISO(start)}, "end": {utcISO(end)}, "unmonitored": {"false"}}
	var up []Upcoming
	var today []Event

	var eps []episode
	epq := cloneValues(rng)
	epq.Set("includeSeries", "true")
	if err := f.get(f.c.SonarrURL, f.c.SonarrKey, "/api/v3/calendar", epq, &eps); err != nil {
		return nil, nil, fmt.Errorf("sonarr calendar: %w", err)
	}
	for _, e := range eps {
		if e.AirDateUTC == "" {
			continue
		}
		t, err := f.local(e.AirDateUTC)
		if err != nil {
			continue
		}
		s := series{Title: "Episode"}
		if e.Series != nil {
			s = *e.Series
		}
		code := fmt.Sprintf("S%d·E%d", e.SeasonNumber, e.EpisodeNumber)
		sub := code + " · " + hm(t)
		if e.HasFile {
			sub += " · ✓"
		}
		up = append(up, Upcoming{Date: t.Format(time.DateOnly), Title: orDefault(s.Title, "Episode"), Sub: sub, Image: poster(s.Images), t: t})
		if sameDay(t, f.now) {
			run := e.Runtime
			if run == 0 {
				run = s.Runtime
			}
			if run == 0 {
				run = 30
			}
			where := e.Title
			if where == "" {
				where = "New episode"
			}
			today = append(today, Event{Start: iso(t), End: iso(t.Add(time.Duration(run) * time.Minute)),
				Title: orDefault(s.Title, "Episode") + " " + code, Where: where})
		}
	}

	var movies []movie
	if err := f.get(f.c.RadarrURL, f.c.RadarrKey, "/api/v3/calendar", rng, &movies); err != nil {
		return nil, nil, fmt.Errorf("radarr calendar: %w", err)
	}
	for _, m := range movies {
		for _, r := range []struct{ when, label string }{
			{m.DigitalRelease, "Digital"}, {m.PhysicalRelease, "Disc"}, {m.InCinemas, "In theaters"}} {
			if r.when == "" {
				continue
			}
			t, err := f.local(r.when)
			if err != nil || t.Before(start) || !t.Before(end) {
				continue
			}
			sub := "Movie · " + r.label
			if m.HasFile {
				sub += " · ✓"
			}
			up = append(up, Upcoming{Date: t.Format(time.DateOnly), Title: orDefault(m.Title, "Movie"), Sub: sub, Image: poster(m.Images), t: t})
			if sameDay(t, f.now) {
				st := t
				if t.Hour() == 0 {
					st = time.Date(t.Year(), t.Month(), t.Day(), 9, 0, t.Second(), t.Nanosecond(), t.Location())
				}
				today = append(today, Event{Start: iso(st), Title: orDefault(m.Title, "Movie"), Where: r.label + " release"})
			}
		}
	}
	sort.SliceStable(up, func(i, j int) bool { return up[i].t.Before(up[j].t) })
	if up == nil {
		up = []Upcoming{}
	}

	added, err := f.added()
	if err != nil {
		return nil, nil, err
	}
	return map[string]any{"upcoming": up, "added": added, "updated": iso(f.now)}, today, nil
}

func (f fetcher) added() ([]Added, error) {
	var all []Added
	for _, h := range []struct {
		name, base, key string
		q               url.Values
		tv              bool
	}{
		{"sonarr", f.c.SonarrURL, f.c.SonarrKey, history(40, "includeSeries", "includeEpisode"), true},
		{"radarr", f.c.RadarrURL, f.c.RadarrKey, history(20, "includeMovie"), false},
	} {
		var page struct {
			Records []historyRecord `json:"records"`
		}
		if err := f.get(h.base, h.key, "/api/v3/history", h.q, &page); err != nil {
			return nil, fmt.Errorf("%s history: %w", h.name, err)
		}
		for _, r := range page.Records {
			t, err := f.local(r.Date)
			if err != nil {
				continue
			}
			if h.tv {
				s, e := series{Title: "Episode"}, episode{}
				if r.Series != nil {
					s = *r.Series
				}
				if r.Episode != nil {
					e = *r.Episode
				}
				all = append(all, Added{key: fmt.Sprintf("tv%d", r.SeriesID), Title: orDefault(s.Title, "Episode"),
					When: fmt.Sprintf("S%d·E%d · %s", e.SeasonNumber, e.EpisodeNumber, ago(t, f.now)), Image: poster(s.Images), t: t})
				continue
			}
			m := movie{Title: "Movie"}
			if r.Movie != nil {
				m = *r.Movie
			}
			all = append(all, Added{key: fmt.Sprintf("mv%d", r.MovieID), Title: orDefault(m.Title, "Movie"),
				When: "Movie · " + ago(t, f.now), Image: poster(m.Images), t: t})
		}
	}
	sort.SliceStable(all, func(i, j int) bool { return all[i].t.After(all[j].t) })
	out, seen := []Added{}, map[string]bool{}
	for _, a := range all { // one card per show or movie: its newest import
		if seen[a.key] {
			continue
		}
		seen[a.key] = true
		out = append(out, a)
		if len(out) == 6 {
			break
		}
	}
	return out, nil
}

func history(size int, include ...string) url.Values {
	q := url.Values{"page": {"1"}, "pageSize": {fmt.Sprint(size)}, "sortKey": {"date"},
		"sortDirection": {"descending"}, "eventType": {"3"}}
	for _, i := range include {
		q.Set(i, "true")
	}
	return q
}

// sun is today's sunrise and sunset from open-meteo.
func (f fetcher) sun() ([]Event, error) {
	base := f.c.SunURL
	if base == "" {
		base = "https://api.open-meteo.com/v1/forecast"
	}
	q := url.Values{"latitude": {fmt.Sprint(f.c.Lat)}, "longitude": {fmt.Sprint(f.c.Lon)},
		"daily": {"sunrise,sunset"}, "timezone": {f.c.Loc.String()}, "forecast_days": {"1"}}
	var d struct {
		Daily struct {
			Sunrise []string `json:"sunrise"`
			Sunset  []string `json:"sunset"`
		} `json:"daily"`
	}
	if err := f.getJSON(base+"?"+q.Encode(), nil, &d); err != nil {
		return nil, fmt.Errorf("sun times: %w", err)
	}
	var out []Event
	for _, s := range []struct {
		name string
		v    []string
	}{{"Sunrise", d.Daily.Sunrise}, {"Sunset", d.Daily.Sunset}} {
		if len(s.v) == 0 {
			return nil, fmt.Errorf("sun times: no %s", strings.ToLower(s.name))
		}
		t, err := time.ParseInLocation("2006-01-02T15:04", s.v[0], f.c.Loc)
		if err != nil {
			return nil, fmt.Errorf("sun times: %w", err)
		}
		out = append(out, Event{Start: iso(t), End: iso(t), Title: s.name})
	}
	return out, nil
}

func (f fetcher) get(base, key, path string, q url.Values, v any) error {
	return f.getJSON(strings.TrimRight(base, "/")+path+"?"+q.Encode(), http.Header{"X-Api-Key": {key}}, v)
}

func (f fetcher) getJSON(u string, h http.Header, v any) error {
	req, err := http.NewRequestWithContext(f.ctx, http.MethodGet, u, nil)
	if err != nil {
		return err
	}
	for k, vs := range h {
		req.Header[k] = vs
	}
	req.Header.Set("Accept", "application/json")
	resp, err := f.c.Client.Do(req)
	if err != nil {
		return redact(err, h)
	}
	defer resp.Body.Close()
	if resp.StatusCode/100 != 2 {
		return fmt.Errorf("HTTP %d", resp.StatusCode)
	}
	return json.NewDecoder(io.LimitReader(resp.Body, 16<<20)).Decode(v)
}

// redact keeps an API key out of an error that quotes the URL. Keys travel in a header, so
// this is belt and braces.
func redact(err error, h http.Header) error {
	s := err.Error()
	for _, k := range h.Values("X-Api-Key") {
		if k != "" {
			s = strings.ReplaceAll(s, k, "[key]")
		}
	}
	return fmt.Errorf("%s", s)
}

func (f fetcher) local(s string) (time.Time, error) {
	t, err := time.Parse(time.RFC3339, s)
	if err != nil {
		return time.Time{}, err
	}
	return t.In(f.c.Loc), nil
}

func poster(images []image) string {
	for _, kind := range []string{"poster", "fanart", "banner"} {
		for _, i := range images {
			if i.CoverType == kind && strings.HasPrefix(i.RemoteURL, "http") {
				return i.RemoteURL
			}
		}
	}
	return ""
}

func hm(t time.Time) string { return t.Format("3:04 PM") }

func iso(t time.Time) string { return t.Format(time.RFC3339) }

func utcISO(t time.Time) string { return t.UTC().Format("2006-01-02T15:04:05+00:00") }

func sameDay(a, b time.Time) bool {
	ay, am, ad := a.Date()
	by, bm, bd := b.Date()
	return ay == by && am == bm && ad == bd
}

// days is the number of calendar days from a to b in b's zone.
func days(a, b time.Time) int {
	da := time.Date(a.Year(), a.Month(), a.Day(), 0, 0, 0, 0, time.UTC)
	db := time.Date(b.Year(), b.Month(), b.Day(), 0, 0, 0, 0, time.UTC)
	return int(db.Sub(da).Hours() / 24)
}

func ago(t, now time.Time) string {
	s := now.Sub(t).Seconds()
	switch d := days(t, now); {
	case s < 3600:
		return fmt.Sprintf("%d min ago", max(1, int(s/60)))
	case s < 86400 && d == 0:
		return fmt.Sprintf("%d h ago", int(s/3600))
	case d == 1:
		return "Yesterday"
	case d < 7:
		return t.Format("Mon")
	default:
		return t.Format("Jan 2")
	}
}

func orDefault(s, def string) string {
	if s == "" {
		return def
	}
	return s
}

func cloneValues(v url.Values) url.Values {
	out := url.Values{}
	for k, vs := range v {
		out[k] = append([]string(nil), vs...)
	}
	return out
}
