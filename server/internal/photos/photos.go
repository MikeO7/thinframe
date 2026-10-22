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
	"io"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"

	"golang.org/x/image/draw"

	"github.com/MikeO7/thinframe/server/internal/store"
)

// Sizes are the only outputs ever rendered: the panel in both orientations and
// their halves. Requests snap to one of these, so the cache holds at most four
// files per photo and no request allocates more than 1280x800 pixels.
var Sizes = [...]image.Point{{1280, 800}, {800, 1280}, {640, 400}, {400, 640}}

const (
	maxCursors   = 256 // per-frame cursors kept; the map is cleared past this
	cacheHigh    = 500 // cache files that trigger eviction
	cacheLow     = 400 // cache files left after eviction (oldest mtime go first)
	sharedCursor = ""  // cursor for requests whose frame id is not a valid store id
)

// snap returns the entry of Sizes with the same orientation nearest w x h by area.
func snap(w, h int) (int, int) {
	best, bestD := Sizes[0], -1
	for _, c := range Sizes {
		if (c.X >= c.Y) != (w >= h) {
			continue
		}
		d := c.X*c.Y - w*h
		if d < 0 {
			d = -d
		}
		if bestD < 0 || d < bestD {
			best, bestD = c, d
		}
	}
	return best.X, best.Y
}

// encodeJPEG is a variable so tests can force an encode failure.
var encodeJPEG = func(w io.Writer, m image.Image) error {
	return jpeg.Encode(w, m, &jpeg.Options{Quality: 85})
}

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
	keys     map[string]*keyLock // per cache key; one generator at a time
}

type keyLock struct {
	sync.Mutex
	refs int
}

// New returns a Service. cacheDir is created.
func New(src Source, cacheDir string) (*Service, error) {
	if err := os.MkdirAll(cacheDir, 0o755); err != nil {
		return nil, err
	}
	return &Service{src: src, cacheDir: cacheDir, cursor: map[string]int{}, keys: map[string]*keyLock{}}, nil
}

// ErrNoPhotos means the source is empty.
var ErrNoPhotos = errors.New("no photos")

// Next returns the path of a cached JPEG for the given frame's next photo, at the
// entry of Sizes nearest w x h. Frame ids that are not valid store ids share one
// cursor. Rescans the source every 5 minutes. Rotation is "recent first", then cyclic.
func (s *Service) Next(frame string, w, h int) (string, error) {
	w, h = snap(w, h)
	if !store.ValidID(frame) {
		frame = sharedCursor
	}
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
	if _, ok := s.cursor[frame]; !ok && len(s.cursor) >= maxCursors {
		clear(s.cursor)
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

// lock takes the per-key generation lock and returns its release.
func (s *Service) lock(key string) func() {
	s.mu.Lock()
	k := s.keys[key]
	if k == nil {
		k = &keyLock{}
		s.keys[key] = k
	}
	k.refs++
	s.mu.Unlock()
	k.Lock()
	return func() {
		k.Unlock()
		s.mu.Lock()
		if k.refs--; k.refs == 0 {
			delete(s.keys, key)
		}
		s.mu.Unlock()
	}
}

func (s *Service) cached(id string, w, h int) (string, error) {
	st, err := os.Stat(id)
	if err != nil {
		return "", err
	}
	sum := sha1.Sum([]byte(fmt.Sprintf("%s|%d|%dx%d", id, st.ModTime().UnixNano(), w, h)))
	key := hex.EncodeToString(sum[:])
	out := filepath.Join(s.cacheDir, key+".jpg")
	defer s.lock(key)()
	if _, err := os.Stat(out); err == nil {
		now := time.Now()
		_ = os.Chtimes(out, now, now) // keep files in use out of eviction
		return out, nil
	}
	img, err := s.src.Open(id)
	if err != nil {
		return "", err
	}
	dst := image.NewRGBA(image.Rect(0, 0, w, h))
	draw.CatmullRom.Scale(dst, dst.Bounds(), img, coverRect(img.Bounds(), w, h), draw.Src, nil)
	fh, err := os.CreateTemp(s.cacheDir, ".gen-*.tmp")
	if err != nil {
		return "", err
	}
	tmp := fh.Name()
	err = encodeJPEG(fh, dst)
	if cerr := fh.Close(); err == nil {
		err = cerr
	}
	if err == nil {
		err = os.Rename(tmp, out)
	}
	if err != nil {
		os.Remove(tmp)
		return "", err
	}
	s.evict(out)
	return out, nil
}

// evict deletes the oldest cache files by mtime once there are more than
// cacheHigh, leaving cacheLow. keep (the file just written) is never removed.
func (s *Service) evict(keep string) {
	ents, err := os.ReadDir(s.cacheDir)
	if err != nil {
		return
	}
	type ent struct {
		p string
		t time.Time
	}
	var files []ent
	for _, e := range ents {
		if e.IsDir() || filepath.Ext(e.Name()) != ".jpg" {
			continue
		}
		if info, err := e.Info(); err == nil {
			files = append(files, ent{filepath.Join(s.cacheDir, e.Name()), info.ModTime()})
		}
	}
	if len(files) <= cacheHigh {
		return
	}
	sort.Slice(files, func(i, j int) bool { return files[i].t.Before(files[j].t) })
	n := len(files)
	for _, f := range files {
		if n <= cacheLow {
			break
		}
		if f.p != keep && os.Remove(f.p) == nil {
			n--
		}
	}
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
