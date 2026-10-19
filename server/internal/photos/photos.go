// Package photos: folder adapter. Scans PHOTOS_DIR, resizes once per target size,
// caches JPEGs under cacheDir, hands each frame the next photo in its own rotation.
package photos

import (
	"crypto/sha1"
	"encoding/hex"
	"errors"
	"fmt"
	"image"
	"image/jpeg"
	_ "image/png" // register decoder
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"

	"golang.org/x/image/draw"
)

// Source lists photos and opens one. Folder is the first implementation.
type Source interface {
	List() ([]string, error) // stable ids (paths for the folder source)
	Open(id string) (image.Image, error)
}

// Folder is a Source over a directory tree.
type Folder struct{ Root string }

var exts = map[string]bool{".jpg": true, ".jpeg": true, ".png": true}

// List walks the tree once; results are sorted newest first.
func (f Folder) List() ([]string, error) {
	type ent struct {
		p string
		t time.Time
	}
	var found []ent
	err := filepath.WalkDir(f.Root, func(p string, d os.DirEntry, err error) error {
		if err != nil || d.IsDir() || !exts[strings.ToLower(filepath.Ext(p))] || strings.HasPrefix(d.Name(), ".") {
			return nil //nolint:nilerr // skip unreadable entries
		}
		info, e := d.Info()
		if e != nil {
			return nil //nolint:nilerr
		}
		found = append(found, ent{p, info.ModTime()})
		return nil
	})
	if err != nil {
		return nil, err
	}
	sort.Slice(found, func(i, j int) bool { return found[i].t.After(found[j].t) })
	out := make([]string, len(found))
	for i, e := range found {
		out[i] = e.p
	}
	return out, nil
}

// Open decodes one file.
func (f Folder) Open(id string) (image.Image, error) {
	if !strings.HasPrefix(filepath.Clean(id), filepath.Clean(f.Root)) {
		return nil, errors.New("path outside root")
	}
	fh, err := os.Open(id)
	if err != nil {
		return nil, err
	}
	defer fh.Close()
	img, _, err := image.Decode(fh)
	return img, err
}

// Service wraps a Source with a cache and per-frame cursors.
type Service struct {
	src      Source
	cacheDir string
	mu       sync.Mutex
	list     []string
	listedAt time.Time
	cursor   map[string]int
}

// New returns a Service. cacheDir is created.
func New(src Source, cacheDir string) (*Service, error) {
	if err := os.MkdirAll(cacheDir, 0o755); err != nil {
		return nil, err
	}
	return &Service{src: src, cacheDir: cacheDir, cursor: map[string]int{}}, nil
}

// ErrNoPhotos means the source is empty.
var ErrNoPhotos = errors.New("no photos")

// Next returns the path of a cached JPEG at w x h for the given frame's next photo.
// Rescans the source every 5 minutes. Rotation is "recent first", then cyclic.
func (s *Service) Next(frame string, w, h int) (string, error) {
	s.mu.Lock()
	if time.Since(s.listedAt) > 5*time.Minute || s.list == nil {
		l, err := s.src.List()
		if err != nil {
			s.mu.Unlock()
			return "", err
		}
		s.list, s.listedAt = l, time.Now()
	}
	if len(s.list) == 0 {
		s.mu.Unlock()
		return "", ErrNoPhotos
	}
	i := s.cursor[frame] % len(s.list)
	s.cursor[frame] = i + 1
	id := s.list[i]
	s.mu.Unlock()
	return s.cached(id, w, h)
}

// Count reports how many photos the last scan found.
func (s *Service) Count() int {
	s.mu.Lock()
	defer s.mu.Unlock()
	return len(s.list)
}

func (s *Service) cached(id string, w, h int) (string, error) {
	st, err := os.Stat(id)
	if err != nil {
		return "", err
	}
	sum := sha1.Sum([]byte(fmt.Sprintf("%s|%d|%dx%d", id, st.ModTime().UnixNano(), w, h)))
	out := filepath.Join(s.cacheDir, hex.EncodeToString(sum[:])+".jpg")
	if _, err := os.Stat(out); err == nil {
		return out, nil
	}
	img, err := s.src.Open(id)
	if err != nil {
		return "", err
	}
	dst := image.NewRGBA(image.Rect(0, 0, w, h))
	draw.CatmullRom.Scale(dst, dst.Bounds(), img, coverRect(img.Bounds(), w, h), draw.Src, nil)
	tmp := out + ".tmp"
	fh, err := os.Create(tmp)
	if err != nil {
		return "", err
	}
	if err := jpeg.Encode(fh, dst, &jpeg.Options{Quality: 85}); err != nil {
		fh.Close()
		return "", err
	}
	fh.Close()
	return out, os.Rename(tmp, out)
}

// coverRect picks the largest centred sub-rectangle of src with the w:h aspect.
func coverRect(src image.Rectangle, w, h int) image.Rectangle {
	sw, sh := src.Dx(), src.Dy()
	if sw*h > sh*w { // too wide: crop width
		nw := sh * w / h
		x := src.Min.X + (sw-nw)/2
		return image.Rect(x, src.Min.Y, x+nw, src.Max.Y)
	}
	nh := sw * h / w
	y := src.Min.Y + (sh-nh)/2
	return image.Rect(src.Min.X, y, src.Max.X, y+nh)
}
