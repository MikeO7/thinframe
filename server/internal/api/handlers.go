package api

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"math"
	"net/http"
	"os"
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
	// Public, so it must not write: an unknown id renders the default dashboard without
	// creating a record (the frame's own authenticated socket does that).
	id := r.PathValue("id")
	if !store.ValidID(id) {
		http.NotFound(w, r)
		return
	}
	f, err := s.Store.Get(id)
	if err != nil {
		f = store.Frame{ID: id, Config: store.DefaultConfig(), Layout: store.DefaultLayout()}
	}
	// Simulator: ?sim=WxH renders at the panel size; &fit=WxH scales it into the surface.
	simW, simH := parseSize(r.URL.Query().Get("sim"))
	fitW, fitH := parseSize(r.URL.Query().Get("fit"))
	// The page carries the frame's emit key: no other origin may read it (no CORS header) or
	// frame it (pageCSP).
	b, err := widgets.RenderSim(f, EmitKey(s.opts.Token, id), simW, simH, fitW, fitH)
	if err != nil {
		writeErr(w, err)
		return
	}
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	w.Header().Set("Content-Security-Policy", pageCSP)
	w.Header().Set("Cache-Control", "no-store")
	_, _ = w.Write(b)
}

// frameView is what the list/get endpoints return.
type frameView struct {
	store.Frame
	Online  bool           `json:"online"`
	Config  map[string]any `json:"config"`
	Reports []Report       `json:"reports,omitempty"`
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
	id := r.PathValue("id")
	if err := s.Store.Delete(id); err != nil {
		writeErr(w, err)
		return
	}
	// The socket was authenticated against the record just removed; drop it and
	// any open pairing request so the frame cannot carry on as if nothing happened.
	s.Hub.Kick(id, "deleted")
	s.mu.Lock()
	delete(s.pair.open, id)
	s.mu.Unlock()
	// Its screenshots and reports go with it (id is valid: Delete checked it).
	for _, dir := range []string{s.shotDir(id), s.reportDir(id)} {
		if err := os.RemoveAll(dir); err != nil {
			log.Printf("delete %s: %v", id, err)
		}
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
	if err := json.NewDecoder(io.LimitReader(r.Body, 1<<17)).Decode(&patch); err != nil { // 128 KiB: splash_png
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
			"line2": fmt.Sprintf("%d/%d MB · up %s · v%s", hb.MemUsedMB, hb.MemTotalMB, (time.Duration(hb.UptimeS) * time.Second).Round(time.Minute), hb.Version()),
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
// Only known frames are updated; the store keeps the last MaxTouchSamples.
func (s *Server) handleBeacon(w http.ResponseWriter, r *http.Request) {
	var in struct {
		LatencyMS   int             `json:"latency_ms"`
		PaintedAtMS int64           `json:"painted_at_ms"`
		Widget      string          `json:"widget"`
		Load        *store.PageLoad `json:"load"`
	}
	err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 4096)).Decode(&in)
	if _, big := errors.AsType[*http.MaxBytesError](err); big {
		http.Error(w, "beacon too large", http.StatusRequestEntityTooLarge)
		return
	}
	// Each beacon rewrites and fsyncs the frame file under the store lock: a tap a second is
	// plenty, a page (or anyone on the LAN) in a loop is not.
	if !s.events.allow("beacon:"+r.PathValue("id"), emitBurst, emitWindow) {
		http.Error(w, "too many beacons", http.StatusTooManyRequests)
		return
	}
	if l := in.Load; l != nil && l.LoadMS > 0 && l.LoadMS < 600000 && beaconMS(l.ResponseMS) && beaconMS(l.DOMMS) && beaconMS(l.PaintMS) {
		_, _ = s.Store.UpdateExisting(r.PathValue("id"), func(f *store.Frame) {
			f.PageLoad = &store.PageLoad{ResponseMS: l.ResponseMS, DOMMS: l.DOMMS, PaintMS: l.PaintMS,
				LoadMS: l.LoadMS, At: time.Now().UTC()}
		})
	}
	if in.LatencyMS > 0 && in.LatencyMS < 10000 {
		_, _ = s.Store.UpdateExisting(r.PathValue("id"), func(f *store.Frame) {
			if f.Touch == nil {
				f.Touch = &store.Touch{}
			}
			f.Touch.Add(in.LatencyMS, in.PaintedAtMS, time.Now().UTC())
		})
	}
	w.WriteHeader(http.StatusNoContent)
}

// beaconMS reports whether a page timing is plausible: 0 (not measured) up to 10 minutes.
func beaconMS(ms int) bool { return ms >= 0 && ms < 600000 }

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
		if s.opts.Artifacts == nil {
			continue
		}
		v := s.opts.Artifacts.Versions()
		sysLag := f.Last.SystemVersion != "" && v.System != "" && f.Last.SystemVersion != v.System
		swLag := f.Last.SoftwareVersion != "" && v.Software != "" && f.Last.SoftwareVersion != v.Software
		if sysLag || swLag {
			lag = append(lag, map[string]string{"id": f.ID, "system": f.Last.SystemVersion, "software": f.Last.SoftwareVersion})
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
	if !store.ValidID(id) {
		http.Error(w, "bad frame id", http.StatusBadRequest)
		return
	}
	fl, ok := w.(http.Flusher)
	if !ok {
		http.Error(w, "streaming unsupported", 500)
		return
	}
	w.Header().Set("Content-Type", "text/event-stream")
	w.Header().Set("Cache-Control", "no-store")
	w.Header().Set("X-Accel-Buffering", "no")
	// An app shown full screen runs in a sandbox (an opaque origin), and the saved boot page
	// is a file:// document, so their streams are cross-origin requests (hosts.go appCORS).
	appCORS(w, r)
	ch, unsub := s.sse.subscribeAs(id, r.URL.Query().Get("apps") == "1")
	defer unsub()
	// Send the headers now: the client sees the stream open without waiting for the first event.
	w.WriteHeader(http.StatusOK)
	fl.Flush()
	// Initial state so a fresh page has data without waiting.
	if f, err := s.Store.Get(id); err == nil && f.Last != nil {
		s.recordHeartbeatSSE(f)
	}
	go s.pushData(id)
	s.pushRing(id)
	// The adapters cache for 10 minutes; refresh on that cadence so a page that stays
	// open for days still shows current weather and calendar.
	refresh := time.NewTicker(s.dataEvery)
	defer refresh.Stop()
	ping := time.NewTicker(25 * time.Second)
	defer ping.Stop()
	for {
		select {
		case <-r.Context().Done():
			return
		case <-refresh.C:
			go s.pushData(id)
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
		"line2": fmt.Sprintf("%d/%d MB · v%s", hb.MemUsedMB, hb.MemTotalMB, hb.Version()),
	}})
}

