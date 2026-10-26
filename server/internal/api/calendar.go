package api

import (
	"bufio"
	"fmt"
	"io"
	"net/http"
	"sort"
	"strings"
	"time"
)

// Event is one upcoming calendar entry. End is zero when the feed gives no DTEND.
type Event struct {
	Start   time.Time
	End     time.Time
	AllDay  bool
	Summary string
}

// CalendarSource provides upcoming events.
type CalendarSource interface {
	Upcoming(now time.Time, n int) ([]Event, error)
}

// ICS reads a public iCalendar feed (Google "secret address", Nextcloud, iCloud share).
// Recurring events (RRULE) are not expanded: only each event's own DTSTART counts [U].
// Cached 10 minutes; a failed fetch is retried after 1 to 10 minutes (see refresher).
type ICS struct {
	URL  string
	Loc  *time.Location // for floating and all-day times, and unknown TZIDs; nil = UTC
	feed refresher[[]Event]
}

// Upcoming returns up to n events: all-day events from today on, and timed events that
// have not started or not ended yet.
func (c *ICS) Upcoming(now time.Time, n int) ([]Event, error) {
	evs, err := c.feed.get(10*time.Minute, c.fetch)
	return pick(evs, now, n), err
}

func (c *ICS) fetch() ([]Event, error) {
	cl := http.Client{Timeout: 10 * time.Second}
	resp, err := cl.Get(feedURL(c.URL))
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != 200 {
		return nil, fmt.Errorf("calendar: %s", resp.Status)
	}
	return ParseICS(io.LimitReader(resp.Body, 8<<20), c.Loc)
}

// feedURL turns the webcal:// links Apple Calendar and iCloud hand out into https://.
func feedURL(u string) string {
	if rest, ok := strings.CutPrefix(u, "webcal://"); ok {
		return "https://" + rest
	}
	return u
}

func pick(evs []Event, now time.Time, n int) []Event {
	day := time.Date(now.Year(), now.Month(), now.Day(), 0, 0, 0, 0, now.Location())
	var out []Event
	for _, e := range evs {
		if e.AllDay && !e.Start.Before(day) || !e.AllDay && (!e.Start.Before(now) || e.End.After(now)) {
			out = append(out, e)
		}
	}
	sort.SliceStable(out, func(i, j int) bool { return out[i].Start.Before(out[j].Start) })
	if len(out) > n {
		out = out[:n]
	}
	return out
}

// Limits against hostile or broken feeds: longer logical lines are cut, later events dropped.
const (
	maxICSLine   = 64 << 10
	maxICSEvents = 2000
)

// ParseICS extracts VEVENTs (SUMMARY, DTSTART and DTEND only).
func ParseICS(r io.Reader, loc *time.Location) ([]Event, error) {
	if loc == nil {
		loc = time.UTC
	}
	zones := map[string]*time.Location{} // TZID -> zone; loc when Go does not know it
	var out []Event
	var cur *Event
	handle := func(l string) {
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
				return
			}
			key, params, _ := strings.Cut(name, ";")
			switch key {
			case "SUMMARY":
				cur.Summary = unescapeICS(val)
			case "DTSTART", "DTEND":
				t, allDay, err := parseICSTime(val, params, zoneFor(zones, params, loc), loc)
				switch {
				case err != nil:
				case key == "DTEND":
					cur.End = t
				default:
					cur.Start, cur.AllDay = t, allDay
				}
			}
		}
	}
	sc := bufio.NewScanner(r)
	sc.Buffer(make([]byte, 64<<10), 1<<20)
	var line strings.Builder // the logical line being unfolded
	started := false
	for len(out) < maxICSEvents && sc.Scan() { // unfold: a line starting with space/tab continues the previous one
		l := strings.TrimRight(sc.Text(), "\r")
		if (strings.HasPrefix(l, " ") || strings.HasPrefix(l, "\t")) && started {
			if room := maxICSLine - line.Len(); room > 0 {
				line.WriteString(l[1:min(len(l), room+1)])
			}
			continue
		}
		if started {
			handle(line.String())
		}
		line.Reset()
		line.WriteString(l[:min(len(l), maxICSLine)])
		started = true
	}
	if err := sc.Err(); err != nil {
		return nil, err
	}
	if started && len(out) < maxICSEvents {
		handle(line.String())
	}
	return out, nil
}

// zoneFor resolves a property's TZID parameter, caching each name in zones.
// No TZID, or one Go does not know (Outlook writes Windows names), means loc.
func zoneFor(zones map[string]*time.Location, params string, loc *time.Location) *time.Location {
	for p := range strings.SplitSeq(params, ";") {
		name, ok := strings.CutPrefix(p, "TZID=")
		if !ok {
			continue
		}
		name = strings.Trim(name, `"`)
		z, ok := zones[name]
		if !ok {
			z = loc
			if l, err := time.LoadLocation(name); err == nil && name != "" {
				z = l
			}
			zones[name] = z
		}
		return z
	}
	return loc
}

// parseICSTime reads a DATE (all-day, in loc), a UTC DATE-TIME (suffix Z) or a local
// DATE-TIME in tz, the zone the caller resolved from TZID. Results are shown in loc.
func parseICSTime(v, params string, tz, loc *time.Location) (time.Time, bool, error) {
	if strings.Contains(params, "VALUE=DATE") && !strings.Contains(params, "DATE-TIME") || len(v) == 8 {
		t, err := time.ParseInLocation("20060102", v, loc)
		return t, true, err
	}
	if strings.HasSuffix(v, "Z") {
		t, err := time.Parse("20060102T150405Z", v)
		return t.In(loc), false, err
	}
	t, err := time.ParseInLocation("20060102T150405", v, tz)
	return t.In(loc), false, err
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
