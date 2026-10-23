// Package artifacts serves the frame software the container carries:
// tfctl-armv7, system-<ver>.tar, each with a .sha256 sidecar,
// and versions.json describing them. Frames learn versions from their config.
package artifacts

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
)

// Versions is the content of versions.json in the artifacts dir.
type Versions struct {
	Software string            `json:"software"` // tfctl version
	System   string            `json:"system"`   // OS bundle version
	Files    map[string]string `json:"files"`    // logical name -> file name, e.g. "tfctl": "tfctl-armv7"
}

// Dir is an artifacts directory.
type Dir struct {
	Path string
	mu   sync.Mutex
	sums map[string]string
}

// New returns a Dir (missing directory is fine: no artifacts).
func New(path string) *Dir { return &Dir{Path: path, sums: map[string]string{}} }

// Versions reads versions.json; zero value when absent.
func (d *Dir) Versions() Versions {
	var v Versions
	b, err := os.ReadFile(filepath.Join(d.Path, "versions.json"))
	if err == nil {
		_ = json.Unmarshal(b, &v)
	}
	return v
}

// Sum returns the sha256 hex of a file, cached by (name, size, mtime).
func (d *Dir) Sum(name string) (string, error) {
	p := filepath.Join(d.Path, filepath.Base(name))
	st, err := os.Stat(p)
	if err != nil {
		return "", err
	}
	key := name + "|" + st.ModTime().String() + "|" + strconv.FormatInt(st.Size(), 10)
	d.mu.Lock()
	if s, ok := d.sums[key]; ok {
		d.mu.Unlock()
		return s, nil
	}
	d.mu.Unlock()
	f, err := os.Open(p)
	if err != nil {
		return "", err
	}
	defer f.Close()
	h := sha256.New()
	if _, err := io.Copy(h, f); err != nil {
		return "", err
	}
	s := hex.EncodeToString(h.Sum(nil))
	d.mu.Lock()
	d.sums[key] = s
	d.mu.Unlock()
	return s, nil
}

// Handler serves GET /api/artifacts/{name} and {name}.sha256 with range support.
func (d *Dir) Handler() http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		name := filepath.Base(r.PathValue("name"))
		if name == "" || name == "." || strings.HasPrefix(name, ".") {
			http.NotFound(w, r)
			return
		}
		if base, ok := strings.CutSuffix(name, ".sha256"); ok {
			sum, err := d.Sum(base)
			if err != nil {
				http.NotFound(w, r)
				return
			}
			w.Header().Set("Content-Type", "text/plain")
			_, _ = io.WriteString(w, sum+"  "+base+"\n")
			return
		}
		p := filepath.Join(d.Path, name)
		if _, err := os.Stat(p); err != nil {
			http.NotFound(w, r)
			return
		}
		w.Header().Set("Content-Type", "application/octet-stream")
		http.ServeFile(w, r, p)
	})
}
