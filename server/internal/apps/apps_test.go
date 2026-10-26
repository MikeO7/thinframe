package apps

import (
	"errors"
	"io"
	"strings"
	"testing"
)

func TestPutVersionsRollback(t *testing.T) {
	s := Open(t.TempDir())
	m, err := s.Put("clock", "a clock", "first", map[string][]byte{"index.html": []byte("v1"), "js/app.js": []byte("x")})
	if err != nil || m.Current != 1 || len(m.Versions) != 1 {
		t.Fatalf("put: %+v %v", m, err)
	}
	m, err = s.Put("clock", "", "second", map[string][]byte{"index.html": []byte("v2")})
	if err != nil || m.Current != 2 || m.Description != "a clock" {
		t.Fatalf("put 2: %+v %v", m, err)
	}
	f, v, err := s.File("clock", "index.html", 0)
	if err != nil || v != 2 {
		t.Fatal(err, v)
	}
	b, _ := io.ReadAll(f)
	_ = f.Close()
	if string(b) != "v2" {
		t.Fatalf("current = %q", b)
	}
	if _, _, err := s.File("clock", "js/app.js", 0); !errors.Is(err, ErrNotFound) {
		t.Fatalf("v2 has no js/app.js: %v", err)
	}
	if m, err = s.Rollback("clock", 0); err != nil || m.Current != 1 {
		t.Fatalf("rollback: %+v %v", m, err)
	}
	files, v, err := s.Files("clock", 0)
	if err != nil || v != 1 || string(files["js/app.js"]) != "x" {
		t.Fatalf("files after rollback: %v %d %v", files, v, err)
	}
	if _, err = s.Rollback("clock", 0); !errors.Is(err, ErrNotFound) {
		t.Fatalf("rollback before v1: %v", err)
	}
	if _, err = s.Rollback("clock", 9); !errors.Is(err, ErrNotFound) {
		t.Fatalf("rollback to v9: %v", err)
	}
	// A put after a rollback is a new number, not a reuse of v2.
	if m, _ = s.Put("clock", "", "", map[string][]byte{"index.html": []byte("v3")}); m.Current != 3 {
		t.Fatalf("after rollback put = v%d", m.Current)
	}
}

func TestKeepsMaxVersions(t *testing.T) {
	s := Open(t.TempDir())
	for i := 0; i < MaxVersions+3; i++ {
		if _, err := s.Put("a", "", "", map[string][]byte{"index.html": []byte("x")}); err != nil {
			t.Fatal(err)
		}
	}
	m, _ := s.Get("a")
	if len(m.Versions) != MaxVersions || m.Versions[0].N != 4 || m.Current != MaxVersions+3 {
		t.Fatalf("versions: %+v", m)
	}
	if _, _, err := s.File("a", "index.html", 1); err == nil {
		t.Fatal("pruned v1 still served")
	}
}

func TestRefuses(t *testing.T) {
	s := Open(t.TempDir())
	ok := []byte("x")
	for _, c := range []struct {
		name  string
		files map[string][]byte
	}{
		{"Bad", map[string][]byte{"index.html": ok}},
		{"a/b", map[string][]byte{"index.html": ok}},
		{"x", map[string][]byte{"main.html": ok}},
		{"x", map[string][]byte{"index.html": ok, "../up.html": ok}},
		{"x", map[string][]byte{"index.html": ok, ".hidden.js": ok}},
		{"x", map[string][]byte{"index.html": ok, "a/./b.js": ok}},
		{"x", map[string][]byte{"index.html": ok, "/abs.js": ok}},
		{"x", map[string][]byte{"index.html": ok, "run.sh": ok}},
		{"x", map[string][]byte{"index.html": ok, "a//b.js": ok}},
		{"x", map[string][]byte{"index.html": ok, `a\b.js`: ok}},
		{"x", map[string][]byte{"index.html": []byte(strings.Repeat("x", MaxBytes+1))}},
	} {
		if _, err := s.Put(c.name, "", "", c.files); !errors.Is(err, ErrInvalid) {
			t.Errorf("put %q %v: %v", c.name, keys(c.files), err)
		}
	}
	if len(s.List()) != 0 {
		t.Fatal("a refused put left an app behind")
	}
	if _, _, err := s.File("x", "../meta.json", 0); !errors.Is(err, ErrNotFound) {
		t.Fatal("traversal", err)
	}
}

func TestDeleteList(t *testing.T) {
	s := Open(t.TempDir())
	_, _ = s.Put("b", "", "", map[string][]byte{"index.html": []byte("x")})
	_, _ = s.Put("a", "", "", map[string][]byte{"index.html": []byte("x")})
	if l := s.List(); len(l) != 2 || l[0].Name != "a" {
		t.Fatalf("list %+v", l)
	}
	if err := s.Delete("a"); err != nil || s.Exists("a") {
		t.Fatal(err)
	}
	if err := s.Delete("a"); !errors.Is(err, ErrNotFound) {
		t.Fatal(err)
	}
}

func keys(m map[string][]byte) []string {
	var out []string
	for k := range m {
		out = append(out, k)
	}
	return out
}

func TestState(t *testing.T) {
	s := Open(t.TempDir())
	if _, err := s.SetState("nope", "k", []byte(`1`)); !errors.Is(err, ErrNotFound) {
		t.Fatal(err)
	}
	_, _ = s.Put("todo", "", "", map[string][]byte{"index.html": []byte("x")})
	if ch, err := s.SetState("todo", "items", []byte(`["a"]`)); !ch || err != nil {
		t.Fatal(ch, err)
	}
	if ch, _ := s.SetState("todo", "items", []byte(`["a"]`)); ch {
		t.Fatal("same value reported as a change")
	}
	if _, err := s.SetState("todo", "Bad Key", []byte(`1`)); !errors.Is(err, ErrInvalid) {
		t.Fatal(err)
	}
	if _, err := s.SetState("todo", "k", []byte(`{nope`)); !errors.Is(err, ErrInvalid) {
		t.Fatal(err)
	}
	// A put keeps the state; null deletes a key.
	_, _ = s.Put("todo", "", "", map[string][]byte{"index.html": []byte("y")})
	st, _ := s.State("todo")
	if string(st["items"]) != `["a"]` {
		t.Fatalf("state after put: %s", st["items"])
	}
	_, _ = s.SetState("todo", "items", []byte(`null`))
	if st, _ = s.State("todo"); len(st) != 0 {
		t.Fatalf("state after null: %v", st)
	}
	big := []byte(`"` + strings.Repeat("x", MaxStateValue-100) + `"`)
	for i := 0; i < 4; i++ {
		_, _ = s.SetState("todo", "k"+string(rune('a'+i)), big)
	}
	if _, err := s.SetState("todo", "kz", big); !errors.Is(err, ErrInvalid) {
		t.Fatal("over MaxState accepted", err)
	}
}
