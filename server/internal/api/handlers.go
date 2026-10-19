package api

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strconv"
	"strings"
	"time"

	"github.com/MikeO7/thinframe/server/internal/photos"
	"github.com/MikeO7/thinframe/server/internal/store"
	"github.com/MikeO7/thinframe/server/internal/widgets"
)

func (s *Server) handleIndex(w http.ResponseWriter, r *http.Request) {
	if r.URL.Path != "/" {
		http.NotFound(w, r)
		return
	}
	http.Redirect(w, r, "/admin", http.StatusFound)
}

func (s *Server) handleFramePage(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	f, err := s.Store.Ensure(id)
	if err != nil {
		writeErr(w, err)
		return
	}
	b, err := widgets.Render(f)
	if err != nil {
		writeErr(w, err)
		return
	}
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	w.Header().Set("Cache-Control", "no-store")
	_, _ = w.Write(b)
}

// frameView is what the list/get endpoints return.
type frameView struct {
	store.Frame
	Online bool           `json:"online"`
	Config map[string]any `json:"config"`
}

func (s *Server) view(f store.Frame) frameView {
	return frameView{Frame: f, Online: s.Online(f), Config: s.ConfigFor(f)}
}

func (s *Server) handleList(w http.ResponseWriter, _ *http.Request) {
	fs := s.Store.List()
	out := make([]frameView, len(fs))
	for i, f := range fs {
		out[i] = s.view(f)
	}
	writeJSON(w, 200, out)
}

func (s *Server) handleGetFrame(w http.ResponseWriter, r *http.Request) {
	f, err := s.Store.Get(r.PathValue("id"))
	if err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, 200, s.view(f))
}

func (s *Server) handleDeleteFrame(w http.ResponseWriter, r *http.Request) {
	if err := s.Store.Delete(r.PathValue("id")); err != nil {
		writeErr(w, err)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func (s *Server) handleGetConfig(w http.ResponseWriter, r *http.Request) {
	f, err := s.Store.Ensure(r.PathValue("id"))
	if err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, 200, s.ConfigFor(f))
}

func (s *Server) handlePutConfig(w http.ResponseWriter, r *http.Request) {
	var patch map[string]any
	if err := json.NewDecoder(io.LimitReader(r.Body, 1<<16)).Decode(&patch); err != nil {
		writeErr(w, fmt.Errorf("%w: %v", ErrInvalid, err))
		return
	}
	f, err := s.SetConfig(r.PathValue("id"), patch)
	if err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, 200, s.ConfigFor(f))
}

func (s *Server) handlePutLayout(w http.ResponseWriter, r *http.Request) {
	var l store.Layout
	if err := json.NewDecoder(io.LimitReader(r.Body, 1<<20)).Decode(&l); err != nil {
		writeErr(w, fmt.Errorf("%w: %v", ErrInvalid, err))
		return
	}
	f, err := s.SetLayout(r.PathValue("id"), l)
	if err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, 200, f.Layout)
}

func (s *Server) handleHeartbeat(w http.ResponseWriter, r *http.Request) {
	var hb store.Heartbeat
	if err := json.NewDecoder(io.LimitReader(r.Body, 1<<16)).Decode(&hb); err != nil {
		writeErr(w, fmt.Errorf("%w: %v", ErrInvalid, err))
		return
	}
	f, err := s.recordHeartbeat(r.PathValue("id"), hb)
	if err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, 200, map[string]any{"config": s.ConfigFor(f), "pending": f.Pending, "time_unix": time.Now().Unix()})
}

func (s *Server) recordHeartbeat(id string, hb store.Heartbeat) (store.Frame, error) {
	hb.At = time.Now().UTC()
	f, err := s.Store.Update(id, func(f *store.Frame) {
		f.Last = &hb
		f.LastSeen = hb.At
	})
	if err == nil {
		s.sse.publish(id, map[string]any{"widget": "status", "data": map[string]any{
			"line1": fmt.Sprintf("%s · %s · %.0f°C", f.ID, hb.IP, hb.TempC),
			"line2": fmt.Sprintf("%d/%d MB · up %s · v%s", hb.MemUsedMB, hb.MemTotalMB, (time.Duration(hb.UptimeS) * time.Second).Round(time.Minute), hb.TfctlVersion),
		}})
	}
	return f, err
}

func (s *Server) handlePostAction(w http.ResponseWriter, r *http.Request) {
	var in struct {
		Name string            `json:"name"`
		Args map[string]string `json:"args"`
	}
	if err := json.NewDecoder(io.LimitReader(r.Body, 1<<16)).Decode(&in); err != nil {
		writeErr(w, fmt.Errorf("%w: %v", ErrInvalid, err))
		return
	}
	a, err := s.Act(r.PathValue("id"), in.Name, in.Args)
	if err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, http.StatusAccepted, a)
}

