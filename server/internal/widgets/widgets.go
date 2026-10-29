// Package widgets renders a frame's Layout into one HTML page. The frame gets
// finished HTML; live values arrive later over SSE as {"widget":id,"data":{...}}.
package widgets

import (
	"bytes"
	"embed"
	"encoding/json"
	"fmt"
	"html/template"
	"net/url"
	"sort"
	"strings"

	"github.com/MikeO7/thinframe/server/internal/store"
)

//go:embed templates/*.html templates/page.css templates/page.js templates/obsidian.css templates/aurora.css
var files embed.FS

// Fonts the themes use, served at /fonts/<name>. Manrope: SIL OFL 1.1 (fonts/OFL.txt).
//
//go:embed fonts/manrope.woff2 fonts/manrope.ttf fonts/OFL.txt
var fonts embed.FS

// Font returns a bundled font file (or its licence, OFL.txt) and its Content-Type. Manrope comes
// twice: the frame's WebKit is built without WOFF2 (no brotli), so it takes the TTF.
func Font(name string) ([]byte, string, bool) {
	types := map[string]string{"manrope.woff2": "font/woff2", "manrope.ttf": "font/ttf", "OFL.txt": "text/plain; charset=utf-8"}
	ct, ok := types[name]
	if !ok {
		return nil, "", false
	}
	b, err := fonts.ReadFile("fonts/" + name)
	return b, ct, err == nil
}

// Known widget types. Adding one = a template file named <type>.html (and, for widgets fed
// by a data source, a renderer in page.js).
var Types = []string{"clock", "photo", "weather", "calendar", "status", "text", "iframe",
	"hero", "forecast", "agenda", "cars", "rooms", "climate", "list", "tiles", "energy", "shows", "worldclock", "services", "app", "cameras"}

// Props documents each type's props (MCP list_widgets, server/README.md). Types marked
// "source" take their data from props.source: a key pushed with PUT /api/sources/{key}.
var Props = map[string]string{
	"photo": "interval_s (seconds between photos, default 600)", "text": "text", "iframe": "src (URL)",
	"weather": "place (label)", "calendar": "", "clock": "", "status": "",
	"hero":       "h12 (bool, default true), source, health (a services source key: badge shows containers healthy); data: temp, desc, code, hi, lo (weather adapter), badge {text,state}, chips [{icon,text}]",
	"forecast":   "title, days (default 7); data from the weather adapter: days [{date,code,hi,lo}]; rows when narrow, columns when wide",
	"agenda":     "title, view list|day|next, from/to (hours, day view), max, all (bool), source; data: events [{start,end,title,where}] (calendar adapter if no source)",
	"cars":       "title, view full|compact, cars [{id,name,model,image}], source; data: cars [{id,battery,charging,range,status,stats [{label,value}],climate {on,set,note}}]",
	"rooms":      "title, rooms [{name}], scenes [names], source; data: rooms [{name,on,lights,brightness,temp,humidity,occupied}], scenes [{name,active}]",
	"climate":    "title, source; data: zones [{name,temp,target,action}], badge",
	"list":       "title, items [{label,icon}], source; data: items [{label,value,note,icon,state}], people {home [names], text}, badge",
	"tiles":      "title, items [{label}], source; data: items [{label,value,note,state}], badge",
	"energy":     "source; data: now_kw, parts [{label,kw}], hourly [24 kW], hour, today",
	"shows":      "title, part upcoming|added, max, source; data: upcoming [{date YYYY-MM-DD,title,sub,image}], added [{title,when,image}]",
	"worldclock": "title, zones [{label,tz}]",
	"app":        "app (an app stored with put_app), props (object the app reads as thinframe.props); the app fills the cell, sandboxed (MCP app_guide)",
	"cameras":    "title, cameras [Protect camera names or ids; none = all], interval_s (seconds between snapshots, default 4, min 2); needs PROTECT_URL. Tap a camera for full screen. A doorbell ring pops up on every page (no widget needed)",
	"services":   "title, groups [{name,members [container names]}], source (default feed: services); data: containers [{name,state,note}], host {name,meters [{label,pct,text}],note}",
}

