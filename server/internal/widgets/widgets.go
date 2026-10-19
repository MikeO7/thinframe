// Package widgets renders a frame's Layout into one HTML page. The frame gets
// finished HTML; live values arrive later over SSE as {"widget":id,"data":{...}}.
package widgets

import (
	"bytes"
	"embed"
	"encoding/json"
	"fmt"
	"html/template"
	"sort"
	"strings"

	"github.com/MikeO7/thinframe/server/internal/store"
)

//go:embed templates/*.html templates/page.css templates/page.js
var files embed.FS

// Known widget types. Adding one = a template file named <type>.html.
var Types = []string{"clock", "photo", "weather", "status", "text", "iframe"}

var tmpl = template.Must(template.New("").Funcs(template.FuncMap{
	"json": func(v any) template.JS { b, _ := json.Marshal(v); return template.JS(b) }, //nolint:gosec // props are server-owned
	"prop": func(w store.Widget, k string, def any) any {
		if v, ok := w.Props[k]; ok {
			return v
		}
		return def
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
}

// Render produces the whole page for a frame.
func Render(f store.Frame) ([]byte, error) {
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

func known(t string) bool {
	i := sort.SearchStrings(sortedTypes, t)
	return i < len(sortedTypes) && sortedTypes[i] == t
}

var sortedTypes = func() []string { s := append([]string(nil), Types...); sort.Strings(s); return s }()
