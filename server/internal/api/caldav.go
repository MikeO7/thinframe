package api

import (
	"bytes"
	"encoding/xml"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"strings"
	"time"
)

// CalDAV reads calendars the way a phone syncs them, so a private calendar never needs a
// public link. Made for iCloud (an app-specific password from account.apple.com; iCloud has
// no secret iCal address), and standard enough for Fastmail or Nextcloud. Google needs OAuth
// for CalDAV; its "secret address in iCal format" works with CALENDAR_ICS_URL instead.
//
// Discovery follows RFC 6764/4791: the principal, its calendar home, the calendars in it (event
// calendars only: iCloud Reminders lists are VTODO), then one time-range REPORT per calendar.
// Recurring events come back as masters with RRULE and are expanded like ICS feeds.
// Cached 10 minutes like the feeds.
type CalDAV struct {
	URL   string   // server root, e.g. https://caldav.icloud.com
	User  string   // Apple ID
	Pass  string   // app-specific password
	Names []string // calendar names to show (case-insensitive); empty = all event calendars
	Loc   *time.Location
	feed  refresher[[]Event]
}

const (
	// calPast and calAhead bound the REPORT: events overlapping this window come back.
	calPast  = 24 * time.Hour
	calAhead = 62 * 24 * time.Hour
)

// Upcoming returns up to n events, like ICS.Upcoming.
func (c *CalDAV) Upcoming(now time.Time, n int) ([]Event, error) {
	evs, err := c.feed.get(10*time.Minute, func() ([]Event, error) { return c.fetch(time.Now()) })
	return pick(expand(evs, now), now, n), err
}

type davProp struct {
	Principal struct {
		Href string `xml:"DAV: href"`
	} `xml:"DAV: current-user-principal"`
	Home struct {
		Href string `xml:"DAV: href"`
	} `xml:"urn:ietf:params:xml:ns:caldav calendar-home-set"`
	Name string `xml:"DAV: displayname"`
	Type struct {
		Calendar *struct{} `xml:"urn:ietf:params:xml:ns:caldav calendar"`
	} `xml:"DAV: resourcetype"`
	Comps struct {
		Comp []struct {
			Name string `xml:"name,attr"`
		} `xml:"urn:ietf:params:xml:ns:caldav comp"`
	} `xml:"urn:ietf:params:xml:ns:caldav supported-calendar-component-set"`
	Data string `xml:"urn:ietf:params:xml:ns:caldav calendar-data"`
}

type davResponse struct {
	Href     string `xml:"DAV: href"`
	Propstat []struct {
		Prop   davProp `xml:"DAV: prop"`
		Status string  `xml:"DAV: status"`
	} `xml:"DAV: propstat"`
}

// prop is the response's properties that came back 200 (others are listed as 404).
func (r davResponse) prop() davProp {
	for _, ps := range r.Propstat {
		if ps.Status == "" || strings.Contains(ps.Status, " 200") {
			return ps.Prop
		}
	}
	return davProp{}
}

// call sends one WebDAV request and returns the multistatus responses and the URL that
// answered (relative hrefs resolve against it). Redirects are followed by hand: Go's client
// would turn a PROPFIND into a GET and drop the password on another host.
func (c *CalDAV) call(method, target, depth, body string) ([]davResponse, *url.URL, error) {
	cl := http.Client{Timeout: 20 * time.Second,
		CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}
	for range 4 {
		u, err := url.Parse(target)
		if err != nil {
			return nil, nil, err
		}
		if !c.trusted(u) {
			return nil, nil, fmt.Errorf("caldav: not sending the password to %s", u.Host)
		}
		req, err := http.NewRequest(method, u.String(), strings.NewReader(body))
		if err != nil {
			return nil, nil, err
		}
		req.SetBasicAuth(c.User, c.Pass)
		req.Header.Set("Depth", depth)
		req.Header.Set("Content-Type", "application/xml; charset=utf-8")
		resp, err := cl.Do(req)
		if err != nil {
			return nil, nil, err
		}
		rs, next, err := c.answer(method, u, resp)
		if next == "" {
			return rs, u, err
		}
		if target, err = resolve(u, next); err != nil {
			return nil, nil, err
		}
	}
	return nil, nil, fmt.Errorf("caldav: too many redirects")
}

// answer reads one response: the multistatus, or the Location of a redirect.
func (c *CalDAV) answer(method string, u *url.URL, resp *http.Response) ([]davResponse, string, error) {
	defer resp.Body.Close()
	switch resp.StatusCode {
	case http.StatusMultiStatus:
	case http.StatusMovedPermanently, http.StatusFound, http.StatusTemporaryRedirect, http.StatusPermanentRedirect:
		if l := resp.Header.Get("Location"); l != "" {
			return nil, l, nil
		}
		return nil, "", fmt.Errorf("caldav: %s %s: %s without Location", method, u.Path, resp.Status)
	case http.StatusUnauthorized:
		return nil, "", fmt.Errorf("caldav: sign-in refused (check CALDAV_USER and the app-specific password)")
	default:
		return nil, "", fmt.Errorf("caldav: %s %s: %s", method, u.Path, resp.Status)
	}
	var ms struct {
		Responses []davResponse `xml:"DAV: response"`
	}
	if err := xml.NewDecoder(io.LimitReader(resp.Body, 16<<20)).Decode(&ms); err != nil {
		return nil, "", fmt.Errorf("caldav: %w", err)
	}
	return ms.Responses, "", nil
}

