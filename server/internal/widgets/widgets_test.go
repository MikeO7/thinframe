package widgets

import (
	"strings"
	"testing"

	"github.com/MikeO7/thinframe/server/internal/store"
)

func TestRenderDefaultLayout(t *testing.T) {
	f := store.Frame{ID: "kitchen", Config: store.DefaultConfig(), Layout: store.DefaultLayout()}
	b, err := Render(f)
	if err != nil {
		t.Fatal(err)
	}
	s := string(b)
	for _, want := range []string{`id="w-clock"`, `id="w-photo"`, `grid-column:3 / span 2`, `thinframe-page-version`, `EventSource`} {
		if !strings.Contains(s, want) {
			t.Errorf("missing %q", want)
		}
	}
}

func TestValidate(t *testing.T) {
	ok := store.DefaultLayout()
	if err := Validate(ok); err != nil {
		t.Fatal(err)
	}
	bad := ok
	bad.Widgets = append(bad.Widgets, store.Widget{ID: "x", Type: "text", X: 0, Y: 0, W: 1, H: 1})
	if err := Validate(bad); err == nil || !strings.Contains(err.Error(), "overlaps") {
		t.Fatalf("overlap not caught: %v", err)
	}
	bad = ok
	bad.Widgets = []store.Widget{{ID: "x", Type: "nope", W: 1, H: 1}}
	if err := Validate(bad); err == nil || !strings.Contains(err.Error(), "unknown type") {
		t.Fatalf("unknown type not caught: %v", err)
	}
	bad.Widgets = []store.Widget{{ID: "x", Type: "text", X: 3, Y: 0, W: 2, H: 1}}
	if err := Validate(bad); err == nil || !strings.Contains(err.Error(), "outside") {
		t.Fatalf("bounds not caught: %v", err)
	}
}

func TestUnknownTypeRendersPlaceholder(t *testing.T) {
	f := store.Frame{ID: "a", Layout: store.Layout{Cols: 1, Rows: 1, Widgets: []store.Widget{{ID: "q", Type: "zzz", W: 1, H: 1}}}}
	b, err := Render(f)
	if err != nil || !strings.Contains(string(b), "unknown widget type") {
		t.Fatalf("%v %s", err, b)
	}
}

func TestCalendarWidgetRenders(t *testing.T) {
	f := store.Frame{ID: "k", Config: store.DefaultConfig(), Layout: store.Layout{Cols: 4, Rows: 3, Widgets: []store.Widget{
		{ID: "cal", Type: "calendar", X: 0, Y: 0, W: 2, H: 1}}}}
	b, err := Render(f)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(b), `data-field="l1"`) || strings.Contains(string(b), `class="unknown"`) {
		t.Errorf("calendar widget missing: %s", b)
	}
}

func iframeLayout(src any) store.Layout {
	w := store.Widget{ID: "web", Type: "iframe", W: 1, H: 1}
	if src != nil {
		w.Props = map[string]any{"src": src}
	}
	return store.Layout{Cols: 1, Rows: 1, Widgets: []store.Widget{w}}
}

func TestValidateIframeSrc(t *testing.T) {
	for _, src := range []any{"", "/frame/kitchen", "frame/kitchen", "//evil.example/x", "javascript:alert(1)",
		"file:///etc/passwd", "data:text/html,hi", "about:blank", "https:", "https:///nohost", "ftp://h/x", 42} {
		if err := Validate(iframeLayout(src)); err == nil || !strings.Contains(err.Error(), "absolute http:// or https://") {
			t.Errorf("src %v accepted: %v", src, err)
		}
	}
	for _, src := range []any{nil, "https://example.com/dash?x=1", "http://192.168.1.5:8123/"} {
		if err := Validate(iframeLayout(src)); err != nil {
			t.Errorf("src %v rejected: %v", src, err)
		}
	}
}

func TestIframeRendersSandboxed(t *testing.T) {
	for _, c := range []struct {
		src  any
		want string
	}{
		{"https://example.com/dash?x=1&y=2", "https://example.com/dash?x=1&amp;y=2"},
		{nil, "about:blank"},              // unset: not the filtered, relative #ZgotmplZ (this page again)
		{"/frame/kitchen", "about:blank"}, // stored before Validate checked src
	} {
		b, err := Render(store.Frame{ID: "k", Layout: iframeLayout(c.src)})
		if err != nil {
			t.Fatal(err)
		}
		s := string(b)
		if !strings.Contains(s, `<iframe src="`+c.want+`" sandbox="allow-scripts" loading="lazy" referrerpolicy="no-referrer">`) {
			t.Errorf("src %v: want %s in %s", c.src, c.want, s)
		}
		if strings.Contains(s, "allow-same-origin") || strings.Contains(s, "ZgotmplZ") {
			t.Errorf("src %v: unsafe iframe markup: %s", c.src, s)
		}
	}
}

func TestPageAssets(t *testing.T) {
	b, err := Render(store.Frame{ID: "k", Layout: store.DefaultLayout()})
	if err != nil {
		t.Fatal(err)
	}
	s := string(b)
	for _, want := range []string{
		"img.replaceWith(n); img = n;",                    // the preloaded photo is the one shown
		"clearTimeout(toastT); toastT = setTimeout(",      // a second toast gets its full 2.5 s
		"transition:opacity .6s ease;will-change:opacity", // one layer per photo, for the fade
	} {
		if !strings.Contains(s, want) {
			t.Errorf("page missing %q", want)
		}
	}
	for _, bad := range []string{"img.src = n.src", "will-change:transform"} {
		if strings.Contains(s, bad) {
			t.Errorf("page still has %q", bad)
		}
	}
	if strings.Count(s, "will-change") != 1 {
		t.Errorf("will-change should appear only on .w-photo img")
	}
}
