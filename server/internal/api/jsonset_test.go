package api

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// A corrupt hooks.json is set aside, byte for byte, before anything can replace it.
func TestCorruptSetFileIsKept(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "hooks.json")
	torn := []byte(`{"lights": {"url": "http://ha.lan/api", "headers": {"Authorization": "Bearer s3cret"`)
	if err := os.WriteFile(path, torn, 0o600); err != nil {
		t.Fatal(err)
	}
	set := loadSet[Hook](path)
	if len(set.keys()) != 0 {
		t.Fatalf("a torn file loaded %v", set.keys())
	}
	if err := set.put("new", Hook{URL: "http://x.lan/"}, MaxHooks); err != nil {
		t.Fatal(err)
	}
	matches, _ := filepath.Glob(path + ".corrupt-*")
	if len(matches) != 1 {
		t.Fatalf("set-aside copies: %v, want one", matches)
	}
	if b, _ := os.ReadFile(matches[0]); string(b) != string(torn) {
		t.Fatalf("the set-aside copy is %q", b)
	}
	if b, _ := os.ReadFile(path); !strings.Contains(string(b), `"new"`) {
		t.Fatalf("hooks.json after a change: %s", b)
	}
	// A missing file is the normal first start: nothing is set aside.
	other := filepath.Join(dir, "polls.json")
	if set := loadSet[Poll](other); len(set.keys()) != 0 {
		t.Fatal("a missing file loaded entries")
	}
	if matches, _ := filepath.Glob(other + ".corrupt-*"); len(matches) != 0 {
		t.Fatalf("a missing file was 'set aside': %v", matches)
	}
}