// weatherData is the weather widget's fields, shared by SSE and /api/data/weather.
// A stale reading (fetch failed, cache still holds one) is shown rather than an error.
func (s *Server) weatherData() map[string]any {
	wx, err := s.opts.Weather.Current()
	s.feeds.note("weather", err)
	if err != nil && wx.Desc == "" {
		return map[string]any{"temp": "", "desc": "weather unavailable"}
	}
	deg := func(c float64) int {
		if s.opts.Fahrenheit {
			c = c*9/5 + 32
		}
		return int(math.Round(c))
	}
	out := map[string]any{"temp": strconv.Itoa(deg(wx.TempC)), "desc": strings.ToLower(wx.Desc), "code": wx.Code, "night": wx.Night}
	days := make([]map[string]any, 0, len(wx.Days))
	for _, d := range wx.Days {
		days = append(days, map[string]any{"date": d.Date, "code": d.Code, "hi": deg(d.MaxC), "lo": deg(d.MinC)})
	}
	if len(days) > 0 {
		out["hi"], out["lo"], out["days"] = days[0]["hi"], days[0]["lo"], days
	}
	return out
}

// calendarData is the calendar widget's fields, shared by SSE and /api/data/calendar.
func (s *Server) calendarData(now time.Time) map[string]any {
	evs, err := s.opts.Calendar.Upcoming(now, 3)
	s.feeds.note("calendar", err)
	data := map[string]any{"l1": "", "l2": "", "l3": ""}
	for i, e := range evs {
		data["l"+strconv.Itoa(i+1)] = e.Line(now)
	}
	switch {
	case err != nil && len(evs) == 0:
		data["l1"] = "calendar unavailable"
	case len(evs) == 0:
		data["l1"] = "nothing coming up"
	}
	return data
}

