package photos

import (
	"image"
	"image/color"
	"image/png"
	"os"
	"path/filepath"
	"testing"
)

func writePNG(t *testing.T, p string, w, h int) {
	img := image.NewRGBA(image.Rect(0, 0, w, h))
	for x := 0; x < w; x++ {
		img.Set(x, 0, color.RGBA{255, 0, 0, 255})
	}
	f, _ := os.Create(p)
	defer f.Close()
	if err := png.Encode(f, img); err != nil {
		t.Fatal(err)
	}
}

func TestNextCyclesAndCaches(t *testing.T) {
	root := t.TempDir()
	writePNG(t, filepath.Join(root, "a.png"), 400, 100)
	writePNG(t, filepath.Join(root, "b.png"), 100, 400)
	s, err := New(Folder{Root: root}, filepath.Join(root, "cache"))
	if err != nil {
		t.Fatal(err)
	}
	p1, err := s.Next("f", 64, 32)
	if err != nil {
		t.Fatal(err)
	}
	p2, _ := s.Next("f", 64, 32)
	p3, _ := s.Next("f", 64, 32)
	if p1 == p2 || p1 != p3 {
		t.Fatalf("rotation wrong: %s %s %s", p1, p2, p3)
	}
	fh, _ := os.Open(p1)
	cfg, _, err := image.DecodeConfig(fh)
	fh.Close()
	if err != nil || cfg.Width != 64 || cfg.Height != 32 {
		t.Fatalf("bad output %v %+v", err, cfg)
	}
	if s.Count() != 2 {
		t.Fatal("count")
	}
}

func TestEmptyFolder(t *testing.T) {
	root := t.TempDir()
	s, _ := New(Folder{Root: root}, filepath.Join(root, "c"))
	if _, err := s.Next("f", 10, 10); err != ErrNoPhotos {
		t.Fatalf("want ErrNoPhotos, got %v", err)
	}
}

func TestCoverRect(t *testing.T) {
	r := coverRect(image.Rect(0, 0, 400, 100), 2, 1) // want 200x100 centred
	if r.Dx() != 200 || r.Dy() != 100 || r.Min.X != 100 {
		t.Fatalf("%v", r)
	}
	r = coverRect(image.Rect(0, 0, 100, 400), 2, 1) // want 100x50 centred
	if r.Dx() != 100 || r.Dy() != 50 || r.Min.Y != 175 {
		t.Fatalf("%v", r)
	}
}
