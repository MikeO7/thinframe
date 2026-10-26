package api

import (
	"io"
	"net/http"
	"net/http/httptest"
	"slices"
	"strings"
	"testing"
	"time"
)

const sampleICS = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nDTSTART:20261020T183000Z\r\nSUMMARY:Dinner\\, with\r\n  Sam\r\nEND:VEVENT\r\n" +
	"BEGIN:VEVENT\r\nDTSTART;VALUE=DATE:20261021\r\nSUMMARY:Dentist\r\nEND:VEVENT\r\n" +
	"BEGIN:VEVENT\r\nDTSTART:20261019T090000Z\r\nSUMMARY:Yesterday\r\nEND:VEVENT\r\n" +
	"BEGIN:VEVENT\r\nDTSTART;TZID=Europe/Paris:20261022T080000\r\nSUMMARY:Floating\r\nEND:VEVENT\r\n" +
	"BEGIN:VEVENT\r\nSUMMARY:No start\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"

func TestParseICS(t *testing.T) {
	evs, err := ParseICS(strings.NewReader(sampleICS), time.UTC)
	if err != nil || len(evs) != 4 {
		t.Fatalf("got %d events, %v", len(evs), err)
	}
	if evs[0].Summary != "Dinner, with Sam" || evs[0].AllDay || evs[0].Start.Hour() != 18 {
		t.Errorf("event 0: %+v", evs[0])
	}
	if !evs[1].AllDay || evs[1].Start.Day() != 21 {
		t.Errorf("all-day: %+v", evs[1])
	}
}

func TestPickAndLine(t *testing.T) {
	evs, _ := ParseICS(strings.NewReader(sampleICS), time.UTC)
	now := time.Date(2026, 10, 20, 12, 0, 0, 0, time.UTC)
	got := pick(evs, now, 2)
	if len(got) != 2 || got[0].Summary != "Dinner, with Sam" || got[1].Summary != "Dentist" {
		t.Fatalf("pick: %+v", got)
	}
	if l := got[0].Line(now); l != "Today 18:30  Dinner, with Sam" {
		t.Errorf("line: %q", l)
	}
	if l := got[1].Line(now); l != "Tomorrow  Dentist" {
		t.Errorf("line: %q", l)
	}
	for _, e := range pick(evs, now, 10) {
		if e.Summary == "Yesterday" {
			t.Error("past day leaked")
		}
	}
}

func TestFeedURL(t *testing.T) {
	for in, want := range map[string]string{
		"webcal://p52-caldav.icloud.com/published/2/abc": "https://p52-caldav.icloud.com/published/2/abc",
		"https://example.com/a.ics":                      "https://example.com/a.ics",
	} {
		if got := feedURL(in); got != want {
			t.Errorf("feedURL(%q) = %q", in, got)
		}
	}
}

// cal wraps VEVENT lines (joined with CRLF) in a VCALENDAR.
func cal(events ...[]string) string {
	var b strings.Builder
	b.WriteString("BEGIN:VCALENDAR\r\n")
	for _, ev := range events {
		b.WriteString("BEGIN:VEVENT\r\n" + strings.Join(ev, "\r\n") + "\r\nEND:VEVENT\r\n")
	}
	b.WriteString("END:VCALENDAR\r\n")
	return b.String()
}

// occurrences parses ics and expands it around now, sorted by start.
func occurrences(t *testing.T, ics string, loc *time.Location, now time.Time) []Event {
	t.Helper()
	evs, err := ParseICS(strings.NewReader(ics), loc)
	if err != nil {
		t.Fatal(err)
	}
	out := expand(evs, now)
	slices.SortStableFunc(out, func(a, b Event) int { return a.Start.Compare(b.Start) })
	return out
}

// wantStarts compares each start, formatted in its own zone, with want.
func wantStarts(t *testing.T, evs []Event, want ...string) {
	t.Helper()
	var got []string
	for _, e := range evs {
		got = append(got, e.Start.Format("2006-01-02 15:04"))
	}
	if !slices.Equal(got, want) {
		t.Errorf("starts:\n got %v\nwant %v", got, want)
	}
}