// Images in props and data are http(s) URLs or "asset:<file>" (DATA_DIR/assets).
// state values are ok, warn, bad or off. Icons in data: shield, lock, unlock, bulb, therm,
// box, car, garage, window, door, wash, drop, wind, fan, msg, bolt, person, cam, leaf.

// Icons are the tab bar's icons (24x24 stroke paths).
var Icons = map[string]string{
	"home":   `<path d="M3 11l9-7 9 7"/><path d="M5 10v10h14V10"/>`,
	"car":    `<path d="M3 16v-3l2-5h14l2 5v3"/><path d="M3 16h18"/><circle cx="7.5" cy="17" r="1.8"/><circle cx="16.5" cy="17" r="1.8"/>`,
	"house":  `<path d="M12 3a6 6 0 0 1 4 10.5V16H8v-2.5A6 6 0 0 1 12 3z"/><path d="M9.5 19h5M10.5 21h3"/>`,
	"shield": `<path d="M12 3l8 3v6c0 5-3.5 8-8 9-4.5-1-8-4-8-9V6z"/>`,
	"shows":  `<rect x="3" y="5" width="18" height="16" rx="2"/><path d="M3 10h18M8 3v4M16 3v4"/>`,
	"work":   `<rect x="3" y="7" width="18" height="13" rx="2"/><path d="M9 7V5a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2M3 13h18"/>`,
	"system": `<rect x="4" y="4" width="16" height="6" rx="1.5"/><rect x="4" y="14" width="16" height="6" rx="1.5"/><path d="M8 7h.01M8 17h.01"/>`,
	"photo":  `<rect x="3" y="5" width="18" height="14" rx="2"/><path d="M3 16l5-5 4 4 3-3 6 6"/>`,
	"bolt":   `<path d="M13 2 4 14h7l-1 8 9-12h-7z"/>`,
	"camera": `<rect x="3" y="6" width="13" height="12" rx="2"/><path d="M16 10l5-3v10l-5-3"/>`,
}

var tmpl = template.Must(template.New("").Funcs(template.FuncMap{
	"json": func(v any) template.JS { b, _ := json.Marshal(v); return template.JS(b) }, //nolint:gosec // props are server-owned
	"prop": func(w store.Widget, k string, def any) any {
		if v, ok := w.Props[k]; ok {
			return v
		}
		return def
	},
	// A stored src that fails iframeSrc (layouts saved before the check) renders blank.
	// about:blank is a constant; html/template would otherwise filter it to the
	// relative "#ZgotmplZ", which loads this dashboard inside its own iframe.
	"simrem": func(h int) float64 { return float64(h) / 80 },
	"icon":   func(name string) template.HTML { return template.HTML(Icons[name]) }, //nolint:gosec // constant table
	// An app widget's document; the name is checked by Validate (a lowercase id).
	"appsrc": func(w store.Widget) any {
		n, _ := w.Props["app"].(string)
		if !store.ValidID(n) {
			return template.URL("about:blank")
		}
		return "/apps/" + n + "/index.html"
	},
	"iframesrc": func(w store.Widget) any {
		if s, err := iframeSrc(w); err == nil && s != "" {
			return s
		}
		return template.URL("about:blank")
	},
}).ParseFS(files, "templates/*.html"))

// Page is the data the base template sees.
type Page struct {
	FrameID string
	// EmitKey is the frame's emit key (api.EmitKey; "" = none needed). The page's own emits
	// and the ones it forwards for app widgets carry it; the apps never see it.
	EmitKey     string
	Layout      store.Layout
	Config      store.Config
	PageVersion int
	CSS         template.CSS
	JS          template.JS
	Widgets     []template.HTML
	// Pages are the tabs' grids in tab order; empty when the layout has no tabs (Widgets is
	// then the one grid). Buttons are the top-level tabs with their child pages.
	Pages   []TabPage
	Buttons []TabButton
	IdleS   int
	// SimW/SimH > 0 renders the grid at that size and scales it to fit the real
	// viewport. Used by the simulator, whose headless surface is fixed at 800x600.
	SimW, SimH int
	// SimScale is the CSS scale factor that fits SimW x SimH into the real surface.
	SimScale float64
}

