package photos

import (
	"errors"
	"fmt"
	"image"
	"image/color"
	"image/jpeg"
	"image/png"
	"io"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
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
	if err != nil || cfg.Width != 640 || cfg.Height != 400 { // 64x32 snaps to the smallest landscape size
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

func TestSnap(t *testing.T) {
	for _, c := range []struct{ w, h, ww, wh int }{
		{1280, 800, 1280, 800},
		{800, 1280, 800, 1280},
		{4096, 4096, 1280, 800}, // square counts as landscape; capped at panel size
		{4096, 100, 640, 400},   // 410k px: nearer 256k than 1024k
		{100, 4096, 400, 640},
		{4096, 300, 1280, 800}, // 1229k px
		{1, 1, 640, 400},
		{640, 400, 640, 400},
		{400, 640, 400, 640},
		{1000, 700, 1280, 800}, // 700k px: nearer 1024k than 256k
		{700, 500, 640, 400},   // 350k px: nearer 256k
		{1279, 801, 1280, 800},
	} {
		if w, h := snap(c.w, c.h); w != c.ww || h != c.wh {
			t.Errorf("snap(%d,%d) = %dx%d, want %dx%d", c.w, c.h, w, h, c.ww, c.wh)
		}
	}
}

type countingSource struct {
	Folder
	opens atomic.Int32
}

func (c *countingSource) Open(id string) (image.Image, error) {
	c.opens.Add(1)
	return c.Folder.Open(id)
}

func TestConcurrentSameKey(t *testing.T) {
	root := t.TempDir()
	writePNG(t, filepath.Join(root, "a.png"), 300, 200)
	src := &countingSource{Folder: Folder{Root: root}}
	cache := filepath.Join(root, "cache")
	s, err := New(src, cache)
	if err != nil {
		t.Fatal(err)
	}
	var wg sync.WaitGroup
	paths := make([]string, 16)
	errs := make([]error, 16)
	for i := range paths {
		wg.Add(1)
		go func() {
			defer wg.Done()
			paths[i], errs[i] = s.Next(fmt.Sprintf("f%d", i), 1280, 800)
		}()
	}
	wg.Wait()
	for i := range paths {
		if errs[i] != nil || paths[i] != paths[0] {
			t.Fatalf("request %d: %q %v (want %q)", i, paths[i], errs[i], paths[0])
		}
	}
	if n := src.opens.Load(); n != 1 {
		t.Fatalf("decoded source %d times, want 1", n)
	}
	fh, err := os.Open(paths[0])
	if err != nil {
		t.Fatal(err)
	}
	defer fh.Close()
	img, err := jpeg.Decode(fh)
	if err != nil || img.Bounds().Dx() != 1280 || img.Bounds().Dy() != 800 {
		t.Fatalf("bad jpeg: %v", err)
	}
	if m, _ := filepath.Glob(filepath.Join(cache, "*.tmp")); len(m) != 0 {
		t.Fatalf("temp files left: %v", m)
	}
	if len(s.keys) != 0 {
		t.Fatalf("key locks leaked: %d", len(s.keys))
	}
}

func TestFailedEncodeLeavesNoTemp(t *testing.T) {
	root := t.TempDir()
	writePNG(t, filepath.Join(root, "a.png"), 300, 200)
	cache := filepath.Join(root, "cache")
	s, _ := New(Folder{Root: root}, cache)
	orig := encodeJPEG
	t.Cleanup(func() { encodeJPEG = orig })
	encodeJPEG = func(w io.Writer, _ image.Image) error {
		_, _ = w.Write([]byte("partial"))
		return errors.New("boom")
	}
	if _, err := s.Next("f", 1280, 800); err == nil {
		t.Fatal("want error")
	}
	if ents, _ := os.ReadDir(cache); len(ents) != 0 {
		t.Fatalf("cache not empty after failure: %v", ents)
	}
}

func TestCursorMapBounded(t *testing.T) {
	root := t.TempDir()
	writePNG(t, filepath.Join(root, "a.png"), 300, 200)
	s, _ := New(Folder{Root: root}, filepath.Join(root, "cache"))
	for i := 0; i < 3*maxCursors; i++ {
		if _, err := s.Next(fmt.Sprintf("f%d", i), 640, 400); err != nil {
			t.Fatal(err)
		}
	}
	if n := len(s.cursor); n > maxCursors {
		t.Fatalf("cursor map has %d entries, cap %d", n, maxCursors)
	}
	for _, bad := range []string{"", "UPPER", strings.Repeat("a", 65), "../x"} {
		if _, err := s.Next(bad, 640, 400); err != nil {
			t.Fatal(err)
		}
		if _, ok := s.cursor[bad]; ok && bad != sharedCursor {
			t.Fatalf("invalid id %q got its own cursor", bad)
		}
	}
}

func TestCacheEviction(t *testing.T) {
	root := t.TempDir()
	writePNG(t, filepath.Join(root, "a.png"), 300, 200)
	cache := filepath.Join(root, "cache")
	s, _ := New(Folder{Root: root}, cache)
	old := time.Now().Add(-time.Hour)
	for i := 0; i < cacheHigh; i++ {
		p := filepath.Join(cache, fmt.Sprintf("old%03d.jpg", i))
		if err := os.WriteFile(p, []byte("x"), 0o644); err != nil {
			t.Fatal(err)
		}
		mt := old.Add(time.Duration(i) * time.Second)
		if err := os.Chtimes(p, mt, mt); err != nil {
			t.Fatal(err)
		}
	}
	p, err := s.Next("f", 1280, 800) // the 501st file triggers eviction
	if err != nil {
		t.Fatal(err)
	}
	if ents, _ := os.ReadDir(cache); len(ents) != cacheLow {
		t.Fatalf("cache holds %d files, want %d", len(ents), cacheLow)
	}
	if _, err := os.Stat(p); err != nil {
		t.Fatalf("fresh file evicted: %v", err)
	}
	for i := 0; i < cacheHigh+1-cacheLow; i++ { // the oldest 101 go first
		if _, err := os.Stat(filepath.Join(cache, fmt.Sprintf("old%03d.jpg", i))); !os.IsNotExist(err) {
			t.Fatalf("old%03d.jpg should be evicted: %v", i, err)
		}
	}
}
