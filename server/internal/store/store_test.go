package store

import (
	"os"
	"path/filepath"
	"testing"
)

func TestEnsureCreatesDefaults(t *testing.T) {
	s, err := Open(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	f, err := s.Ensure("kitchen")
	if err != nil {
		t.Fatal(err)
	}
	if f.Config.Rotation != 3 || f.Config.Brightness != 100 || len(f.Layout.Widgets) == 0 {
		t.Fatalf("defaults not applied: %+v", f.Config)
	}
	if _, err := s.Ensure("Bad ID"); err == nil {
		t.Fatal("invalid id accepted")
	}
}

func TestPersistsAcrossOpen(t *testing.T) {
	dir := t.TempDir()
	s, _ := Open(dir)
	_, err := s.Update("a", func(f *Frame) { f.Config.Rotation = 1; f.Name = "Hall" })
	if err != nil {
		t.Fatal(err)
	}
	s2, err := Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	f, err := s2.Get("a")
	if err != nil || f.Config.Rotation != 1 || f.Name != "Hall" {
		t.Fatalf("not persisted: %+v %v", f, err)
	}
}

func TestAckMovesToHistory(t *testing.T) {
	s, _ := Open(t.TempDir())
	f, _ := s.Update("a", func(f *Frame) {
		f.Pending = append(f.Pending, Action{ID: "x1", Name: "reboot"})
	})
	if len(f.Pending) != 1 {
		t.Fatal("pending not stored")
	}
	f, _ = s.Update("a", func(f *Frame) {
		if !Ack(f, "x1", true, "") {
			t.Fatal("ack failed")
		}
		if Ack(f, "x1", true, "") {
			t.Fatal("double ack accepted")
		}
	})
	if len(f.Pending) != 0 || len(f.History) != 1 || f.History[0].OK == nil || !*f.History[0].OK {
		t.Fatalf("history wrong: %+v", f)
	}
}

func TestGetReturnsCopy(t *testing.T) {
	s, _ := Open(t.TempDir())
	_, _ = s.Ensure("a")
	f, _ := s.Get("a")
	f.Config.Rotation = 0
	g, _ := s.Get("a")
	if g.Config.Rotation != 3 {
		t.Fatal("Get leaked internal pointer")
	}
}

func TestTouchKeepsLastSamples(t *testing.T) {
	var tc Touch
	for i := 1; i <= MaxTouchSamples+5; i++ {
		tc.Add(i, 0, tc.At)
	}
	if len(tc.SamplesMS) != MaxTouchSamples || tc.SamplesMS[0] != 6 || tc.Max() != MaxTouchSamples+5 {
		t.Fatalf("samples %v", tc.SamplesMS)
	}
}

// A torn frame file (power cut mid-write) is set aside, not fatal: the other frames still load.
func TestOpenSetsAsideCorruptFile(t *testing.T) {
	dir := t.TempDir()
	s, _ := Open(dir)
	if _, err := s.Update("good", func(f *Frame) { f.Name = "ok" }); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "frames", "torn.json"), []byte(`{"id":"to`), 0o644); err != nil {
		t.Fatal(err)
	}
	s2, err := Open(dir)
	if err != nil {
		t.Fatalf("open failed on a torn file: %v", err)
	}
	if f, err := s2.Get("good"); err != nil || f.Name != "ok" {
		t.Fatalf("good frame lost: %+v %v", f, err)
	}
	if _, err := os.Stat(filepath.Join(dir, "frames", "torn.json.bad")); err != nil {
		t.Fatal("torn file was not set aside")
	}
}

// When the save fails, memory keeps the old state: callers never act on a change that is not on disk.
func TestUpdateKeepsMemoryWhenSaveFails(t *testing.T) {
	if os.Getuid() == 0 {
		t.Skip("root ignores directory modes")
	}
	dir := t.TempDir()
	s, _ := Open(dir)
	if _, err := s.Update("a", func(f *Frame) { f.Name = "before" }); err != nil {
		t.Fatal(err)
	}
	frames := filepath.Join(dir, "frames")
	if err := os.Chmod(frames, 0o500); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Chmod(frames, 0o755) })
	if _, err := s.Update("a", func(f *Frame) { f.Name = "after" }); err == nil {
		t.Fatal("save should fail on a read-only directory")
	}
	if f, _ := s.Get("a"); f.Name != "before" {
		t.Fatalf("memory took a change that is not on disk: %q", f.Name)
	}
}

func TestPendingIsCapped(t *testing.T) {
	s, _ := Open(t.TempDir())
	f, err := s.Update("a", func(f *Frame) {
		for i := 0; i < MaxPending+10; i++ {
			f.Pending = append(f.Pending, Action{Name: "reboot"})
		}
	})
	if err != nil || len(f.Pending) != MaxPending {
		t.Fatalf("pending %d, err %v", len(f.Pending), err)
	}
}
