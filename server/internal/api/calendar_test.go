package api

import (
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
