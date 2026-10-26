package api

import (
	"bytes"
	"context"
	"fmt"
	"image"
	"image/png"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/MikeO7/thinframe/server/internal/store"
)

// Screenshots: the "screenshot" action makes the frame read its scanned-out framebuffer and
// POST it here as raw RGB in the page orientation (tfctl/src/screenshot.rs). The server keeps
// the last keepShots per frame as PNG; MCP's screenshot tool waits for the next one.

const (
	MaxShot   = 16 << 20
	keepShots = 10
	shotWait  = 25 * time.Second
)

type shotWaiters struct {
	mu sync.Mutex
	m  map[string][]chan []byte
}

func (s *Server) shotDir(id string) string { return filepath.Join(s.data, "screenshots", id) }

// handleShotUpload: POST /api/frames/{id}/screenshot?w=&h= with w*h*3 bytes of RGB.
func (s *Server) handleShotUpload(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	if _, err := s.Store.Get(id); err != nil {
		writeErr(w, err)
		return
	}
	pw, _ := strconv.Atoi(r.URL.Query().Get("w"))
	ph, _ := strconv.Atoi(r.URL.Query().Get("h"))
	if pw < 1 || ph < 1 || pw > 4096 || ph > 4096 || pw*ph*3 > MaxShot {
		http.Error(w, "w and h must be 1..4096 (and w*h*3 at most 16 MiB)", http.StatusBadRequest)
		return
	}
	b, ok := readSmall(w, r, MaxShot)
	if !ok {
		return
	}
	if len(b) != pw*ph*3 {
		http.Error(w, fmt.Sprintf("body is %d bytes, want w*h*3 = %d", len(b), pw*ph*3), http.StatusBadRequest)
		return
	}
	img := image.NewNRGBA(image.Rect(0, 0, pw, ph))
	for i, j := 0, 0; i < len(b); i, j = i+3, j+4 {
		img.Pix[j], img.Pix[j+1], img.Pix[j+2], img.Pix[j+3] = b[i], b[i+1], b[i+2], 0xff
	}
	var buf bytes.Buffer
	if err := (&png.Encoder{CompressionLevel: png.BestSpeed}).Encode(&buf, img); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	if err := s.saveShot(id, buf.Bytes()); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	s.shots.mu.Lock()
	for _, ch := range s.shots.m[id] {
		select {
		case ch <- buf.Bytes():
		default:
		}
	}
	delete(s.shots.m, id)
	s.shots.mu.Unlock()
	w.WriteHeader(http.StatusCreated)
}

func (s *Server) saveShot(id string, b []byte) error {
	dir := s.shotDir(id)
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return err
	}
	name := time.Now().UTC().Format("20060102T150405.000Z") + ".png"
	if err := writeDurable(filepath.Join(dir, name), b); err != nil {
		return err
	}
	list := s.shotNames(id)
	for len(list) > keepShots {
		_ = os.Remove(filepath.Join(dir, list[0]))
		list = list[1:]
	}
	return nil
}

func (s *Server) shotNames(id string) []string {
	ents, _ := os.ReadDir(s.shotDir(id))
	var out []string
	for _, e := range ents {
		if strings.HasSuffix(e.Name(), ".png") {
			out = append(out, e.Name())
		}
	}
	sort.Strings(out)
	return out
}

// LatestScreenshot is the newest stored screenshot of a frame and when it was taken.
func (s *Server) LatestScreenshot(id string) ([]byte, string, error) {
	if !store.ValidID(id) {
		return nil, "", store.ErrNotFound
	}
	list := s.shotNames(id)
	if len(list) == 0 {
		return nil, "", fmt.Errorf("%w: no screenshot of %s yet", store.ErrNotFound, id)
	}
	b, err := os.ReadFile(filepath.Join(s.shotDir(id), list[len(list)-1]))
	return b, strings.TrimSuffix(list[len(list)-1], ".png"), err
}

func (s *Server) handleShotLatest(w http.ResponseWriter, r *http.Request) {
	b, _, err := s.LatestScreenshot(r.PathValue("id"))
	if err != nil {
		writeErr(w, err)
		return
	}
	w.Header().Set("Content-Type", "image/png")
	w.Header().Set("Cache-Control", "no-store")
	_, _ = w.Write(b)
}

// Screenshot asks an online frame for a screenshot and waits for it (PNG).
func (s *Server) Screenshot(ctx context.Context, id string) ([]byte, error) {
	f, err := s.Store.Get(id)
	if err != nil {
		return nil, err
	}
	if !s.Hub.Online(f.ID) {
		return nil, fmt.Errorf("%w: frame %s is not connected", ErrInvalid, id)
	}
	ch := make(chan []byte, 1)
	s.shots.mu.Lock()
	if s.shots.m == nil {
		s.shots.m = map[string][]chan []byte{}
	}
	s.shots.m[id] = append(s.shots.m[id], ch)
	s.shots.mu.Unlock()
	if _, err := s.Act(id, "screenshot", nil); err != nil {
		return nil, err
	}
	select {
	case b := <-ch:
		return b, nil
	case <-ctx.Done():
		return nil, ctx.Err()
	case <-time.After(shotWait):
		return nil, fmt.Errorf("frame %s sent no screenshot within %s (get_logs shows its screenshot event)", id, shotWait)
	}
}

// ScalePNG shrinks a PNG to width (box filter); width <= 0 or not smaller returns it as is.
func ScalePNG(b []byte, width int) ([]byte, error) {
	src, err := png.Decode(bytes.NewReader(b))
	if err != nil {
		return nil, err
	}
	sb := src.Bounds()
	if width <= 0 || width >= sb.Dx() {
		return b, nil
	}
	height := max(1, sb.Dy()*width/sb.Dx())
	dst := image.NewNRGBA(image.Rect(0, 0, width, height))
	for y := range height {
		y0, y1 := sb.Min.Y+y*sb.Dy()/height, sb.Min.Y+max((y+1)*sb.Dy()/height, y*sb.Dy()/height+1)
		for x := range width {
			x0, x1 := sb.Min.X+x*sb.Dx()/width, sb.Min.X+max((x+1)*sb.Dx()/width, x*sb.Dx()/width+1)
			var r, g, bl, n uint32
			for yy := y0; yy < y1; yy++ {
				for xx := x0; xx < x1; xx++ {
					cr, cg, cb, _ := src.At(xx, yy).RGBA()
					r, g, bl, n = r+cr, g+cg, bl+cb, n+1
				}
			}
			i := dst.PixOffset(x, y)
			dst.Pix[i], dst.Pix[i+1], dst.Pix[i+2], dst.Pix[i+3] = uint8(r/n>>8), uint8(g/n>>8), uint8(bl/n>>8), 0xff
		}
	}
	var out bytes.Buffer
	err = (&png.Encoder{CompressionLevel: png.BestSpeed}).Encode(&out, dst)
	return out.Bytes(), err
}