func TestRRULEWeeklyByDayCount(t *testing.T) {
	ics := cal([]string{"UID:w", "DTSTART:20261026T180000Z", "DTEND:20261026T190000Z",
		"RRULE:FREQ=WEEKLY;BYDAY=MO,WE;COUNT=5", "SUMMARY:Swim"})
	got := occurrences(t, ics, time.UTC, time.Date(2026, 10, 26, 12, 0, 0, 0, time.UTC))
	wantStarts(t, got, "2026-10-26 18:00", "2026-10-28 18:00", "2026-11-02 18:00", "2026-11-04 18:00", "2026-11-09 18:00")
	for _, e := range got {
		if e.End.Sub(e.Start) != time.Hour || e.Summary != "Swim" || e.AllDay {
			t.Errorf("occurrence %+v", e)
		}
	}
}

func TestRRULEMonthly(t *testing.T) {
	ny, _ := time.LoadLocation("America/New_York")
	now := time.Date(2026, 10, 26, 12, 0, 0, 0, ny)
	second := cal([]string{"UID:m", "DTSTART;TZID=America/New_York:20260113T190000", "RRULE:FREQ=MONTHLY;BYDAY=2TU", "SUMMARY:PTA"})
	wantStarts(t, occurrences(t, second, ny, now), "2026-11-10 19:00", "2026-12-08 19:00")
	last := cal([]string{"UID:l", "DTSTART;TZID=America/New_York:20260130T080000", "RRULE:FREQ=MONTHLY;BYDAY=-1FR", "SUMMARY:Pay"})
	wantStarts(t, occurrences(t, last, ny, now), "2026-10-30 08:00", "2026-11-27 08:00", "2026-12-25 08:00")
	eom := cal([]string{"UID:e", "DTSTART;TZID=America/New_York:20260131T100000", "RRULE:FREQ=MONTHLY;BYMONTHDAY=-1", "SUMMARY:Rent"})
	wantStarts(t, occurrences(t, eom, ny, now), "2026-10-31 10:00", "2026-11-30 10:00")
	weekday := cal([]string{"UID:w", "DTSTART;TZID=America/New_York:20260102T090000", "RRULE:FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=1", "SUMMARY:Bills"})
	wantStarts(t, occurrences(t, weekday, ny, now), "2026-11-02 09:00", "2026-12-01 09:00") // first weekday; Nov 1 is a Sunday
	lastWeekend := cal([]string{"UID:x", "DTSTART;TZID=America/New_York:20260131T090000", "RRULE:FREQ=MONTHLY;BYDAY=SA,SU;BYSETPOS=-1", "SUMMARY:Clean"})
	wantStarts(t, occurrences(t, lastWeekend, ny, now), "2026-10-31 09:00", "2026-11-29 09:00") // Dec 27 is past the window
	thirtyFirst := cal([]string{"UID:t", "DTSTART;TZID=America/New_York:20260131T100000", "RRULE:FREQ=MONTHLY", "SUMMARY:31st"})
	wantStarts(t, occurrences(t, thirtyFirst, ny, now), "2026-10-31 10:00") // no November 31st; December 31 is past the window
}

func TestRRULEYearlyBirthdayAllDay(t *testing.T) {
	la, _ := time.LoadLocation("America/Los_Angeles")
	ics := cal([]string{"UID:b", "DTSTART;VALUE=DATE:19800105", "DTEND;VALUE=DATE:19800106",
		"RRULE:FREQ=YEARLY", "SUMMARY:Mum's birthday"})
	got := occurrences(t, ics, la, time.Date(2026, 12, 1, 9, 0, 0, 0, la))
	if len(got) != 1 {
		t.Fatalf("got %+v", got)
	}
	e := got[0]
	if !e.AllDay || !e.Start.Equal(time.Date(2027, 1, 5, 0, 0, 0, 0, la)) || !e.End.Equal(time.Date(2027, 1, 6, 0, 0, 0, 0, la)) {
		t.Errorf("birthday %+v", e)
	}
	thanks := cal([]string{"UID:t", "DTSTART;VALUE=DATE:20201126", "RRULE:FREQ=YEARLY;BYMONTH=11;BYDAY=4TH", "SUMMARY:Thanksgiving"})
	wantStarts(t, occurrences(t, thanks, la, time.Date(2026, 10, 26, 9, 0, 0, 0, la)), "2026-11-26 00:00")
}

