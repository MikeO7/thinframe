package api

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"image"
	"image/color"
	"image/jpeg"
	_ "image/png" // posters and logos come as PNG too
	"io"
	"net"
	"net/http"
	"net/netip"
	"net/url"
	"strconv"
	"sync"
	"time"
)

// Remote pictures named in source data (show posters, logos) are fetched here, shrunk and
// cached, and the page loads the small copy from the server. The frame then never decodes a
// 2000x3000 poster (tens of MB in memory, seconds of A7 time) and never needs TLS to the
// internet itself; on the frame they did not show at all [V 2026-10-26]. Only URLs that appear
// in a pushed source are served, so this is not an open proxy.

const (
	imgMaxBytes = 20 << 20
	imgKeep     = 160 // cached pictures, a few KB to ~30 KB each
	imgTimeout  = 15 * time.Second
)

// imgWidths are the sizes served; a request is rounded up to one, so the cache stays small.
var imgWidths = []int{96, 160, 320, 640}

type imgCache struct {
	mu      sync.Mutex
	m       map[string][]byte
	order   []string
	pending map[string]chan struct{}
	client  *http.Client
}

func newImgCache() *imgCache {
	return &imgCache{m: map[string][]byte{}, pending: map[string]chan struct{}{},
		client: &http.Client{Timeout: imgTimeout, CheckRedirect: imgRedirect}}
}

// imgRedirect follows at most 5 redirects, and none from a public host into the LAN. The URL a
// source names is trusted (pushing a source takes FRAME_TOKEN), and may be a LAN service's
// poster; where a public server redirects to is not, so an internet picture cannot make the
// server fetch from loopback, link-local or private addresses.
func imgRedirect(req *http.Request, via []*http.Request) error {
	if len(via) >= 5 {
		return errors.New("image: too many redirects")
	}
	ctx := req.Context()
	if !hostIsPublic(ctx, via[0].URL.Hostname()) || hostIsPublic(ctx, req.URL.Hostname()) {
		return nil
	}
	return fmt.Errorf("image: refusing a redirect from %s to non-public %s", via[0].URL.Host, req.URL.Host)
}

// hostIsPublic reports whether every address host has (resolved if a name) is a public unicast
// one. A name that does not resolve counts as not public.
func hostIsPublic(ctx context.Context, host string) bool {
	var addrs []netip.Addr
	if a, err := netip.ParseAddr(host); err == nil {
		addrs = []netip.Addr{a}
	} else {
		ips, err := net.DefaultResolver.LookupNetIP(ctx, "ip", host)
		if err != nil || len(ips) == 0 {
			return false
		}
		addrs = ips
	}
	for _, a := range addrs {
		a = a.Unmap()
		if !a.IsGlobalUnicast() || a.IsPrivate() || a.IsLoopback() || a.IsLinkLocalUnicast() || cgnat.Contains(a) {
			return false
		}
	}
	return true
}

// cgnat is 100.64.0.0/10 (carrier-grade NAT, and Tailscale's addresses): not the internet.
var cgnat = netip.MustParsePrefix("100.64.0.0/10")

// get returns the JPEG for (u, w), fetching it once however many requests ask at the same time.
func (c *imgCache) get(ctx context.Context, u string, w int) ([]byte, error) {
	key := strconv.Itoa(w) + " " + u
	for {
		c.mu.Lock()
		if b, ok := c.m[key]; ok {
			c.mu.Unlock()
			return b, nil
		}
		ch, busy := c.pending[key]
		if !busy {
			ch = make(chan struct{})
			c.pending[key] = ch
			c.mu.Unlock()
			b, err := c.fetch(ctx, u, w)
			c.mu.Lock()
			delete(c.pending, key)
			if err == nil {
				c.m[key] = b
				c.order = append(c.order, key)
				for len(c.order) > imgKeep {
					delete(c.m, c.order[0])
					c.order = c.order[1:]
				}
			}
			close(ch)
			c.mu.Unlock()
			return b, err
		}
		c.mu.Unlock()
		select {
		case <-ch: // the other request finished: look again (it may have failed)
		case <-ctx.Done():
			return nil, ctx.Err()
		}
	}
}

func (c *imgCache) fetch(ctx context.Context, u string, w int) ([]byte, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, u, nil)
	if err != nil {
		return nil, err
	}
	resp, err := c.client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("image: %s", resp.Status)
	}
	body, err := io.ReadAll(io.LimitReader(resp.Body, imgMaxBytes))
	if err != nil {
		return nil, err
	}
	src, err := decodeBounded(body)
	if err != nil {
		return nil, err
	}
	var out bytes.Buffer
	if err := jpeg.Encode(&out, shrink(src, w), &jpeg.Options{Quality: 82}); err != nil {
		return nil, err
	}
	return out.Bytes(), nil
}

