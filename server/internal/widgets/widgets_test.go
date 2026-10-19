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
