package api

// Reports: a frame's `report` action posts one text file (tfctl report.rs: versions, config,
// mounts, processes, dmesg, this and the previous boot's log). The server keeps the last
// MaxReports per frame under <data>/reports/<id>/ and serves them to the API, /admin and MCP,
// so a frame can be debugged with no ssh and no cable.

import (
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"time"

	"github.com/MikeO7/thinframe/server/internal/store"
)

const (
	// MaxReport bounds one posted report (tfctl caps its own at 384 KiB).
	MaxReport = 512 << 10
	// MaxReports is how many a frame keeps; older ones are deleted.
	MaxReports = 20
)

var reportNameRe = regexp.MustCompile(`^[0-9]{8}T[0-9]{6}Z\.txt$`)

// Report describes one stored report.
type Report struct {
	Name string    `json:"name"`
	Size int64     `json:"size"`
	At   time.Time `json:"at"`
}

func (s *Server) reportDir(id string) string { return filepath.Join(s.opts.Reports, id) }

// SaveReport stores body as a new report for id and prunes old ones. The frame must exist.
func (s *Server) SaveReport(id string, body []byte) (Report, error) {
	if !store.ValidID(id) {
		return Report{}, store.ErrInvalidID
	}
	if _, err := s.Store.Get(id); err != nil {
		return Report{}, err
	}
	dir := s.reportDir(id)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return Report{}, err
	}
	now := time.Now().UTC()
	name := now.Format("20060102T150405Z") + ".txt"
	// Two reports in one second: keep the newer one by appending.
	f, err := os.OpenFile(filepath.Join(dir, name), os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o644)
	if err != nil {
		return Report{}, err
	}
	_, werr := f.Write(body)
	if cerr := f.Close(); werr == nil {
		werr = cerr
	}
	if werr != nil {
		return Report{}, werr
	}
	if list, err := s.ListReports(id); err == nil {
		for i := MaxReports; i < len(list); i++ {
			_ = os.Remove(filepath.Join(dir, list[i].Name))
		}
	}
	_, _ = s.Store.Update(id, func(f *store.Frame) {
		store.AddEvent(f, "report", fmt.Sprintf("%s (%d bytes)", name, len(body)))
	})
	return Report{Name: name, Size: int64(len(body)), At: now}, nil
}

// ListReports returns a frame's reports, newest first.
func (s *Server) ListReports(id string) ([]Report, error) {
	if !store.ValidID(id) {
		return nil, store.ErrInvalidID
	}
	entries, err := os.ReadDir(s.reportDir(id))
	if errors.Is(err, os.ErrNotExist) {
		return []Report{}, nil
	}
	if err != nil {
		return nil, err
	}
	out := []Report{}
	for _, e := range entries {
		if !reportNameRe.MatchString(e.Name()) {
			continue
		}
		info, err := e.Info()
		if err != nil {
			continue
		}
		at, _ := time.Parse("20060102T150405Z", strings.TrimSuffix(e.Name(), ".txt"))
		out = append(out, Report{Name: e.Name(), Size: info.Size(), At: at})
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Name > out[j].Name })
	return out, nil
}

// ReadReport returns one report's text; name "latest" means the newest.
func (s *Server) ReadReport(id, name string) ([]byte, error) {
	if !store.ValidID(id) {
		return nil, store.ErrInvalidID
	}
	if name == "latest" {
		list, err := s.ListReports(id)
		if err != nil {
			return nil, err
		}
		if len(list) == 0 {
			return nil, store.ErrNotFound
		}
		name = list[0].Name
	}
	if !reportNameRe.MatchString(name) {
		return nil, fmt.Errorf("%w: bad report name", ErrInvalid)
	}
	b, err := os.ReadFile(filepath.Join(s.reportDir(id), name))
	if errors.Is(err, os.ErrNotExist) {
		return nil, store.ErrNotFound
	}
	return b, err
}

func (s *Server) handlePostReport(w http.ResponseWriter, r *http.Request) {
	r.Body = http.MaxBytesReader(w, r.Body, MaxReport)
	body, err := io.ReadAll(r.Body)
	if err != nil {
		code := http.StatusBadRequest
		if _, big := errors.AsType[*http.MaxBytesError](err); big {
			code = http.StatusRequestEntityTooLarge
		}
		http.Error(w, err.Error(), code)
		return
	}
	rep, err := s.SaveReport(r.PathValue("id"), body)
	if err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, http.StatusCreated, rep)
}

func (s *Server) handleListReports(w http.ResponseWriter, r *http.Request) {
	list, err := s.ListReports(r.PathValue("id"))
	if err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, 200, list)
}

func (s *Server) handleGetReport(w http.ResponseWriter, r *http.Request) {
	b, err := s.ReadReport(r.PathValue("id"), r.PathValue("name"))
	if err != nil {
		writeErr(w, err)
		return
	}
	w.Header().Set("Content-Type", "text/plain; charset=utf-8")
	w.Header().Set("X-Content-Type-Options", "nosniff")
	_, _ = w.Write(b)
}