func TestRRULEUntil(t *testing.T) {
	now := time.Date(2026, 10, 26, 12, 0, 0, 0, time.UTC)
	timed := cal([]string{"UID:u", "DTSTART:20261020T100000Z", "RRULE:FREQ=DAILY;UNTIL=20261028T235959Z", "SUMMARY:Camp"})
	wantStarts(t, occurrences(t, timed, time.UTC, now), "2026-10-26 10:00", "2026-10-27 10:00", "2026-10-28 10:00")
	allDay := cal([]string{"UID:d", "DTSTART;VALUE=DATE:20261001", "RRULE:FREQ=WEEKLY;INTERVAL=2;UNTIL=20261112", "SUMMARY:Bins"})
	wantStarts(t, occurrences(t, allDay, time.UTC, now), "2026-10-29 00:00", "2026-11-12 00:00")
}

func TestRRULEExdate(t *testing.T) {
	la, _ := time.LoadLocation("America/Los_Angeles")
	ics := cal([]string{"UID:x", "DTSTART;TZID=America/Los_Angeles:20261005T090000", "RRULE:FREQ=WEEKLY;BYDAY=MO",
		"EXDATE;TZID=America/Los_Angeles:20261102T090000,20261109T090000", "EXDATE:20261116T170000Z", "SUMMARY:Piano"},
		[]string{"UID:y", "DTSTART;VALUE=DATE:20261027", "RRULE:FREQ=WEEKLY", "EXDATE;VALUE=DATE:20261103", "SUMMARY:Bins"})
	var piano, bins []Event
	for _, e := range occurrences(t, ics, la, time.Date(2026, 10, 26, 8, 0, 0, 0, la)) {
		if e.Summary == "Piano" {
			piano = append(piano, e)
		} else {
			bins = append(bins, e)
		}
	}
	wantStarts(t, piano[:3], "2026-10-26 09:00", "2026-11-23 09:00", "2026-11-30 09:00")
	wantStarts(t, bins[:3], "2026-10-27 00:00", "2026-11-10 00:00", "2026-11-17 00:00")
}

func TestRRULERecurrenceIDMoveAndCancel(t *testing.T) {
	ics := cal(
		[]string{"UID:r@x", "DTSTART:20261029T180000Z", "DURATION:PT90M", "RRULE:FREQ=WEEKLY", "SUMMARY:Football"},
		[]string{"UID:r@x", "RECURRENCE-ID:20261105T180000Z", "DTSTART:20261105T180000Z", "STATUS:CANCELLED", "SUMMARY:Football"},
		[]string{"UID:r@x", "RECURRENCE-ID:20261029T180000Z", "DTSTART:20261030T200000Z", "DTEND:20261030T210000Z", "SUMMARY:Football (moved)"},
	)
	got := occurrences(t, ics, time.UTC, time.Date(2026, 10, 26, 12, 0, 0, 0, time.UTC))
	wantStarts(t, got[:3], "2026-10-30 20:00", "2026-11-12 18:00", "2026-11-19 18:00")
	if got[0].Summary != "Football (moved)" || got[0].End.Sub(got[0].Start) != time.Hour {
		t.Errorf("override %+v", got[0])
	}
	if got[1].End.Sub(got[1].Start) != 90*time.Minute {
		t.Errorf("DURATION lost: %+v", got[1])
	}
}

func TestRRULEKeepsLocalTimeAcrossDST(t *testing.T) {
	la, _ := time.LoadLocation("America/Los_Angeles")
	ics := cal([]string{"UID:dst", "DTSTART;TZID=America/Los_Angeles:20261019T090000", "DTEND;TZID=America/Los_Angeles:20261019T100000",
		"RRULE:FREQ=WEEKLY", "SUMMARY:Standup"})
	got := occurrences(t, ics, la, time.Date(2026, 10, 25, 12, 0, 0, 0, la))
	wantStarts(t, got[:3], "2026-10-26 09:00", "2026-11-02 09:00", "2026-11-09 09:00")
	if a, b := got[0].Start.UTC().Hour(), got[1].Start.UTC().Hour(); a != 16 || b != 17 {
		t.Errorf("UTC hours %d, %d: want 16 (PDT) then 17 (PST)", a, b)
	}
	for _, e := range got {
		if e.Start.In(la).Hour() != 9 || e.End.Sub(e.Start) != time.Hour {
			t.Errorf("occurrence %v to %v", e.Start, e.End)
		}
	}
}

