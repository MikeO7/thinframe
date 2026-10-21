package store

import (
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
