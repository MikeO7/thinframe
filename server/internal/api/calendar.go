package api

import (
	"bufio"
	"errors"
	"fmt"
	"io"
	"log"
	"net/http"
	"sort"
	"strings"
	"sync"
	"time"
	"unicode"
)

// Event is one upcoming calendar entry. End is zero when the feed gives no DTEND.
type Event struct {
	Start   time.Time
	End     time.Time
	AllDay  bool
	Summary string
	rec     *recurrence // set on a recurring event as parsed; expand turns it into occurrences
}

// CalendarSource provides upcoming events.
type CalendarSource interface {
	Upcoming(now time.Time, n int) ([]Event, error)
}

// ICS reads a public iCalendar feed (Google "secret address", Nextcloud, iCloud share).
// Recurring events are expanded on every call (see rrule.go), so the cached feed stays right.
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
	return pick(expand(evs, now), now, n), err
}

// FeedURLs splits CALENDAR_ICS_URL: one or more feeds separated by spaces, commas or newlines.
func FeedURLs(list string) []string {
	return strings.FieldsFunc(list, func(r rune) bool { return r == ',' || unicode.IsSpace(r) })
}

// ICSFeeds is the calendar for these feed URLs: one ICS, or Calendars merging several.
func ICSFeeds(urls []string, loc *time.Location) CalendarSource {
	if len(urls) == 1 {
		return &ICS{URL: urls[0], Loc: loc}
	}
	cs := make(Calendars, len(urls))
	for i, u := range urls {
		cs[i] = &ICS{URL: u, Loc: loc}
	}
	return cs
}

// Calendars merges several sources, each with its own cache, sorted by start. A source
// that fails is logged and the others still show; the error comes back only when all fail.
type Calendars []CalendarSource

// Upcoming asks every source at once and keeps the first n events of all of them.
func (cs Calendars) Upcoming(now time.Time, n int) ([]Event, error) {
	evs := make([][]Event, len(cs))
	errs := make([]error, len(cs))
	var wg sync.WaitGroup
	for i, c := range cs {
		wg.Go(func() { evs[i], errs[i] = c.Upcoming(now, n) })
	}
	wg.Wait()
	var out []Event
	failed := 0
	for i := range cs {
		out = append(out, evs[i]...)
		if errs[i] != nil {
			failed++
			errs[i] = fmt.Errorf("feed %d: %w", i+1, errs[i])
		}
	}
	sort.SliceStable(out, func(i, j int) bool { return out[i].Start.Before(out[j].Start) })
	if len(out) > n {
		out = out[:n]
	}
	if failed > 0 && failed == len(cs) {
		return out, errors.Join(errs...)
	}
	for _, err := range errs {
		if err != nil {
			log.Printf("calendar: %s", redactErr(err))
		}
	}
	return out, nil
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

// vevent is a VEVENT while it is read: properties come in any order.
type vevent struct {
	Event
	uid, rrule, status, duration string
	tz                           *time.Location // DTSTART's zone
	exAt                         []time.Time
	exDay                        []string
	recurID                      time.Time // set on an override of one occurrence
}

// ParseICS extracts VEVENTs: SUMMARY, DTSTART, DTEND or DURATION, and for recurring events
// RRULE, EXDATE and the RECURRENCE-ID overrides. A recurring event comes back once, at its
// DTSTART, carrying its rule; Upcoming expands it. Cancelled events and occurrences are dropped.
func ParseICS(r io.Reader, loc *time.Location) ([]Event, error) {
	if loc == nil {
		loc = time.UTC
	}
	zones := map[string]*time.Location{} // TZID -> zone; loc when Go does not know it
	var out []Event
	masters := map[string]int{}           // UID -> index in out of the recurring event
	overrides := map[string][]time.Time{} // UID -> RECURRENCE-IDs
	var cur *vevent
	finish := func(v *vevent) {
		if !v.recurID.IsZero() { // even a cancellation without DTSTART removes its occurrence
			overrides[v.uid] = append(overrides[v.uid], v.recurID)
		}
		if v.Start.IsZero() {
			return
		}
		if v.End.IsZero() && v.duration != "" {
			v.End = addICSDuration(v.Start, v.duration)
		}
		cancelled := v.status == "CANCELLED"
		switch {
		case !v.recurID.IsZero(), cancelled: // an override shows as its own event
		case v.rrule != "":
			if rec, ok := parseRRULE(v.rrule, v.tz); ok {
				for _, t := range v.exAt {
					rec.exAt[t.Unix()] = true
				}
				for _, d := range v.exDay {
					rec.exDay[d] = true
				}
				v.rec = rec
				if v.uid != "" {
					masters[v.uid] = len(out)
				}
			}
		}
		if !cancelled {
			out = append(out, v.Event)
		}
	}
	handle := func(l string) {
		switch {
		case l == "BEGIN:VEVENT":
			cur = &vevent{}
		case l == "END:VEVENT":
			if cur != nil {
				finish(cur)
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
			case "UID":
				cur.uid = val
			case "RRULE":
				cur.rrule = val
			case "STATUS":
				cur.status = strings.ToUpper(strings.TrimSpace(val))
			case "DURATION":
				cur.duration = strings.TrimSpace(val)
			case "EXDATE":
				zone := zoneFor(zones, params, loc)
				for v := range strings.SplitSeq(val, ",") {
					switch t, allDay, err := parseICSTime(v, params, zone, loc); {
					case err != nil:
					case allDay:
						cur.exDay = append(cur.exDay, t.Format("20060102"))
					default:
						cur.exAt = append(cur.exAt, t)
					}
				}
			case "RECURRENCE-ID":
				if t, _, err := parseICSTime(val, params, zoneFor(zones, params, loc), loc); err == nil {
					cur.recurID = t
				}
			case "DTSTART", "DTEND":
				zone := zoneFor(zones, params, loc)
				t, allDay, err := parseICSTime(val, params, zone, loc)
				switch {
				case err != nil:
				case key == "DTEND":
					cur.End = t
				default:
					cur.Start, cur.AllDay, cur.tz = t, allDay, zone
					if allDay {
						cur.tz = loc
					} else if strings.HasSuffix(val, "Z") {
						cur.tz = time.UTC
					}
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
	for uid, ids := range overrides { // an override replaces its occurrence, or cancels it
		if i, ok := masters[uid]; ok {
			for _, t := range ids {
				out[i].rec.exAt[t.Unix()] = true
			}
		}
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