// TabPage is one tab's grid.
type TabPage struct {
	ID      string
	Widgets []template.HTML
}

// TabButton is a tab bar button; Pages lists the tab and its child pages (2+ = a switch).
type TabButton struct {
	store.Tab
	Pages []store.Tab
}

// Render produces the whole page for a frame.
func Render(f store.Frame) ([]byte, error) { return RenderSim(f, "", 0, 0, 0, 0) }

// RenderSim renders with the frame's emit key (Page.EmitKey) and an optional simulated panel
// size (simW x simH) scaled to fit a surface of fitW x fitH (see Page.SimW). fit 0 means
// scale 1.
func RenderSim(f store.Frame, emitKey string, simW, simH, fitW, fitH int) ([]byte, error) {
	scale := 1.0
	if simW > 0 && fitW > 0 && fitH > 0 {
		scale = min(float64(fitW)/float64(simW), float64(fitH)/float64(simH))
	}
	css, _ := files.ReadFile("templates/page.css")
	// obsidian is a theme of its own and the base of aurora.
	if lay := f.Layout; lay.Theme == "obsidian" || lay.Theme == "aurora" {
		ob, _ := files.ReadFile("templates/obsidian.css")
		css = append(append(css, '\n'), ob...)
		if lay.Theme == "aurora" {
			au, _ := files.ReadFile("templates/aurora.css")
			css = append(append(css, '\n'), au...)
		}
	}
	js, _ := files.ReadFile("templates/page.js")
	lay := f.Layout
	if lay.Cols <= 0 || lay.Rows <= 0 {
		lay = store.DefaultLayout()
	}
	ws := make([]template.HTML, 0, len(lay.Widgets))
	for _, w := range lay.Widgets {
		h, err := renderWidget(w)
		if err != nil {
			return nil, err
		}
		ws = append(ws, h)
	}
	pages, buttons := tabPages(lay, ws)
	idle := lay.IdleS
	if idle <= 0 {
		idle = 90
	}
	var out bytes.Buffer
	err := tmpl.ExecuteTemplate(&out, "page.html", Page{
		FrameID: f.ID, EmitKey: emitKey, Layout: lay, Config: f.Config, PageVersion: f.Config.PageVersion,
		CSS: template.CSS(css), JS: template.JS(js), Widgets: ws, //nolint:gosec // embedded assets
		Pages: pages, Buttons: buttons, IdleS: idle,
		SimW: simW, SimH: simH, SimScale: scale,
	})
	return out.Bytes(), err
}

// tabPages groups rendered widgets (same order as lay.Widgets) into one grid per tab.
func tabPages(lay store.Layout, ws []template.HTML) ([]TabPage, []TabButton) {
	if len(lay.Tabs) == 0 {
		return nil, nil
	}
	idx := map[string]int{}
	pages := make([]TabPage, len(lay.Tabs))
	for i, t := range lay.Tabs {
		idx[t.ID] = i
		pages[i].ID = t.ID
	}
	for i, w := range lay.Widgets {
		j, ok := idx[w.Tab]
		if !ok {
			j = 0
		}
		pages[j].Widgets = append(pages[j].Widgets, ws[i])
	}
	var buttons []TabButton
	for _, t := range lay.Tabs {
		if t.Parent != "" {
			continue
		}
		b := TabButton{Tab: t, Pages: []store.Tab{t}}
		for _, c := range lay.Tabs {
			if c.Parent == t.ID {
				b.Pages = append(b.Pages, c)
			}
		}
		buttons = append(buttons, b)
	}
	return pages, buttons
}

