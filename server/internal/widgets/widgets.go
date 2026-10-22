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

//go:embed templates/*.html templates/page.css templates/page.js
var files embed.FS

// Known widget types. Adding one = a template file named <type>.html.
var Types = []string{"clock", "photo", "weather", "calendar", "status", "text", "iframe"}

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
	"iframesrc": func(w store.Widget) any {
		if s, err := iframeSrc(w); err == nil && s != "" {
			return s
		}
		return template.URL("about:blank")
	},
}).ParseFS(files, "templates/*.html"))

// Page is the data the base template sees.
type Page struct {
	FrameID     string
	Layout      store.Layout
	Config      store.Config
	PageVersion int
	CSS         template.CSS
	JS          template.JS
	Widgets     []template.HTML
	// SimW/SimH > 0 renders the grid at that size and scales it to fit the real
	// viewport. Used by the simulator, whose headless surface is fixed at 800x600.
	SimW, SimH int
	// SimScale is the CSS scale factor that fits SimW x SimH into the real surface.
	SimScale float64
}

// Render produces the whole page for a frame.
func Render(f store.Frame) ([]byte, error) { return RenderSim(f, 0, 0, 0, 0) }

// RenderSim renders with an optional simulated panel size (simW x simH) scaled to
// fit a surface of fitW x fitH (see Page.SimW). fit 0 means scale 1.
func RenderSim(f store.Frame, simW, simH, fitW, fitH int) ([]byte, error) {
	scale := 1.0
	if simW > 0 && fitW > 0 && fitH > 0 {
		scale = min(float64(fitW)/float64(simW), float64(fitH)/float64(simH))
	}
	css, _ := files.ReadFile("templates/page.css")
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
	var out bytes.Buffer
	err := tmpl.ExecuteTemplate(&out, "page.html", Page{
		FrameID: f.ID, Layout: lay, Config: f.Config, PageVersion: f.Config.PageVersion,
		CSS: template.CSS(css), JS: template.JS(js), Widgets: ws, //nolint:gosec // embedded assets
		SimW: simW, SimH: simH, SimScale: scale,
	})
	return out.Bytes(), err
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
	return template.HTML(fmt.Sprintf(`<section class="w w-%s" id="w-%s" data-type="%s" style="%s">%s</section>`, //nolint:gosec // ids validated
		template.HTMLEscapeString(w.Type), template.HTMLEscapeString(w.ID), template.HTMLEscapeString(w.Type), style, b.String())), nil
}

// Validate checks a layout for overlapping or out-of-grid widgets and unknown types.
func Validate(l store.Layout) error {
	if l.Cols < 1 || l.Cols > 12 || l.Rows < 1 || l.Rows > 12 {
		return fmt.Errorf("grid must be 1..12 x 1..12")
	}
	seen := map[string]bool{}
	cells := map[[2]int]string{}
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
		for x := w.X; x < w.X+w.W; x++ {
			for y := w.Y; y < w.Y+w.H; y++ {
				if other, ok := cells[[2]int{x, y}]; ok {
					return fmt.Errorf("widget %s overlaps %s at %d,%d", w.ID, other, x, y)
				}
				cells[[2]int{x, y}] = w.ID
			}
		}
	}
	return nil
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