// imgMaxSide and imgMaxPixels bound a picture before it is decoded. The byte limit alone does
// not: a 242 KB PNG of 16000x16000 flat pixels made the server allocate 244 MB. 40 MP is a
// large camera photo, decoded at 4 bytes a pixel, 160 MB at worst.
const (
	imgMaxSide   = 8192
	imgMaxPixels = 40_000_000
)

// decodeBounded decodes b after checking the dimensions its header claims.
func decodeBounded(b []byte) (image.Image, error) {
	cfg, _, err := image.DecodeConfig(bytes.NewReader(b))
	if err != nil {
		return nil, err
	}
	if cfg.Width <= 0 || cfg.Height <= 0 || cfg.Width > imgMaxSide || cfg.Height > imgMaxSide ||
		cfg.Width*cfg.Height > imgMaxPixels {
		return nil, fmt.Errorf("image: %dx%d is too large (at most %d a side and %d pixels)",
			cfg.Width, cfg.Height, imgMaxSide, imgMaxPixels)
	}
	src, _, err := image.Decode(bytes.NewReader(b))
	return src, err
}

// shrink scales src down to width w by averaging each block of source pixels (a box filter).
// A picture already narrower is returned as it is.
func shrink(src image.Image, w int) image.Image {
	b := src.Bounds()
	sw, sh := b.Dx(), b.Dy()
	if sw <= w || sw == 0 || sh == 0 {
		return src
	}
	h := max(1, sh*w/sw)
	dst := image.NewRGBA(image.Rect(0, 0, w, h))
	yc, isYC := src.(*image.YCbCr)
	for y := range h {
		y0, y1 := b.Min.Y+y*sh/h, b.Min.Y+max((y+1)*sh/h, y*sh/h+1)
		for x := range w {
			x0, x1 := b.Min.X+x*sw/w, b.Min.X+max((x+1)*sw/w, x*sw/w+1)
			var s0, s1, s2, s3, n uint64
			for py := y0; py < y1; py++ {
				for px := x0; px < x1; px++ {
					if isYC {
						// Average in YCbCr (linear in RGB), convert once per output pixel.
						s0 += uint64(yc.Y[yc.YOffset(px, py)])
						ci := yc.COffset(px, py)
						s1 += uint64(yc.Cb[ci])
						s2 += uint64(yc.Cr[ci])
					} else {
						r, g, bl, a := src.At(px, py).RGBA()
						s0, s1, s2, s3 = s0+uint64(r), s1+uint64(g), s2+uint64(bl), s3+uint64(a)
					}
					n++
				}
			}
			if isYC {
				r, g, bl := color.YCbCrToRGB(uint8(s0/n), uint8(s1/n), uint8(s2/n))
				dst.SetRGBA(x, y, color.RGBA{r, g, bl, 255})
			} else {
				dst.SetRGBA(x, y, color.RGBA{uint8(s0 / n >> 8), uint8(s1 / n >> 8), uint8(s2 / n >> 8), uint8(s3 / n >> 8)})
			}
		}
	}
	return dst
}

// mentions reports whether u is a string somewhere in a pushed source.
func (s *sourceSet) mentions(u string) bool {
	s.mu.RLock()
	defer s.mu.RUnlock()
	var walk func(v any) bool
	walk = func(v any) bool {
		switch t := v.(type) {
		case string:
			return t == u
		case []any:
			for _, x := range t {
				if walk(x) {
					return true
				}
			}
		case map[string]any:
			for _, x := range t {
				if walk(x) {
					return true
				}
			}
		}
		return false
	}
	for _, e := range s.data {
		if !bytes.Contains(e.Data, []byte(u)) && !bytes.Contains(e.Data, []byte(jsonEscaped(u))) {
			continue
		}
		var v any
		if json.Unmarshal(e.Data, &v) == nil && walk(v) {
			return true
		}
	}
	return false
}

// jsonEscaped is u as encoding/json writes it inside a string (it escapes &, < and >).
func jsonEscaped(u string) string {
	b, _ := json.Marshal(u)
	return string(b[1 : len(b)-1])
}

// handleImage serves GET /api/data/img?u=URL&w=WIDTH (public, like the page).
func (s *Server) handleImage(w http.ResponseWriter, r *http.Request) {
	u := r.URL.Query().Get("u")
	pu, err := url.Parse(u)
	if err != nil || (pu.Scheme != "https" && pu.Scheme != "http") || pu.Host == "" || !s.src.mentions(u) {
		http.Error(w, "no such picture", http.StatusNotFound)
		return
	}
	want, _ := strconv.Atoi(r.URL.Query().Get("w"))
	width := imgWidths[len(imgWidths)-1]
	for _, x := range imgWidths {
		if want <= x {
			width = x
			break
		}
	}
	b, err := s.img.get(r.Context(), u, width)
	if err != nil {
		http.Error(w, "picture unavailable", http.StatusBadGateway)
		return
	}
	w.Header().Set("Content-Type", "image/jpeg")
	w.Header().Set("X-Content-Type-Options", "nosniff")
	w.Header().Set("Cache-Control", "max-age=86400")
	_, _ = w.Write(b)
}
