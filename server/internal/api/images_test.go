package api

import (
	"bytes"
	"encoding/binary"
	"encoding/json"
	"hash/crc32"
	"image"
	"image/color"
	"image/jpeg"
	"image/png"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
)

// A poster too big for the frame is fetched once, shrunk to the asked width and cached; a URL
// that no source names is refused.
func TestRemotePicturesAreShrunkAndOnlyFromSources(t *testing.T) {
	big := image.NewYCbCr(image.Rect(0, 0, 400, 600), image.YCbCrSubsampleRatio420)
	for i := range big.Y {
		big.Y[i] = 200
	}
	var poster bytes.Buffer
	if err := jpeg.Encode(&poster, big, nil); err != nil {
		t.Fatal(err)
	}
	var fetches atomic.Int32
	remote := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		fetches.Add(1)
		_, _ = w.Write(poster.Bytes())
	}))
	defer remote.Close()
	posterURL := remote.URL + "/p.jpg?a=1&b=2" // & is escaped as & when Go writes JSON

	s, ts := newDataServer(t, Options{})
	raw, _ := json.Marshal(map[string]any{"added": []any{map[string]any{"title": "X", "image": posterURL}}})
	if _, _, err := s.src.put("shows", raw); err != nil {
		t.Fatal(err)
	}
	get := func(u, w string) (*http.Response, []byte) {
		return do(t, "GET", ts.URL+"/api/data/img?w="+w+"&u="+url.QueryEscape(u), "", nil)
	}
	var wg sync.WaitGroup
	for range 4 {
		wg.Go(func() {
			resp, body := get(posterURL, "90")
			if resp.StatusCode != 200 || resp.Header.Get("Content-Type") != "image/jpeg" {
				t.Errorf("poster: %d %s", resp.StatusCode, resp.Header.Get("Content-Type"))
				return
			}
			img, err := jpeg.Decode(bytes.NewReader(body))
			if err != nil || img.Bounds().Dx() != 96 || img.Bounds().Dy() != 144 {
				t.Errorf("poster %v %v, want 96x144", img.Bounds(), err)
			}
		})
	}
	wg.Wait()
	if n := fetches.Load(); n != 1 {
		t.Fatalf("4 requests made %d fetches, want 1", n)
	}
	for _, u := range []string{remote.URL + "/other.jpg", "file:///etc/passwd", ""} {
		if resp, _ := get(u, "96"); resp.StatusCode != 404 {
			t.Errorf("%q: %d, want 404", u, resp.StatusCode)
		}
	}
}

// pngHeader is the start of a PNG claiming w x h pixels: enough for DecodeConfig, nothing to decode.
func pngHeader(w, h uint32) []byte {
	ihdr := make([]byte, 0, 17)
	ihdr = append(ihdr, "IHDR"...)
	ihdr = binary.BigEndian.AppendUint32(ihdr, w)
	ihdr = binary.BigEndian.AppendUint32(ihdr, h)
	ihdr = append(ihdr, 8, 0, 0, 0, 0) // 8-bit gray
	b := []byte("\x89PNG\r\n\x1a\n")
	b = binary.BigEndian.AppendUint32(b, 13)
	b = append(b, ihdr...)
	return binary.BigEndian.AppendUint32(b, crc32.ChecksumIEEE(ihdr))
}

// A small file claiming huge dimensions (a decompression bomb) is refused before decoding.
func TestImageDimensionsCheckedBeforeDecode(t *testing.T) {
	for _, wh := range [][2]uint32{{16000, 16000}, {9000, 10}, {10, 9000}, {7000, 7000}} {
		if _, err := decodeBounded(pngHeader(wh[0], wh[1])); err == nil || !strings.Contains(err.Error(), "too large") {
			t.Errorf("%dx%d: %v, want too large", wh[0], wh[1], err)
		}
	}
	var ok bytes.Buffer
	if err := png.Encode(&ok, image.NewGray(image.Rect(0, 0, 300, 200))); err != nil {
		t.Fatal(err)
	}
	if img, err := decodeBounded(ok.Bytes()); err != nil || img.Bounds().Dx() != 300 {
		t.Fatalf("a normal picture: %v", err)
	}
}

// A public picture host cannot redirect the proxy into the LAN; a LAN host a source names can
// still redirect within it, and public to public is fine.
func TestImageRedirectsStayOutOfTheLAN(t *testing.T) {
	hop := func(from, to string) error {
		first, _ := http.NewRequest("GET", from, nil)
		next, _ := http.NewRequest("GET", to, nil)
		return imgRedirect(next, []*http.Request{first})
	}
	for _, to := range []string{"http://127.0.0.1/x", "http://10.0.0.5/x", "http://192.168.1.2/x",
		"http://169.254.169.254/latest/meta-data", "http://[::1]/x", "http://[fd00::1]/x", "http://0.0.0.0/x", "http://100.100.1.1/x"} {
		if err := hop("https://93.184.216.34/p.png", to); err == nil {
			t.Errorf("public -> %s was followed", to)
		}
	}
	for _, c := range [][2]string{{"https://93.184.216.34/p.png", "https://8.8.8.8/p.png"},
		{"http://192.168.1.10:8989/poster", "http://192.168.1.10:8989/poster/2"},
		{"http://192.168.1.10/poster", "http://127.0.0.1/poster"}} {
		if err := hop(c[0], c[1]); err != nil {
			t.Errorf("%s -> %s: %v", c[0], c[1], err)
		}
	}
	first, _ := http.NewRequest("GET", "https://8.8.8.8/", nil)
	if err := imgRedirect(first, make([]*http.Request, 5)); err == nil {
		t.Error("a sixth redirect was followed")
	}
}

func TestShrinkAveragesBlocks(t *testing.T) {
	src := image.NewRGBA(image.Rect(0, 0, 4, 2))
	for x := range 4 {
		for y := range 2 {
			c := uint8(0)
			if x%2 == 1 {
				c = 200
			}
			src.SetRGBA(x, y, color.RGBA{c, c, c, 255})
		}
	}
	out := shrink(src, 2)
	if out.Bounds().Dx() != 2 || out.Bounds().Dy() != 1 {
		t.Fatalf("size %v", out.Bounds())
	}
	if r, _, _, _ := out.At(0, 0).RGBA(); r>>8 != 100 {
		t.Fatalf("average %d, want 100", r>>8)
	}
	if shrink(src, 8) != image.Image(src) {
		t.Fatal("a narrow picture was scaled up")
	}
}