// pushData publishes weather and calendar fields to every matching widget of a frame.
func (s *Server) pushData(id string) {
	f, err := s.Store.Get(id)
	if err != nil {
		return
	}
	var wx, cal, ag map[string]any
	for _, wdg := range f.Layout.Widgets {
		switch {
		case (wdg.Type == "weather" || wdg.Type == "hero" || wdg.Type == "forecast") && s.opts.Weather != nil:
			if wx == nil {
				wx = s.weatherData()
			}
			s.sse.publish(id, map[string]any{"widget": wdg.ID, "data": wx})
		case wdg.Type == "calendar" && s.opts.Calendar != nil:
			if cal == nil {
				cal = s.calendarData(time.Now())
			}
			s.sse.publish(id, map[string]any{"widget": wdg.ID, "data": cal})
		case wdg.Type == "agenda" && sourceOf(wdg) == "" && s.opts.Calendar != nil:
			if ag == nil {
				ag = s.agendaData(time.Now())
			}
			s.sse.publish(id, map[string]any{"widget": wdg.ID, "data": ag})
		case wdg.Type == "cameras" && s.opts.Protect != nil:
			s.sse.publish(id, map[string]any{"widget": wdg.ID, "data": s.camerasData(wdg)})
		case wdg.Type == "cameras":
			s.sse.publish(id, map[string]any{"widget": wdg.ID, "data": map[string]any{"setup": true}})
		}
	}
	s.pushSources(f)
}

// agendaData is the agenda widget's events from the calendar feed: the next 20, as
// {start, end, title} with RFC 3339 times. A failed fetch with nothing cached sends nothing.
func (s *Server) agendaData(now time.Time) map[string]any {
	day := time.Date(now.Year(), now.Month(), now.Day(), 0, 0, 0, 0, now.Location()) // keep today's past events
	evs, err := s.opts.Calendar.Upcoming(day, 20)
	if err != nil && len(evs) == 0 {
		return map[string]any{}
	}
	out := make([]map[string]any, 0, len(evs))
	for _, e := range evs {
		end := e.End
		if end.IsZero() {
			end = e.Start.Add(time.Hour)
		}
		out = append(out, map[string]any{"start": e.Start.Format(time.RFC3339), "end": end.Format(time.RFC3339), "title": e.Summary, "all_day": e.AllDay})
	}
	return map[string]any{"events": out}
}

// handleDataWeather serves GET /api/data/weather. The page never calls a third party:
// the server fetches and caches, the page reads this or the SSE stream.
func (s *Server) handleDataWeather(w http.ResponseWriter, r *http.Request) {
	appCORS(w, r) // apps read it from their sandbox
	if s.opts.Weather == nil {
		http.Error(w, "no weather source configured (WEATHER_LAT, WEATHER_LON)", http.StatusNotFound)
		return
	}
	w.Header().Set("Cache-Control", "no-store")
	writeJSON(w, http.StatusOK, s.weatherData())
}

// handleDataCalendar serves GET /api/data/calendar.
func (s *Server) handleDataCalendar(w http.ResponseWriter, r *http.Request) {
	appCORS(w, r) // apps read it from their sandbox
	if s.opts.Calendar == nil {
		http.Error(w, "no calendar source configured (CALENDAR_ICS_URL)", http.StatusNotFound)
		return
	}
	w.Header().Set("Cache-Control", "no-store")
	writeJSON(w, http.StatusOK, s.calendarData(time.Now()))
}

func parseSize(s string) (int, int) {
	var w, h int
	if s == "" {
		return 0, 0
	}
	_, _ = fmt.Sscanf(s, "%dx%d", &w, &h)
	if w < 100 || h < 100 || w > 4096 || h > 4096 {
		return 0, 0
	}
	return w, h
}