// trusted: the password goes only to the configured host or its sibling hosts (iCloud keeps
// each account on a pNN-caldav.icloud.com host), over the configured scheme or https.
func (c *CalDAV) trusted(u *url.URL) bool {
	base, err := url.Parse(c.URL)
	if err != nil || (u.Scheme != "https" && u.Scheme != base.Scheme) {
		return false
	}
	h, bh := u.Hostname(), base.Hostname()
	if h == bh {
		return true
	}
	if net.ParseIP(bh) != nil {
		return false
	}
	if _, parent, ok := strings.Cut(bh, "."); ok && strings.Contains(parent, ".") {
		return strings.HasSuffix(h, "."+parent)
	}
	return false
}

func resolve(base *url.URL, href string) (string, error) {
	u, err := url.Parse(strings.TrimSpace(href))
	if err != nil || href == "" {
		return "", fmt.Errorf("caldav: bad href %q", href)
	}
	return base.ResolveReference(u).String(), nil
}

// one is the single property a Depth: 0 PROPFIND asked for.
func (c *CalDAV) one(target, propXML string, get func(davProp) string) (string, error) {
	rs, base, err := c.call("PROPFIND", target, "0",
		`<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop>`+propXML+`</d:prop></d:propfind>`)
	if err != nil {
		return "", err
	}
	for _, r := range rs {
		if v := get(r.prop()); v != "" {
			return resolve(base, v)
		}
	}
	return "", fmt.Errorf("caldav: %s has no %s", target, propXML)
}

// calendars lists the event calendars to read: (name, URL).
func (c *CalDAV) calendars() ([][2]string, error) {
	principal, err := c.one(c.URL, "<d:current-user-principal/>", func(p davProp) string { return p.Principal.Href })
	if err != nil {
		return nil, err
	}
	home, err := c.one(principal, "<c:calendar-home-set/>", func(p davProp) string { return p.Home.Href })
	if err != nil {
		return nil, err
	}
	rs, base, err := c.call("PROPFIND", home, "1",
		`<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:displayname/><d:resourcetype/><c:supported-calendar-component-set/></d:prop></d:propfind>`)
	if err != nil {
		return nil, err
	}
	var out [][2]string
	for _, r := range rs {
		p := r.prop()
		if p.Type.Calendar == nil || !hasEvents(p) || !c.wanted(p.Name) {
			continue
		}
		u, err := resolve(base, r.Href)
		if err != nil {
			return nil, err
		}
		out = append(out, [2]string{p.Name, u})
	}
	if len(out) == 0 {
		return nil, fmt.Errorf("caldav: no event calendar matches CALDAV_CALENDARS")
	}
	return out, nil
}

// hasEvents: a calendar that lists its components must list VEVENT; one that lists none holds any.
func hasEvents(p davProp) bool {
	if len(p.Comps.Comp) == 0 {
		return true
	}
	for _, x := range p.Comps.Comp {
		if strings.EqualFold(x.Name, "VEVENT") {
			return true
		}
	}
	return false
}

func (c *CalDAV) wanted(name string) bool {
	if len(c.Names) == 0 {
		return true
	}
	for _, n := range c.Names {
		if strings.EqualFold(strings.TrimSpace(n), strings.TrimSpace(name)) {
			return true
		}
	}
	return false
}

func (c *CalDAV) fetch(now time.Time) ([]Event, error) {
	cals, err := c.calendars()
	if err != nil {
		return nil, err
	}
	const f = "20060102T150405Z"
	q := `<?xml version="1.0" encoding="utf-8"?><c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">` +
		`<d:prop><c:calendar-data/></d:prop><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VEVENT">` +
		`<c:time-range start="` + now.Add(-calPast).UTC().Format(f) + `" end="` + now.Add(calAhead).UTC().Format(f) + `"/>` +
		`</c:comp-filter></c:comp-filter></c:filter></c:calendar-query>`
	var out []Event
	for _, cal := range cals {
		rs, _, err := c.call("REPORT", cal[1], "1", q)
		if err != nil {
			return nil, fmt.Errorf("calendar %q: %w", cal[0], err)
		}
		for _, r := range rs {
			if d := r.prop().Data; d != "" {
				evs, err := ParseICS(bytes.NewReader([]byte(d)), c.Loc)
				if err != nil {
					return nil, fmt.Errorf("calendar %q: %w", cal[0], err)
				}
				out = append(out, evs...)
			}
		}
	}
	return out, nil
}
