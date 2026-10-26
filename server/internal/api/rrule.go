package api

import (
	"math"
	"slices"
	"strconv"
	"strings"
	"time"
)

// Recurring events (RFC 5545 RRULE) are expanded when the widget asks, not when the feed
// is fetched, so a cached feed stays right as days pass. Only occurrences that can be on
// screen are made: from a day before now to 60 days after.
const (
	recurBack  = 24 * time.Hour
	recurAhead = 60 * 24 * time.Hour
	maxPeriods = 5000 // days, weeks, months or years walked per rule, so no rule can spin
)

// recurrence is a supported RRULE plus the occurrences removed from it: EXDATE, and the
// RECURRENCE-ID of every override (which shows as its own event, or not at all when cancelled).
type recurrence struct {
	freq       string // DAILY, WEEKLY, MONTHLY or YEARLY
	interval   int
	count      int       // 0 = no COUNT
	until      time.Time // zero = no UNTIL
	byDay      []weekdayNum
	byMonthDay []int // negative counts from the end of the month
	byMonth    []time.Month
	bySetPos   []int           // keeps only these of each period's days: 1 = first, -1 = last
	tz         *time.Location  // the rule repeats in DTSTART's zone, so 9:00 stays 9:00 across DST
	exAt       map[int64]bool  // removed instants, Unix seconds
	exDay      map[string]bool // removed days (EXDATE;VALUE=DATE), YYYYMMDD in tz
}

// weekdayNum is one BYDAY entry: MO is every Monday, 2TU the second Tuesday, -1FR the last Friday.
type weekdayNum struct {
	n  int
	wd time.Weekday
}

var icsWeekdays = map[string]time.Weekday{"SU": time.Sunday, "MO": time.Monday, "TU": time.Tuesday,
	"WE": time.Wednesday, "TH": time.Thursday, "FR": time.Friday, "SA": time.Saturday}

// parseRRULE reads an RRULE value. ok is false for a rule part this code does not support
// (BYWEEKNO, BYYEARDAY, HOURLY, ...) or a broken value: the event then shows once, at DTSTART.
func parseRRULE(v string, tz *time.Location) (*recurrence, bool) {
	r := &recurrence{interval: 1, tz: tz, exAt: map[int64]bool{}, exDay: map[string]bool{}}
	for part := range strings.SplitSeq(v, ";") {
		k, val, _ := strings.Cut(part, "=")
		good := true
		var err error
		switch strings.ToUpper(k) {
		case "FREQ":
			r.freq = strings.ToUpper(val)
		case "INTERVAL":
			r.interval, err = strconv.Atoi(val)
			good = err == nil && r.interval >= 1
		case "COUNT":
			r.count, err = strconv.Atoi(val)
			good = err == nil && r.count >= 1
		case "UNTIL":
			r.until, good = parseUntil(val, tz)
		case "BYDAY":
			for item := range strings.SplitSeq(strings.ToUpper(val), ",") {
				cut := max(len(item)-2, 0)
				wd, known := icsWeekdays[item[cut:]]
				n := 0
				if item[:cut] != "" {
					n, err = strconv.Atoi(item[:cut])
				}
				if !known || err != nil || n < -53 || n > 53 {
					return r, false
				}
				r.byDay = append(r.byDay, weekdayNum{n, wd})
			}
		case "BYMONTHDAY":
			for item := range strings.SplitSeq(val, ",") {
				d, err := strconv.Atoi(item)
				if err != nil || d == 0 || d < -31 || d > 31 {
					return r, false
				}
				r.byMonthDay = append(r.byMonthDay, d)
			}
		case "BYMONTH":
			for item := range strings.SplitSeq(val, ",") {
				m, err := strconv.Atoi(item)
				if err != nil || m < 1 || m > 12 {
					return r, false
				}
				r.byMonth = append(r.byMonth, time.Month(m))
			}
		case "BYSETPOS":
			for item := range strings.SplitSeq(val, ",") {
				n, err := strconv.Atoi(item)
				if err != nil || n == 0 || n < -366 || n > 366 {
					return r, false
				}
				r.bySetPos = append(r.bySetPos, n)
			}
		case "WKST", "": // weeks start on Monday here; "" is a trailing ";"
		default:
			good = false
		}
		if !good {
			return r, false
		}
	}
	ordinals := slices.ContainsFunc(r.byDay, func(w weekdayNum) bool { return w.n != 0 })
	switch r.freq {
	case "DAILY":
		return r, !ordinals
	case "WEEKLY":
		return r, !ordinals && len(r.byMonthDay) == 0
	case "MONTHLY", "YEARLY":
		return r, true
	}
	return r, false
}

// parseUntil reads UNTIL: a date (inclusive, so the end of that day), a UTC date-time,
// or a floating date-time read in the rule's zone.
func parseUntil(v string, tz *time.Location) (time.Time, bool) {
	var t time.Time
	var err error
	switch {
	case len(v) == 8:
		t, err = time.ParseInLocation("20060102", v, tz)
		t = t.Add(24*time.Hour - time.Second)
	case strings.HasSuffix(v, "Z"):
		t, err = time.Parse("20060102T150405Z", v)
	default:
		t, err = time.ParseInLocation("20060102T150405", v, tz)
	}
	return t, err == nil
}

