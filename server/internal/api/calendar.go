package api

import (
	"bufio"
	"fmt"
	"io"
	"net/http"
	"sort"
	"strings"
	"sync"
	"time"
)

// Event is one upcoming calendar entry.
type Event struct {
	Start   time.Time
	AllDay  bool
	Summary string
}

// CalendarSource provides upcoming events.
type CalendarSource interface {
	Upcoming(now time.Time, n int) ([]Event, error)
}

// ICS reads a public iCalendar feed (Google "secret address", Nextcloud, iCloud share).
// Recurring events (RRULE) are not expanded: only each event's own DTSTART counts [U].
// Cached 10 minutes.
type ICS struct {
	URL string
	Loc *time.Location // for floating and all-day times; nil = UTC
	mu  sync.Mutex
	got []Event
	at  time.Time
}

// Upcoming returns up to n events starting at or after the start of today (so all-day events show).
func (c *ICS) Upcoming(now time.Time, n int) ([]Event, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if time.Since(c.at) >= 10*time.Minute {
		cl := http.Client{Timeout: 10 * time.Second}
		resp, err := cl.Get(c.URL)
		if err != nil {
			return pick(c.got, now, n), err
		}
		defer resp.Body.Close()
		if resp.StatusCode != 200 {
			return pick(c.got, now, n), fmt.Errorf("calendar: %s", resp.Status)
		}
		evs, err := ParseICS(io.LimitReader(resp.Body, 8<<20), c.Loc)
		if err != nil {
			return pick(c.got, now, n), err
		}
		c.got, c.at = evs, time.Now()
	}
	return pick(c.got, now, n), nil
}

func pick(evs []Event, now time.Time, n int) []Event {
	day := time.Date(now.Year(), now.Month(), now.Day(), 0, 0, 0, 0, now.Location())
	var out []Event
	for _, e := range evs {
		if !e.Start.Before(day) && (e.AllDay || !e.Start.Before(now) || e.Start.After(day)) {
			out = append(out, e)
		}
	}
	sort.SliceStable(out, func(i, j int) bool { return out[i].Start.Before(out[j].Start) })
	if len(out) > n {
		out = out[:n]
	}
	return out
}

// ParseICS extracts VEVENTs (SUMMARY and DTSTART only).
func ParseICS(r io.Reader, loc *time.Location) ([]Event, error) {
	if loc == nil {
		loc = time.UTC
	}
	sc := bufio.NewScanner(r)
	sc.Buffer(make([]byte, 64<<10), 1<<20)
	var lines []string
	for sc.Scan() { // unfold: a line starting with space/tab continues the previous one
		l := strings.TrimRight(sc.Text(), "\r")
		if (strings.HasPrefix(l, " ") || strings.HasPrefix(l, "\t")) && len(lines) > 0 {
			lines[len(lines)-1] += l[1:]
		} else {
			lines = append(lines, l)
		}
	}
	if err := sc.Err(); err != nil {
		return nil, err
	}
	var out []Event
	var cur *Event
	for _, l := range lines {
		switch {
		case l == "BEGIN:VEVENT":
			cur = &Event{}
		case l == "END:VEVENT":
			if cur != nil && !cur.Start.IsZero() {
				out = append(out, *cur)
			}
			cur = nil
		case cur != nil:
			name, val, ok := strings.Cut(l, ":")
			if !ok {
				continue
			}
			key, params, _ := strings.Cut(name, ";")
			switch key {
			case "SUMMARY":
				cur.Summary = unescapeICS(val)
			case "DTSTART":
				t, allDay, err := parseICSTime(val, params, loc)
				if err == nil {
					cur.Start, cur.AllDay = t, allDay
				}
			}
		}
	}
	return out, nil
}

func parseICSTime(v, params string, loc *time.Location) (time.Time, bool, error) {
	if strings.Contains(params, "VALUE=DATE") && !strings.Contains(params, "DATE-TIME") || len(v) == 8 {
		t, err := time.ParseInLocation("20060102", v, loc)
		return t, true, err
	}
	if strings.HasSuffix(v, "Z") {
		t, err := time.Parse("20060102T150405Z", v)
		return t.In(loc), false, err
	}
	t, err := time.ParseInLocation("20060102T150405", v, loc)
	return t, false, err
}

func unescapeICS(s string) string {
	r := strings.NewReplacer(`\n`, " ", `\N`, " ", `\,`, ",", `\;`, ";", `\\`, `\`)
	return r.Replace(s)
}

// Line renders an event for the widget: "Today 18:30  Dinner", "Fri  Dentist".
func (e Event) Line(now time.Time) string {
	day := e.Start.Format("Mon")
	switch {
	case sameDay(e.Start, now):
		day = "Today"
	case sameDay(e.Start, now.AddDate(0, 0, 1)):
		day = "Tomorrow"
	}
	if e.AllDay {
		return day + "  " + e.Summary
	}
	return day + " " + e.Start.Format("15:04") + "  " + e.Summary
}

func sameDay(a, b time.Time) bool {
	ay, am, ad := a.Date()
	by, bm, bd := b.In(a.Location()).Date()
	return ay == by && am == bm && ad == bd
}
