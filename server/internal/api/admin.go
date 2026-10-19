package api

import (
	"encoding/json"
	"fmt"
	"html/template"
	"net/http"
	"strings"
	"time"

	"github.com/MikeO7/thinframe/server/internal/store"
	"github.com/MikeO7/thinframe/server/internal/widgets"
)

var adminTmpl = template.Must(template.New("admin").Funcs(template.FuncMap{
	"ago": func(t time.Time) string {
		if t.IsZero() {
			return "never"
		}
		return time.Since(t).Round(time.Second).String() + " ago"
	},
	"pretty": func(v any) string { b, _ := json.MarshalIndent(v, "", "  "); return string(b) },
	"join":   strings.Join,
	"list":   func(xs ...int) []int { return xs },
}).Parse(`<!doctype html><html lang="en"><head><meta charset="utf-8"><title>thinframe admin</title>
<meta name="viewport" content="width=device-width,initial-scale=1">
<style>
body{font:15px/1.4 system-ui,sans-serif;margin:2rem;background:#0b0d10;color:#e8e6e3}
a{color:#f2b84b}table{border-collapse:collapse;width:100%}td,th{padding:.4rem .6rem;border-bottom:1px solid #222;text-align:left;vertical-align:top}
.on{color:#6fd36f}.off{color:#d36f6f}details{margin:.5rem 0}textarea{width:100%;min-height:14rem;font:13px monospace;background:#14181d;color:#e8e6e3;border:1px solid #333}
input,select,button{font:inherit;background:#14181d;color:#e8e6e3;border:1px solid #333;padding:.3rem .5rem}button{cursor:pointer}
.row{display:flex;gap:.5rem;flex-wrap:wrap;align-items:center}.err{color:#d36f6f}.hint{color:#7a8088;font-size:.9em}
</style></head><body>
<h1>thinframe <span class="hint">{{.Version}} · {{len .Frames}} frames · {{.Online}} online</span></h1>
{{if .Error}}<p class="err">{{.Error}}</p>{{end}}
{{if not .Frames}}<p>No frames yet. A frame registers itself on its first connection.</p>{{end}}
<table><tr><th>frame</th><th>state</th><th>ip</th><th>mem</th><th>temp</th><th>latency</th><th>versions</th><th>last seen</th></tr>
{{range .Frames}}<tr><td><a href="#f-{{.ID}}">{{.ID}}</a>{{if .Name}}<br><span class="hint">{{.Name}}</span>{{end}}</td>
<td class="{{if .Online}}on{{else}}off{{end}}">{{if .Online}}online{{else}}offline{{end}}</td>
{{with .Last}}<td>{{.IP}}</td><td>{{.MemUsedMB}}/{{.MemTotalMB}} MB</td><td>{{printf "%.0f" .TempC}}°C</td><td>{{.LatencyMS}} ms</td><td>tfctl {{.TfctlVersion}}<br>sys {{.SystemVersion}} {{.Slot}}</td>{{else}}<td colspan="5" class="hint">no heartbeat yet</td>{{end}}
<td>{{ago .LastSeen}}</td></tr>{{end}}</table>

{{range .Frames}}<h2 id="f-{{.ID}}">{{.ID}} <a class="hint" href="/frame/{{.ID}}" target="_blank">open page ↗</a></h2>
<form method="post" action="/admin/{{.ID}}/config" class="row">
 <label>name <input name="name" value="{{.Name}}"></label>
 <label>rotation <select name="rotation">{{$rot := .Config.Rotation}}{{range $r := list 0 1 2 3}}<option {{if eq $r $rot}}selected{{end}}>{{$r}}</option>{{end}}</select></label>
 <label>brightness <input name="brightness" type="number" min="0" max="100" value="{{.Config.Brightness}}" style="width:5rem"></label>
 <label>off <input name="screen_off" value="{{.Config.ScreenOff}}" placeholder="23:00" style="width:5rem"></label>
 <label>on <input name="screen_on" value="{{.Config.ScreenOn}}" placeholder="06:30" style="width:5rem"></label>
 <label>url <input name="url" value="{{.Config.URL}}" placeholder="(default: /frame/{{.ID}})" style="width:18rem"></label>
 <label>tags <input name="tags" value="{{join .Config.Tags ","}}"></label>
 <button>save</button>
</form>
<form method="post" action="/admin/{{.ID}}/action" class="row">
 <select name="name">{{range $.Actions}}<option>{{.}}</option>{{end}}</select>
 <input name="arg" placeholder="arg (screen: on|off)" style="width:10rem"> <button>send action</button>
 <span class="hint">{{len .Pending}} pending</span>
</form>
<details><summary>layout ({{len .Layout.Widgets}} widgets) · types: {{join $.Types ", "}}</summary>
<form method="post" action="/admin/{{.ID}}/layout"><textarea name="layout">{{pretty .Layout}}</textarea><br><button>save layout</button></form></details>
<details><summary>events</summary><pre class="hint">{{range .Events}}{{.At.Format "15:04:05"}} {{.Kind}} {{.Text}}
{{end}}</pre></details>
{{end}}
</body></html>`))

type adminData struct {
	Version string
	Frames  []frameView
	Online  int
	Error   string
	Actions []string
	Types   []string
}

func (s *Server) handleAdmin(w http.ResponseWriter, r *http.Request) {
	s.renderAdmin(w, r.URL.Query().Get("err"))
}

func (s *Server) renderAdmin(w http.ResponseWriter, errText string) {
	fs := s.Store.List()
	d := adminData{Version: s.opts.Version, Error: errText, Actions: ValidActions, Types: widgets.Types}
	for _, f := range fs {
		v := s.view(f)
		d.Frames = append(d.Frames, v)
		if v.Online {
			d.Online++
		}
	}
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	if err := adminTmpl.Execute(w, d); err != nil {
		http.Error(w, err.Error(), 500)
	}
}

func (s *Server) adminRedirect(w http.ResponseWriter, r *http.Request, err error) {
	if err != nil {
		http.Redirect(w, r, "/admin?err="+template.URLQueryEscaper(err.Error()), http.StatusSeeOther)
		return
	}
	http.Redirect(w, r, "/admin", http.StatusSeeOther)
}

func (s *Server) handleAdminConfig(w http.ResponseWriter, r *http.Request) {
	if err := r.ParseForm(); err != nil {
		s.adminRedirect(w, r, err)
		return
	}
	patch := map[string]any{}
	for _, k := range []string{"name", "rotation", "brightness", "screen_off", "screen_on", "url", "tags"} {
		patch[k] = r.Form.Get(k)
	}
	_, err := s.SetConfig(r.PathValue("id"), patch)
	s.adminRedirect(w, r, err)
}

func (s *Server) handleAdminLayout(w http.ResponseWriter, r *http.Request) {
	var l store.Layout
	if err := json.Unmarshal([]byte(r.FormValue("layout")), &l); err != nil {
		s.adminRedirect(w, r, fmt.Errorf("layout json: %w", err))
		return
	}
	_, err := s.SetLayout(r.PathValue("id"), l)
	s.adminRedirect(w, r, err)
}

func (s *Server) handleAdminAction(w http.ResponseWriter, r *http.Request) {
	var args map[string]string
	if a := r.FormValue("arg"); a != "" {
		args = map[string]string{"arg": a}
	}
	_, err := s.Act(r.PathValue("id"), r.FormValue("name"), args)
	s.adminRedirect(w, r, err)
}
