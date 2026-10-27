package api

import (
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
	"time"
)

// fakeICloud answers the way caldav.icloud.com does: the root redirects, the principal names
// a calendar home given as an absolute URL, and the home holds event calendars and a
// Reminders list (VTODO only) that must be skipped.
func fakeICloud(t *testing.T, reports *[]string) *httptest.Server {
	t.Helper()
	ms := func(w http.ResponseWriter, body string) {
		w.Header().Set("Content-Type", "application/xml")
		w.WriteHeader(http.StatusMultiStatus)
		fmt.Fprint(w, `<?xml version="1.0"?><multistatus xmlns="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">`+body+`</multistatus>`)
	}
	cal := func(href, name, comps string) string {
		return `<response><href>` + href + `</href><propstat><prop><displayname>` + name + `</displayname>` +
			`<resourcetype><collection/><C:calendar/></resourcetype><C:supported-calendar-component-set>` + comps +
			`</C:supported-calendar-component-set></prop><status>HTTP/1.1 200 OK</status></propstat>` +
			`<propstat><prop><getctag/></prop><status>HTTP/1.1 404 Not Found</status></propstat></response>`
	}
	ev := `<C:comp name="VEVENT"/>`
	var srv *httptest.Server
	srv = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if u, p, _ := r.BasicAuth(); u != "me@icloud.com" || p != "abcd-efgh-ijkl-mnop" {
			w.WriteHeader(http.StatusUnauthorized)
			return
		}
		body, _ := io.ReadAll(r.Body)
		switch r.Method + " " + r.URL.Path {
		case "PROPFIND /":
			http.Redirect(w, r, "/dav/", http.StatusMovedPermanently)
		case "PROPFIND /dav/":
			ms(w, `<response><href>/dav/</href><propstat><prop><current-user-principal><href>/123/principal/</href></current-user-principal></prop><status>HTTP/1.1 200 OK</status></propstat></response>`)
		case "PROPFIND /123/principal/":
			ms(w, `<response><href>/123/principal/</href><propstat><prop><C:calendar-home-set><href>`+srv.URL+`/123/calendars/</href></C:calendar-home-set></prop><status>HTTP/1.1 200 OK</status></propstat></response>`)
		case "PROPFIND /123/calendars/":
			if r.Header.Get("Depth") != "1" {
				t.Errorf("home listed with Depth %q", r.Header.Get("Depth"))
			}
			ms(w, `<response><href>/123/calendars/</href><propstat><prop><resourcetype><collection/></resourcetype></prop><status>HTTP/1.1 200 OK</status></propstat></response>`+
				cal("/123/calendars/home/", "Home", ev)+cal("/123/calendars/work/", "Work", ev)+
				cal("/123/calendars/tasks/", "Reminders", `<C:comp name="VTODO"/>`))
		case "REPORT /123/calendars/home/", "REPORT /123/calendars/work/", "REPORT /123/calendars/tasks/":
			*reports = append(*reports, r.URL.Path)
			if !strings.Contains(string(body), `<c:time-range start="`) {
				t.Errorf("REPORT without a time range: %s", body)
			}
			name := strings.Split(r.URL.Path, "/")[3]
			ms(w, `<response><href>`+r.URL.Path+`a.ics</href><propstat><prop><C:calendar-data>BEGIN:VCALENDAR
BEGIN:VEVENT
UID:`+name+`-1
DTSTART:20261027T170000Z
DTEND:20261027T180000Z
SUMMARY:`+name+` dinner &amp; games
END:VEVENT
END:VCALENDAR
</C:calendar-data></prop><status>HTTP/1.1 200 OK</status></propstat></response>`+
				`<response><href>`+r.URL.Path+`b.ics</href><propstat><prop><C:calendar-data>BEGIN:VCALENDAR
BEGIN:VEVENT
UID:`+name+`-2
DTSTART:20261020T090000Z
DTEND:20261020T093000Z
RRULE:FREQ=WEEKLY
SUMMARY:`+name+` standup
END:VEVENT
END:VCALENDAR
</C:calendar-data></prop><status>HTTP/1.1 200 OK</status></propstat></response>`)
		default:
			t.Errorf("unexpected %s %s", r.Method, r.URL.Path)
			http.NotFound(w, r)
		}
	}))
	return srv
}

func TestCalDAVReadsEventCalendarsWithoutAPublicLink(t *testing.T) {
	var reports []string
	srv := fakeICloud(t, &reports)
	defer srv.Close()
	now := time.Date(2026, 10, 26, 12, 0, 0, 0, time.UTC)

	c := &CalDAV{URL: srv.URL, User: "me@icloud.com", Pass: "abcd-efgh-ijkl-mnop", Loc: time.UTC}
	evs, err := c.Upcoming(now, 10)
	if err != nil {
		t.Fatal(err)
	}
	if len(reports) != 2 {
		t.Fatalf("read %v, want Home and Work only (not Reminders)", reports)
	}
	var got []string
	for _, e := range evs {
		got = append(got, e.Start.Format("01-02 15:04")+" "+e.Summary)
	}
	want := "10-27 09:00 home standup|10-27 09:00 work standup|10-27 17:00 home dinner & games|10-27 17:00 work dinner & games|" +
		"11-03 09:00 home standup|11-03 09:00 work standup"
	if g := strings.Join(got[:6], "|"); g != want {
		t.Fatalf("events\n got %s\nwant %s", g, want)
	}

	reports = nil
	c = &CalDAV{URL: srv.URL, User: "me@icloud.com", Pass: "abcd-efgh-ijkl-mnop", Names: []string{" work "}, Loc: time.UTC}
	if _, err := c.Upcoming(now, 10); err != nil || len(reports) != 1 || reports[0] != "/123/calendars/work/" {
		t.Fatalf("CALDAV_CALENDARS=work read %v (%v)", reports, err)
	}

	c = &CalDAV{URL: srv.URL, User: "me@icloud.com", Pass: "wrong", Loc: time.UTC}
	if _, err := c.Upcoming(now, 10); err == nil || !strings.Contains(err.Error(), "app-specific password") {
		t.Fatalf("wrong password: %v", err)
	}
	c = &CalDAV{URL: srv.URL, User: "me@icloud.com", Pass: "abcd-efgh-ijkl-mnop", Names: []string{"Nope"}, Loc: time.UTC}
	if _, err := c.Upcoming(now, 10); err == nil || !strings.Contains(err.Error(), "CALDAV_CALENDARS") {
		t.Fatalf("unknown calendar: %v", err)
	}
}

// The password goes to the configured provider's hosts only, never where a redirect or href points.
func TestCalDAVKeepsThePasswordAtTheProvider(t *testing.T) {
	c := &CalDAV{URL: "https://caldav.icloud.com"}
	for raw, ok := range map[string]bool{
		"https://caldav.icloud.com/":         true,
		"https://p52-caldav.icloud.com/123/": true,
		"http://p52-caldav.icloud.com/123/":  false,
		"https://evil.example/":              false,
		"https://icloud.com.evil.example/":   false,
		"https://evilicloud.com/":            false,
	} {
		u, _ := url.Parse(raw)
		if c.trusted(u) != ok {
			t.Errorf("%s trusted=%v, want %v", raw, !ok, ok)
		}
	}
	local := &CalDAV{URL: "http://127.0.0.1:5232"}
	for _, raw := range []string{"http://127.0.0.2:5232/", "http://1.0.0.1:5232/"} {
		if u, _ := url.Parse(raw); (&CalDAV{URL: "http://10.0.0.1:5232"}).trusted(u) || local.trusted(u) {
			t.Errorf("an IP address trusted %s", raw)
		}
	}
}