func TestRRULEPathologicalIsFast(t *testing.T) {
	now := time.Date(2026, 10, 26, 12, 0, 0, 0, time.UTC)
	ics := cal(
		[]string{"UID:p", "DTSTART:19000101T090000Z", "RRULE:FREQ=DAILY;INTERVAL=1", "SUMMARY:Forever"},
		[]string{"UID:c", "DTSTART:10000101T090000Z", "RRULE:FREQ=DAILY;COUNT=100000000", "SUMMARY:Counted"},
		[]string{"UID:e", "DTSTART:20000101T090000Z", "RRULE:FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=30", "SUMMARY:Never"},
	)
	t0 := time.Now()
	got := occurrences(t, ics, time.UTC, now)
	if d := time.Since(t0); d > time.Second {
		t.Errorf("took %v", d)
	}
	if len(got) != 61 || !got[0].Start.Equal(time.Date(2026, 10, 26, 9, 0, 0, 0, time.UTC)) {
		t.Errorf("got %d occurrences: %v", len(got), got)
	}
}

func TestRRULEUnsupportedShowsFirstInstance(t *testing.T) {
	ics := cal([]string{"UID:h", "DTSTART:20261027T090000Z", "RRULE:FREQ=HOURLY", "SUMMARY:Hourly"},
		[]string{"UID:s", "DTSTART:20261028T090000Z", "RRULE:FREQ=MONTHLY;BYWEEKNO=20", "SUMMARY:Weekno"},
		[]string{"UID:z", "DTSTART:20261029T090000Z", "RRULE:FREQ=WEEKLY;BYDAY=XX", "SUMMARY:Broken"})
	got := occurrences(t, ics, time.UTC, time.Date(2026, 10, 26, 12, 0, 0, 0, time.UTC))
	wantStarts(t, got, "2026-10-27 09:00", "2026-10-28 09:00", "2026-10-29 09:00")
}

func TestAddICSDuration(t *testing.T) {
	t0 := time.Date(2026, 10, 31, 0, 0, 0, 0, time.UTC)
	for in, want := range map[string]time.Time{
		"P1D":        t0.AddDate(0, 0, 1),
		"P1W":        t0.AddDate(0, 0, 7),
		"PT1H30M":    t0.Add(90 * time.Minute),
		"P1DT2H":     t0.AddDate(0, 0, 1).Add(2 * time.Hour),
		"1D":         {},
		"P1":         {},
		"PT1D":       {},
		"P99999999D": {},
	} {
		if got := addICSDuration(t0, in); !got.Equal(want) {
			t.Errorf("%s: got %v want %v", in, got, want)
		}
	}
}

func TestFeedURLs(t *testing.T) {
	got := FeedURLs(" webcal://a.example/1 ,https://b.example/2.ics\nhttps://c.example/3\t")
	if !slices.Equal(got, []string{"webcal://a.example/1", "https://b.example/2.ics", "https://c.example/3"}) {
		t.Errorf("got %q", got)
	}
	if len(FeedURLs("  ")) != 0 {
		t.Error("blank list")
	}
	if _, ok := ICSFeeds([]string{"https://x"}, nil).(*ICS); !ok {
		t.Error("one URL should be a plain ICS")
	}
}

func TestSeveralFeedsOneDown(t *testing.T) {
	soon := time.Now().UTC().Add(2 * time.Hour).Format("20060102T150405Z")
	weekly := time.Now().UTC().Add(-7*24*time.Hour + 26*time.Hour).Format("20060102T150405Z") // next one in 26 h
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/family.ics":
			_, _ = io.WriteString(w, cal([]string{"UID:1", "DTSTART:" + weekly, "RRULE:FREQ=WEEKLY", "SUMMARY:Family"}))
		case "/work.ics":
			_, _ = io.WriteString(w, cal([]string{"UID:2", "DTSTART:" + soon, "SUMMARY:Work"}))
		default:
			http.Error(w, "boom", http.StatusInternalServerError)
		}
	}))
	defer ts.Close()
	src := ICSFeeds(FeedURLs(ts.URL+"/family.ics, "+ts.URL+"/broken.ics "+ts.URL+"/work.ics"), time.UTC)
	got, err := src.Upcoming(time.Now(), 3)
	if err != nil || len(got) != 3 || got[0].Summary != "Work" || got[1].Summary != "Family" || got[2].Summary != "Family" {
		t.Fatalf("got %+v, %v", got, err)
	}
	if got, _ := src.Upcoming(time.Now(), 1); len(got) != 1 || got[0].Summary != "Work" {
		t.Errorf("n=1: %+v", got)
	}
	down := ICSFeeds([]string{ts.URL + "/a", ts.URL + "/b"}, time.UTC)
	if _, err := down.Upcoming(time.Now(), 5); err == nil {
		t.Error("all feeds down should be an error")
	}
}