func renderWidget(w store.Widget) (template.HTML, error) {
	name := w.Type + ".html"
	if tmpl.Lookup(name) == nil {
		name = "unknown.html"
	}
	var b bytes.Buffer
	if err := tmpl.ExecuteTemplate(&b, name, w); err != nil {
		return "", fmt.Errorf("widget %s: %w", w.ID, err)
	}
	style := fmt.Sprintf("grid-column:%d / span %d;grid-row:%d / span %d", w.X+1, max(w.W, 1), w.Y+1, max(w.H, 1))
	// Props go to the page as JSON for the client-side renderers. json.Marshal escapes <, > and &,
	// so a prop cannot close the script element.
	if len(w.Props) > 0 {
		pj, err := json.Marshal(w.Props)
		if err != nil {
			return "", fmt.Errorf("widget %s props: %w", w.ID, err)
		}
		b.WriteString(`<script type="application/json" class="wp">`)
		b.Write(pj)
		b.WriteString(`</script>`)
	}
	return template.HTML(fmt.Sprintf(`<section class="w w-%s" id="w-%s" data-type="%s" style="%s">%s</section>`, //nolint:gosec // ids validated
		template.HTMLEscapeString(w.Type), template.HTMLEscapeString(w.ID), template.HTMLEscapeString(w.Type), style, b.String())), nil
}

// Validate checks a layout for overlapping or out-of-grid widgets and unknown types.
func Validate(l store.Layout) error {
	if l.Cols < 1 || l.Cols > 12 || l.Rows < 1 || l.Rows > 12 {
		return fmt.Errorf("grid must be 1..12 x 1..12")
	}
	tabs, err := validTabs(l.Tabs)
	if err != nil {
		return err
	}
	if l.Nav != "" && l.Nav != "left" && l.Nav != "bottom" {
		return fmt.Errorf("nav must be left or bottom")
	}
	if l.IdleS < 0 || l.IdleS > 86400 {
		return fmt.Errorf("idle_s must be 0..86400")
	}
	seen := map[string]bool{}
	cells := map[string]map[[2]int]string{}
	for _, w := range l.Widgets {
		if w.ID == "" || seen[w.ID] {
			return fmt.Errorf("widget ids must be unique and non-empty (%q)", w.ID)
		}
		seen[w.ID] = true
		if !known(w.Type) {
			return fmt.Errorf("widget %s: unknown type %q (known: %s)", w.ID, w.Type, strings.Join(Types, ", "))
		}
		if w.X < 0 || w.Y < 0 || w.W < 1 || w.H < 1 || w.X+w.W > l.Cols || w.Y+w.H > l.Rows {
			return fmt.Errorf("widget %s: outside the %dx%d grid", w.ID, l.Cols, l.Rows)
		}
		if w.Type == "iframe" {
			if _, err := iframeSrc(w); err != nil {
				return fmt.Errorf("widget %s: %w", w.ID, err)
			}
		}
		if w.Type == "app" {
			if n, _ := w.Props["app"].(string); !store.ValidID(n) {
				return fmt.Errorf("widget %s: props.app must name an app (a lowercase id), got %q", w.ID, fmt.Sprint(w.Props["app"]))
			}
			if p, ok := w.Props["props"]; ok {
				if _, obj := p.(map[string]any); !obj {
					return fmt.Errorf("widget %s: props.props must be an object", w.ID)
				}
			}
		}
		if w.Type == "cameras" {
			if c, ok := w.Props["cameras"]; ok {
				l, isList := c.([]any)
				if !isList || len(l) > 16 {
					return fmt.Errorf("widget %s: props.cameras must be a list of up to 16 camera names or ids", w.ID)
				}
				for _, x := range l {
					if s, _ := x.(string); s == "" || len(s) > 100 {
						return fmt.Errorf("widget %s: props.cameras entries must be names or ids (1 to 100 characters)", w.ID)
					}
				}
			}
		}
		for _, p := range []string{"source", "health"} {
			if src, ok := w.Props[p]; ok {
				if k, _ := src.(string); !store.ValidID(k) {
					return fmt.Errorf("widget %s: %s must be a lowercase id (a-z, 0-9, _ or -), got %q", w.ID, p, fmt.Sprint(src))
				}
			}
		}
		tab := w.Tab
		if len(l.Tabs) == 0 && tab != "" {
			return fmt.Errorf("widget %s: tab %q set but the layout has no tabs", w.ID, tab)
		}
		if len(l.Tabs) > 0 {
			if tab == "" {
				tab = l.Tabs[0].ID
			}
			if !tabs[tab] {
				return fmt.Errorf("widget %s: unknown tab %q", w.ID, w.Tab)
			}
		}
		if cells[tab] == nil {
			cells[tab] = map[[2]int]string{}
		}
		for x := w.X; x < w.X+w.W; x++ {
			for y := w.Y; y < w.Y+w.H; y++ {
				if other, ok := cells[tab][[2]int{x, y}]; ok {
					return fmt.Errorf("widget %s overlaps %s at %d,%d", w.ID, other, x, y)
				}
				cells[tab][[2]int{x, y}] = w.ID
			}
		}
	}
	return nil
}