// expand replaces each recurring event by its occurrences around now; other events pass through.
func expand(evs []Event, now time.Time) []Event {
	from, to := now.Add(-recurBack), now.Add(recurAhead)
	out := make([]Event, 0, len(evs))
	for _, e := range evs {
		if e.rec == nil {
			out = append(out, e)
			continue
		}
		out = e.rec.occurrences(e, from, to, out)
	}
	return out
}

// occurrences appends e's occurrences that overlap [from, to], each as long as e.
func (r *recurrence) occurrences(e Event, from, to time.Time, out []Event) []Event {
	start := e.Start.In(r.tz)
	days, dur := 0, time.Duration(0) // all-day events keep whole days, timed ones exact length
	lo := from
	if !e.End.IsZero() {
		if e.AllDay {
			days = int(math.Round(e.End.Sub(e.Start).Hours() / 24))
			lo = from.AddDate(0, 0, -days)
		} else {
			dur = e.End.Sub(e.Start)
			lo = from.Add(-dur)
		}
	}
	emit := func(t time.Time) {
		if t.Before(lo) || r.exAt[t.Unix()] || r.exDay[t.In(r.tz).Format("20060102")] {
			return
		}
		o := e
		o.rec, o.Start = nil, t.In(e.Start.Location())
		switch {
		case e.End.IsZero():
		case e.AllDay:
			o.End = o.Start.AddDate(0, 0, days)
		default:
			o.End = o.Start.Add(dur)
		}
		out = append(out, o)
	}
	if !start.After(to) {
		emit(start) // DTSTART is always the first occurrence, rule or not (RFC 5545 3.8.5.3)
	}
	n, p0 := 1, 0
	if r.count == 0 { // without COUNT nothing before the window matters: jump close to it
		p0 = r.skip(start, lo)
	}
	for p := p0; p < p0+maxPeriods; p++ {
		for _, d := range r.period(start, p) {
			t := time.Date(d.Year(), d.Month(), d.Day(), start.Hour(), start.Minute(), start.Second(), 0, r.tz)
			if !t.After(start) {
				continue
			}
			if r.count > 0 && n >= r.count || !r.until.IsZero() && t.After(r.until) || t.After(to) {
				return out
			}
			n++
			emit(t)
		}
	}
	return out
}

// skip is how many whole periods lie before lo, less one, so period skip still starts before it.
func (r *recurrence) skip(start, lo time.Time) int {
	l := lo.In(r.tz)
	var units int
	switch r.freq {
	case "DAILY":
		units = civilDays(start, l)
	case "WEEKLY":
		units = civilDays(start, l) / 7
	case "MONTHLY":
		units = (l.Year()-start.Year())*12 + int(l.Month()) - int(start.Month())
	case "YEARLY":
		units = l.Year() - start.Year()
	}
	return max(units/r.interval-1, 0)
}

// civilDays counts calendar days from a's date to b's, ignoring DST.
func civilDays(a, b time.Time) int {
	ad := time.Date(a.Year(), a.Month(), a.Day(), 0, 0, 0, 0, time.UTC)
	bd := time.Date(b.Year(), b.Month(), b.Day(), 0, 0, 0, 0, time.UTC)
	return int((bd.Unix() - ad.Unix()) / 86400)
}

// period lists, sorted, the candidate days (midnight UTC, used as plain dates) of the
// p-th day, week, month or year of the rule counted from DTSTART's.
func (r *recurrence) period(start time.Time, p int) []time.Time {
	y, m, d := start.Date()
	var c []time.Time
	switch r.freq {
	case "DAILY":
		day := time.Date(y, m, d+p*r.interval, 0, 0, 0, 0, time.UTC)
		if r.dailyMatch(day) {
			c = append(c, day)
		}
	case "WEEKLY":
		mon := time.Date(y, m, d-(int(start.Weekday())+6)%7+7*p*r.interval, 0, 0, 0, 0, time.UTC)
		wds := []weekdayNum{{0, start.Weekday()}}
		if len(r.byDay) > 0 {
			wds = r.byDay
		}
		for _, w := range wds {
			day := mon.AddDate(0, 0, (int(w.wd)+6)%7)
			if len(r.byMonth) == 0 || slices.Contains(r.byMonth, day.Month()) {
				c = append(c, day)
			}
		}
	case "MONTHLY":
		first := time.Date(y, m+time.Month(p*r.interval), 1, 0, 0, 0, 0, time.UTC)
		if len(r.byMonth) == 0 || slices.Contains(r.byMonth, first.Month()) {
			c = r.inMonth(first, d)
		}
	case "YEARLY":
		yr := y + p*r.interval
		months := r.byMonth
		switch {
		case len(months) > 0:
		case len(r.byMonthDay) > 0:
			months = []time.Month{1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12}
		case len(r.byDay) > 0: // BYDAY alone counts weeks of the whole year
			c = r.byDayIn(time.Date(yr, 1, 1, 0, 0, 0, 0, time.UTC), time.Date(yr+1, 1, 1, 0, 0, 0, 0, time.UTC))
		default:
			if day := time.Date(yr, m, d, 0, 0, 0, 0, time.UTC); day.Day() == d { // Feb 29 only in leap years
				c = append(c, day)
			}
		}
		for _, mo := range months {
			c = append(c, r.inMonth(time.Date(yr, mo, 1, 0, 0, 0, 0, time.UTC), d)...)
		}
	}
	slices.SortFunc(c, func(a, b time.Time) int { return a.Compare(b) })
	c = slices.CompactFunc(c, time.Time.Equal)
	if len(r.bySetPos) == 0 {
		return c
	}
	var set []time.Time // "the first weekday of the month": BYDAY=MO,TU,WE,TH,FR;BYSETPOS=1
	for _, n := range r.bySetPos {
		switch {
		case n > 0 && n <= len(c):
			set = append(set, c[n-1])
		case n < 0 && -n <= len(c):
			set = append(set, c[len(c)+n])
		}
	}
	slices.SortFunc(set, func(a, b time.Time) int { return a.Compare(b) })
	return slices.CompactFunc(set, time.Time.Equal)
}

