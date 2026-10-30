package widgets

import (
	"encoding/json"
	"os"
	"strings"
	"testing"

	"github.com/MikeO7/thinframe/server/internal/store"
)

func demoLayout(t *testing.T) store.Layout {
	t.Helper()
	b, err := os.ReadFile("../../examples/obsidian/layout.json")
	if err != nil {
		t.Fatal(err)
	}
	var l store.Layout
	if err := json.Unmarshal(b, &l); err != nil {
		t.Fatal(err)
	}
	return l
}

// The demo shipped in examples/ must stay valid as types and rules change.
func TestDemoLayoutRenders(t *testing.T) {
	l := demoLayout(t)
	if err := Validate(l); err != nil {
		t.Fatal(err)
	}
	b, err := Render(store.Frame{ID: "demo", Config: store.DefaultConfig(), Layout: l})
	if err != nil {
		t.Fatal(err)
	}
	s := string(b)
	for _, want := range []string{`class="tabbed nav-left"`, `data-page="security"`, `class="page on" data-page="home"`,
		`data-for="house"`, `>Security &amp; energy<`, `class="wp"`, `/api/data/asset/inter.woff2`, `id="w-server"`} {
		if !strings.Contains(s, want) {
			t.Errorf("missing %q", want)
		}
	}
	if strings.Count(s, `<main class="page`) != len(l.Tabs) {
		t.Errorf("want one page per tab")
	}
	if !strings.Contains(s, `data-tab="home" data-icon="home"`) {
		t.Error("tabs carry no data-icon for themes")
	}
	l.Theme = "aurora"
	b, _ = Render(store.Frame{ID: "demo", Config: store.DefaultConfig(), Layout: l})
	if !strings.Contains(string(b), "[data-mood=car]") || !strings.Contains(string(b), ".clock-time{font-weight:300}") {
		t.Error("aurora renders without its CSS on top of obsidian")
	}
	// Manrope only: aurora asking for the Inter file it does not use got a 404 on every load.
	if strings.Contains(string(b), "inter.woff2") || !strings.Contains(string(b), "font-display:swap}") {
		t.Error("aurora still loads Inter, or blocks text on Manrope")
	}
	if strings.Contains(string(b), "<feTurbulence") || !strings.Contains(string(b), "/static/grain-") {
		t.Error("aurora's grain is not the baked tile")
	}
	l.Nav = "bottom"
	b, _ = Render(store.Frame{ID: "demo", Config: store.DefaultConfig(), Layout: l})
	if !strings.Contains(string(b), `class="tabbed nav-bottom"`) {
		t.Error("nav bottom not rendered")
	}
}

func TestValidateTabs(t *testing.T) {
	base := func() store.Layout {
		return store.Layout{Cols: 2, Rows: 1, Tabs: []store.Tab{{ID: "a", Title: "A", Icon: "home"}, {ID: "b", Title: "B", Parent: "a"}},
			Widgets: []store.Widget{{ID: "x", Type: "text", Tab: "a", W: 2, H: 1}, {ID: "y", Type: "text", Tab: "b", W: 2, H: 1}}}
	}
	if err := Validate(base()); err != nil {
		t.Fatalf("same cells on two tabs must be fine: %v", err)
	}
	cases := map[string]func(*store.Layout){
		"unknown tab":      func(l *store.Layout) { l.Widgets[0].Tab = "zz" },
		"overlaps":         func(l *store.Layout) { l.Widgets[1].Tab = "a" },
		"unknown icon":     func(l *store.Layout) { l.Tabs[0].Icon = "rocket" },
		"first tab":        func(l *store.Layout) { l.Tabs[0].Parent = "b" },
		"top-level":        func(l *store.Layout) { l.Tabs = append(l.Tabs, store.Tab{ID: "c", Title: "C", Parent: "b"}) },
		"unique":           func(l *store.Layout) { l.Tabs[1].ID = "a" },
		"title":            func(l *store.Layout) { l.Tabs[1].Title = "" },
		"no tabs":          func(l *store.Layout) { l.Tabs = nil },
		"idle_s":           func(l *store.Layout) { l.IdleS = -1 },
		"nav must be":      func(l *store.Layout) { l.Nav = "top" },
		"source must be a": func(l *store.Layout) { l.Widgets[0].Props = map[string]any{"source": "../x"} },
	}
	for want, mut := range cases {
		l := base()
		mut(&l)
		if err := Validate(l); err == nil || !strings.Contains(err.Error(), want) {
			t.Errorf("%s: got %v", want, err)
		}
	}
}

// Props reach the page as JSON; a prop cannot end the script element.
func TestPropsCannotCloseScript(t *testing.T) {
	h, err := renderWidget(store.Widget{ID: "x", Type: "list", W: 1, H: 1, Props: map[string]any{"title": "</script><script>alert(1)</script>"}})
	if err != nil {
		t.Fatal(err)
	}
	if strings.Count(string(h), "</script>") != 1 {
		t.Fatalf("prop escaped the script element: %s", h)
	}
}