// handleBeacon takes the page's touch latency measurement (no auth: sendBeacon cannot set headers).
func (s *Server) handleBeacon(w http.ResponseWriter, r *http.Request) {
	var in struct {
		LatencyMS int    `json:"latency_ms"`
		Widget    string `json:"widget"`
	}
	_ = json.NewDecoder(io.LimitReader(r.Body, 4096)).Decode(&in)
	if in.LatencyMS > 0 && in.LatencyMS < 10000 {
		_, _ = s.Store.Update(r.PathValue("id"), func(f *store.Frame) {
			if f.Last != nil {
				f.Last.LatencyMS = in.LatencyMS
			}
		})
	}
	w.WriteHeader(http.StatusNoContent)
}

func (s *Server) handleRelease(w http.ResponseWriter, _ *http.Request) {
	writeJSON(w, 200, s.ReleaseStatus())
}

// ReleaseStatus reports carried versions and lagging frames.
func (s *Server) ReleaseStatus() map[string]any {
	out := map[string]any{"server_version": s.opts.Version}
	if s.opts.Artifacts != nil {
		out["artifacts"] = s.opts.Artifacts.Versions()
	}
	lag := []map[string]string{}
	for _, f := range s.Store.List() {
		if f.Last == nil {
			continue
		}
		if s.opts.Artifacts != nil && f.Last.SystemVersion != "" && f.Last.SystemVersion != s.opts.Artifacts.Versions().System {
			lag = append(lag, map[string]string{"id": f.ID, "system": f.Last.SystemVersion})
		}
	}
	out["lagging"] = lag
	return out
}

func (s *Server) handlePhotoNext(w http.ResponseWriter, r *http.Request) {
	if s.opts.Photos == nil {
		http.Error(w, "no photo source configured (PHOTOS_DIR)", http.StatusNotFound)
		return
	}
	id := r.URL.Query().Get("frame")
	wd, _ := strconv.Atoi(r.URL.Query().Get("w"))
	ht, _ := strconv.Atoi(r.URL.Query().Get("h"))
	if wd <= 0 || ht <= 0 || wd > 4096 || ht > 4096 {
		wd, ht = 1280, 800
		if f, err := s.Store.Get(id); err == nil && f.Config.Rotation%2 == 0 {
			wd, ht = 800, 1280
		}
	}
	p, err := s.opts.Photos.Next(id, wd, ht)
	if err == photos.ErrNoPhotos {
		http.Error(w, "no photos", http.StatusNotFound)
		return
	}
	if err != nil {
		writeErr(w, err)
		return
	}
	w.Header().Set("Cache-Control", "no-store")
	http.ServeFile(w, r, p)
}

// ---- SSE data plane ------------------------------------------------------------

func (s *Server) handleSSE(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	fl, ok := w.(http.Flusher)
	if !ok {
		http.Error(w, "streaming unsupported", 500)
		return
	}
	w.Header().Set("Content-Type", "text/event-stream")
	w.Header().Set("Cache-Control", "no-store")
	w.Header().Set("X-Accel-Buffering", "no")
	ch, unsub := s.sse.subscribe(id)
	defer unsub()
	// Initial state so a fresh page has data without waiting.
	if f, err := s.Store.Get(id); err == nil && f.Last != nil {
		s.recordHeartbeatSSE(f)
	}
	if s.opts.Weather != nil {
		go s.pushWeather(id)
	}
	ping := time.NewTicker(25 * time.Second)
	defer ping.Stop()
	for {
		select {
		case <-r.Context().Done():
			return
		case <-ping.C:
			_, _ = io.WriteString(w, ": ping\n\n")
			fl.Flush()
		case b := <-ch:
			_, _ = fmt.Fprintf(w, "data: %s\n\n", b)
			fl.Flush()
		}
	}
}

func (s *Server) recordHeartbeatSSE(f store.Frame) {
	hb := f.Last
	s.sse.publish(f.ID, map[string]any{"widget": "status", "data": map[string]any{
		"line1": fmt.Sprintf("%s · %s · %.0f°C", f.ID, hb.IP, hb.TempC),
		"line2": fmt.Sprintf("%d/%d MB · v%s", hb.MemUsedMB, hb.MemTotalMB, hb.TfctlVersion),
	}})
}

func (s *Server) pushWeather(id string) {
	f, err := s.Store.Get(id)
	if err != nil {
		return
	}
	for _, wdg := range f.Layout.Widgets {
		if wdg.Type != "weather" {
			continue
		}
		wx, err := s.opts.Weather.Current()
		if err != nil {
			s.sse.publish(id, map[string]any{"widget": wdg.ID, "data": map[string]any{"desc": "weather unavailable"}})
			continue
		}
		s.sse.publish(id, map[string]any{"widget": wdg.ID, "data": map[string]any{
			"temp": strconv.Itoa(int(wx.TempC + 0.5)), "desc": strings.ToLower(wx.Desc),
		}})
	}
}