// dailyMatch applies BYMONTH, BYMONTHDAY and BYDAY as filters, as a DAILY rule does.
func (r *recurrence) dailyMatch(day time.Time) bool {
	if len(r.byMonth) > 0 && !slices.Contains(r.byMonth, day.Month()) {
		return false
	}
	if len(r.byDay) > 0 && !slices.ContainsFunc(r.byDay, func(w weekdayNum) bool { return w.wd == day.Weekday() }) {
		return false
	}
	if len(r.byMonthDay) == 0 {
		return true
	}
	last := time.Date(day.Year(), day.Month()+1, 0, 0, 0, 0, 0, time.UTC).Day()
	return slices.ContainsFunc(r.byMonthDay, func(md int) bool { return md == day.Day() || md < 0 && last+1+md == day.Day() })
}

// inMonth lists one month's candidate days for a MONTHLY or YEARLY rule; dom is DTSTART's day.
func (r *recurrence) inMonth(first time.Time, dom int) []time.Time {
	next := first.AddDate(0, 1, 0)
	last := next.AddDate(0, 0, -1).Day()
	var c []time.Time
	switch {
	case len(r.byMonthDay) > 0:
		var allowed []time.Time // with BYDAY too, a day must be both
		if len(r.byDay) > 0 {
			allowed = r.byDayIn(first, next)
		}
		for _, md := range r.byMonthDay {
			if md < 0 {
				md = last + 1 + md
			}
			if md < 1 || md > last {
				continue
			}
			day := first.AddDate(0, 0, md-1)
			if len(r.byDay) == 0 || slices.ContainsFunc(allowed, day.Equal) {
				c = append(c, day)
			}
		}
	case len(r.byDay) > 0:
		c = r.byDayIn(first, next)
	case dom <= last: // a 31st skips shorter months
		c = append(c, first.AddDate(0, 0, dom-1))
	}
	return c
}

// byDayIn lists the days in [a, b) that BYDAY picks: every such weekday, or the nth one
// (counted from the end when n is negative).
func (r *recurrence) byDayIn(a, b time.Time) []time.Time {
	var c []time.Time
	for _, w := range r.byDay {
		var all []time.Time
		for d := a.AddDate(0, 0, (int(w.wd)-int(a.Weekday())+7)%7); d.Before(b); d = d.AddDate(0, 0, 7) {
			all = append(all, d)
		}
		switch {
		case w.n == 0:
			c = append(c, all...)
		case w.n > 0 && w.n <= len(all):
			c = append(c, all[w.n-1])
		case w.n < 0 && -w.n <= len(all):
			c = append(c, all[len(all)+w.n])
		}
	}
	return c
}

// addICSDuration adds a DURATION value (P1D, PT1H30M, P2W) to t. Days and weeks move the
// calendar, so an all-day event stays on midnights. Zero time for a value it cannot read.
func addICSDuration(t time.Time, v string) time.Time {
	rest, ok := strings.CutPrefix(strings.TrimPrefix(v, "+"), "P")
	if !ok {
		return time.Time{}
	}
	days, d, n, digits, inTime := 0, time.Duration(0), 0, false, false
	for _, ch := range rest {
		switch {
		case ch >= '0' && ch <= '9':
			if n = n*10 + int(ch-'0'); n > 1e6 {
				return time.Time{}
			}
			digits = true
			continue
		case ch == 'T' && !inTime && !digits:
			inTime = true
			continue
		case !digits:
			return time.Time{}
		case ch == 'W' && !inTime:
			days += 7 * n
		case ch == 'D' && !inTime:
			days += n
		case ch == 'H' && inTime:
			d += time.Duration(n) * time.Hour
		case ch == 'M' && inTime:
			d += time.Duration(n) * time.Minute
		case ch == 'S' && inTime:
			d += time.Duration(n) * time.Second
		default:
			return time.Time{}
		}
		n, digits = 0, false
	}
	if digits {
		return time.Time{}
	}
	return t.AddDate(0, 0, days).Add(d)
}