// MaxTabs bounds a layout's tabs (child pages included).
const MaxTabs = 12

// validTabs checks tab ids, icons and parents, and returns the set of ids.
func validTabs(ts []store.Tab) (map[string]bool, error) {
	if len(ts) > MaxTabs {
		return nil, fmt.Errorf("at most %d tabs", MaxTabs)
	}
	ids := map[string]bool{}
	for _, t := range ts {
		if !store.ValidID(t.ID) || ids[t.ID] {
			return nil, fmt.Errorf("tab ids must be unique lowercase ids (%q)", t.ID)
		}
		if t.Title == "" || len(t.Title) > 24 || len(t.Label) > 24 {
			return nil, fmt.Errorf("tab %s: title must be 1..24 characters, label at most 24", t.ID)
		}
		if _, ok := Icons[t.Icon]; t.Icon != "" && !ok {
			return nil, fmt.Errorf("tab %s: unknown icon %q", t.ID, t.Icon)
		}
		ids[t.ID] = true
	}
	for i, t := range ts {
		if t.Parent == "" {
			continue
		}
		if i == 0 {
			return nil, fmt.Errorf("tab %s: the first tab cannot have a parent", t.ID)
		}
		var parent *store.Tab
		for j := range ts {
			if ts[j].ID == t.Parent {
				parent = &ts[j]
			}
		}
		if parent == nil || parent.Parent != "" {
			return nil, fmt.Errorf("tab %s: parent %q must be a top-level tab", t.ID, t.Parent)
		}
	}
	return ids, nil
}

// iframeSrc returns an iframe widget's src prop: "" when unset (renders about:blank),
// otherwise it must be an absolute http:// or https:// URL with a host. Relative and
// same-origin paths would load this server's pages next to the dashboard.
func iframeSrc(w store.Widget) (string, error) {
	v, ok := w.Props["src"]
	if !ok {
		return "", nil
	}
	s, _ := v.(string)
	u, err := url.Parse(s)
	if err != nil || (u.Scheme != "http" && u.Scheme != "https") || u.Host == "" {
		return "", fmt.Errorf("iframe src must be an absolute http:// or https:// URL with a host, got %q", fmt.Sprint(v))
	}
	return s, nil
}

func known(t string) bool {
	i := sort.SearchStrings(sortedTypes, t)
	return i < len(sortedTypes) && sortedTypes[i] == t
}

var sortedTypes = func() []string { s := append([]string(nil), Types...); sort.Strings(s); return s }()

// IconNames lists the tab icons, sorted.
func IconNames() []string {
	out := make([]string, 0, len(Icons))
	for k := range Icons {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}
