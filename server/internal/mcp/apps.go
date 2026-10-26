package mcp

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"sort"
	"strings"
	"unicode/utf8"

	sdk "github.com/modelcontextprotocol/go-sdk/mcp"

	"github.com/MikeO7/thinframe/server/internal/api"
	"github.com/MikeO7/thinframe/server/internal/apps"
)

// AppGuide is what app_guide returns: everything needed to write an app.
const AppGuide = `# thinframe apps: the frame's screen is yours

An app is a small website: index.html plus any css, js, images and fonts. put_app stores it
(each put is a new version, the last 10 are kept, rollback_app goes back). Show it:
- full screen: show_app {id, app}. show_app {id, app: ""} goes back to the dashboard.
- as a widget: in set_layout, {"type":"app","props":{"app":"<name>","props":{...}}} fills any
  grid cell, next to the built-in widgets. Several app widgets can share one page.
Every page showing an app reloads by itself after put_app or rollback_app.
See the result: screenshot {id} returns what the panel shows right now. Errors in the app
(exceptions, rejected promises) show up in get_app_events as "app-error": check after a put.

## The screen and the browser
- 1280x800 CSS px, landscape, touch. Pointer events (pointerdown/up/move, click); no hover, no
  keyboard unless you draw one. Make tap targets at least 64 px.
- WPE WebKit (cog) on a 4-core ARM Cortex-A7 with 1 GB RAM and a Mali-400 GPU: modern JS and
  CSS (grid, flex, custom properties, ES modules, canvas, SVG, fetch, WebSocket to CORS hosts).
  Keep it light: animate transform and opacity; avoid large blur/backdrop-filter/box-shadow
  animations, per-frame full-screen canvas redraws, and multi-megabyte images.
- It runs for weeks: no memory growth (clear intervals, cap arrays), and reconnect anything
  you open yourself.
- No alert/confirm/prompt, no popups, no navigation away. Bundle fonts as woff2 for exact type.
- The screen may be rotated by the frame's config; the app always sees its page as upright.

## The sandbox
Every app page runs in a sandbox with an opaque origin. So:
- No cookies, localStorage or IndexedDB (they throw). Use thinframe.state.
- Relative URLs (style.css, img/logo.png, ./mod.mjs) load from the app.
- fetch() to other sites works when they allow CORS (many public APIs do). For an API that
  needs a secret or has no CORS, use a hook (a button runs a server-side request; the token
  stays on the server) or a poll (the server fetches every N s into a source the app watches).
- Built-in server data readable with fetch: /api/data/weather, /api/data/calendar,
  /api/data/source/<key>. Images in DATA_DIR/assets: <img src="/api/data/asset/<file>">.

## window.thinframe (injected before your scripts)
- ready: Promise, resolves when frame/widget/props are known (at once when full screen).
- frame, app, widget, props, embedded: where it runs; props come from the widget's props.props.
- data(key): Promise<object|null>, one source (set_source, set_poll, PUT /api/sources/<key>).
- on(key, cb): cb(data) now and on every change of that source; returns an unsubscribe fn.
- emit(name, data): Promise<{ok, status, body, error}>. Logged (get_app_events); if a hook named
  name exists (set_hook) the server runs it and body is its answer (with return_body). Buttons!
- state.get(key) / state.set(key, value) / state.all() / state.watch(key, cb): JSON kept on the
  server per app (64 keys, 64 KB), shared by every frame showing the app and pushed live.
  get_app_state / set_app_state read and write it from MCP.
- listen(name, cb): events MCP sends with send_to_app.

## Minimal example (files for put_app)
index.html:
<!doctype html><html><head><meta charset="utf-8"><link rel="stylesheet" href="app.css"></head>
<body><h1 id="t">--</h1><button id="b">Lamp</button><script src="app.js"></script></body></html>
app.css:
html,body{margin:0;height:100%;background:#0b0d10;color:#eef;font:600 28px system-ui,sans-serif}
body{display:grid;place-items:center;align-content:center;gap:40px}
button{font:inherit;padding:24px 48px;border-radius:24px;border:0;background:#2b6cff;color:#fff}
app.js:
thinframe.on('house', d => { document.getElementById('t').textContent = d.temp + '°'; });
document.getElementById('b').onclick = async () => {
  const r = await thinframe.emit('lamp', { entity: 'light.desk' });
  document.getElementById('b').textContent = r.ok ? 'Done' : 'Failed';
};
With: set_source {key:"house", data:{temp:21}} and set_hook {name:"lamp", url:"http://ha:8123/api/services/light/toggle",
headers:{Authorization:"Bearer ..."}, body:"{\"entity_id\":{{data}}}"} (or body "{\"entity_id\":\"light.desk\"}").

## Loop
put_app -> show_app (or set_layout) -> screenshot -> get_app_events -> fix -> put_app (merge:
true sends only the changed files).`

func registerApps(m *sdk.Server, s *api.Server) {
	openWorld := &sdk.ToolAnnotations{DestructiveHint: ptr(false), IdempotentHint: true, OpenWorldHint: ptr(true)}

	sdk.AddTool(m, tool("app_guide", "App guide", "How to write an app: a sandboxed web page (HTML/CSS/JS) shown full screen or as a widget, the window.thinframe SDK (live data, buttons, state), screen size and limits. Read it before put_app.", readOnly),
		func(_ context.Context, _ *sdk.CallToolRequest, _ empty) (*sdk.CallToolResult, map[string]any, error) {
			return &sdk.CallToolResult{Content: []sdk.Content{&sdk.TextContent{Text: AppGuide}}}, map[string]any{"bytes": len(AppGuide)}, nil
		})

	type putIn struct {
		Name        string            `json:"name" jsonschema:"app name: lowercase letters, digits, - and _ (up to 32)"`
		Files       map[string]string `json:"files,omitempty" jsonschema:"path -> text content (index.html, app.js, css/x.css, img/a.svg ...)"`
		BinaryFiles map[string]string `json:"binary_files,omitempty" jsonschema:"path -> base64 content (png, jpg, webp, gif, woff2)"`
		Merge       bool              `json:"merge,omitempty" jsonschema:"true: start from the current version's files, then apply files/binary_files/remove (send only what changed)"`
		Remove      []string          `json:"remove,omitempty" jsonschema:"with merge: paths to drop"`
		Description string            `json:"description,omitempty" jsonschema:"what the app is (kept until replaced)"`
		Note        string            `json:"note,omitempty" jsonschema:"what changed in this version"`
	}
	sdk.AddTool(m, tool("put_app", "Put app", "Store a new version of an app (a web page: index.html plus css/js/images, up to 64 files and 4 MB) and reload every frame showing it. Read app_guide first. Returns the version and the frames showing it.", safeWrite),
		func(_ context.Context, _ *sdk.CallToolRequest, in putIn) (*sdk.CallToolResult, map[string]any, error) {
			files := map[string][]byte{}
			if in.Merge {
				if cur, _, err := s.Apps().Files(in.Name, 0); err == nil {
					files = cur
				}
				for _, p := range in.Remove {
					delete(files, p)
				}
			} else if len(in.Remove) > 0 {
				return nil, nil, fmt.Errorf("remove needs merge: true")
			}
			for p, t := range in.Files {
				files[p] = []byte(t)
			}
			for p, b64 := range in.BinaryFiles {
				b, err := base64.StdEncoding.DecodeString(b64)
				if err != nil {
					return nil, nil, fmt.Errorf("binary_files[%s]: not base64: %v", p, err)
				}
				files[p] = b
			}
			meta, err := s.PutApp(in.Name, in.Description, in.Note, files)
			if err != nil {
				return nil, nil, err
			}
			return nil, map[string]any{"app": meta, "url": "/apps/" + in.Name + "/", "shown_on": nonNil(s.AppUsers(in.Name))}, nil
		})

	type getIn struct {
		Name    string `json:"name"`
		Version int    `json:"version,omitempty" jsonschema:"a kept version; default the current one"`
		Path    string `json:"path,omitempty" jsonschema:"only this file"`
	}
	sdk.AddTool(m, tool("get_app", "Get app", "An app's versions and the text of its files (binary files are listed with their size).", readOnly),
		func(_ context.Context, _ *sdk.CallToolRequest, in getIn) (*sdk.CallToolResult, map[string]any, error) {
			meta, err := s.Apps().Get(in.Name)
			if err != nil {
				return nil, nil, err
			}
			files, v, err := s.Apps().Files(in.Name, in.Version)
			if err != nil {
				return nil, nil, err
			}
			text, binary := map[string]string{}, map[string]int{}
			for p, b := range files {
				if in.Path != "" && p != in.Path {
					continue
				}
				if isText(p) && utf8.Valid(b) {
					text[p] = string(b)
				} else {
					binary[p] = len(b)
				}
			}
			if in.Path != "" && len(text)+len(binary) == 0 {
				return nil, nil, fmt.Errorf("app %s v%d has no file %q", in.Name, v, in.Path)
			}
			return nil, map[string]any{"app": meta, "version": v, "files": text, "binary_files": binary, "shown_on": nonNil(s.AppUsers(in.Name))}, nil
		})

	sdk.AddTool(m, tool("list_apps", "List apps", "Stored apps: name, description, current version, kept versions, and the frames showing each.", readOnly),
		func(_ context.Context, _ *sdk.CallToolRequest, _ empty) (*sdk.CallToolResult, map[string]any, error) {
			out := []map[string]any{}
			for _, a := range s.Apps().List() {
				out = append(out, map[string]any{"name": a.Name, "description": a.Description, "current": a.Current, "versions": a.Versions, "shown_on": nonNil(s.AppUsers(a.Name))})
			}
			return nil, map[string]any{"apps": out}, nil
		})

	type nameIn struct {
		Name string `json:"name" jsonschema:"app name"`
	}
	sdk.AddTool(m, tool("delete_app", "Delete app", "Delete an app and all its versions. Refused while a frame shows it (show_app another page or change the layout first).", destructive),
		func(_ context.Context, _ *sdk.CallToolRequest, in nameIn) (*sdk.CallToolResult, map[string]any, error) {
			if err := s.DeleteApp(in.Name); err != nil {
				return nil, nil, err
			}
			return nil, map[string]any{"deleted": in.Name}, nil
		})

	type rbIn struct {
		Name    string `json:"name"`
		Version int    `json:"version,omitempty" jsonschema:"kept version to make current; default the one before the current"`
	}
	sdk.AddTool(m, tool("rollback_app", "Roll back app", "Make an older kept version of an app current; frames showing it reload.", replaces),
		func(_ context.Context, _ *sdk.CallToolRequest, in rbIn) (*sdk.CallToolResult, map[string]any, error) {
			meta, err := s.RollbackApp(in.Name, in.Version)
			if err != nil {
				return nil, nil, err
			}
			return nil, map[string]any{"app": meta}, nil
		})

	type showIn struct {
		ID  string `json:"id" jsonschema:"frame id"`
		App string `json:"app" jsonschema:"app to show full screen; empty string = back to the dashboard"`
	}
	sdk.AddTool(m, tool("show_app", "Show app", "Show an app full screen on a frame (or the dashboard again with app \"\"). The frame switches pages in about a second.", safeWrite),
		func(_ context.Context, _ *sdk.CallToolRequest, in showIn) (*sdk.CallToolResult, map[string]any, error) {
			f, err := s.ShowApp(in.ID, in.App)
			if err != nil {
				return nil, nil, err
			}
			return nil, map[string]any{"id": f.ID, "url": s.ConfigFor(f)["url"], "online": s.Online(f)}, nil
		})

	type shotIn struct {
		ID    string `json:"id" jsonschema:"frame id"`
		Width int    `json:"width,omitempty" jsonschema:"scale to this width in px (default 800; 0 or 1280 = full size)"`
	}
	sdk.AddTool(m, tool("screenshot", "Screenshot", "What the frame's panel shows right now, as a PNG (taken on the frame from the scanned-out framebuffer, in the page's orientation). Takes 1 to 5 seconds. A dark screen (screen off) is still captured as it was last drawn.", readOnly),
		func(ctx context.Context, _ *sdk.CallToolRequest, in shotIn) (*sdk.CallToolResult, map[string]any, error) {
			b, err := s.Screenshot(ctx, in.ID)
			if err != nil {
				return nil, nil, err
			}
			w := in.Width
			if w == 0 {
				w = 800
			}
			if b, err = api.ScalePNG(b, w); err != nil {
				return nil, nil, err
			}
			out := map[string]any{"bytes": len(b)}
			content := []sdk.Content{&sdk.ImageContent{Data: b, MIMEType: "image/png"}}
			if f, err := s.Store.Get(in.ID); err == nil && f.Last != nil && f.Last.ScreenState != "" && f.Last.ScreenState != "on" {
				// The frame pauses the browser while the screen is off: the image is the last frame drawn.
				out["screen"] = f.Last.ScreenState
				content = append(content, &sdk.TextContent{Text: "screen is " + f.Last.ScreenState + ": the page is paused, this is the last frame drawn before that"})
			}
			return &sdk.CallToolResult{Content: content}, out, nil
		})

	type evIn struct {
		ID  string `json:"id,omitempty" jsonschema:"only this frame"`
		App string `json:"app,omitempty" jsonschema:"only this app"`
		N   int    `json:"n,omitempty" jsonschema:"newest N (default 50)"`
	}
	sdk.AddTool(m, tool("get_app_events", "Get app events", "What apps emitted (button presses, app-error reports) with the hook's status, newest last. Kept in memory, last 200.", readOnly),
		func(_ context.Context, _ *sdk.CallToolRequest, in evIn) (*sdk.CallToolResult, map[string]any, error) {
			n := in.N
			if n <= 0 {
				n = 50
			}
			return nil, map[string]any{"events": s.AppEvents(in.ID, in.App, n)}, nil
		})

	sdk.AddTool(m, tool("get_app_state", "Get app state", "The JSON state an app keeps on the server (thinframe.state).", readOnly),
		func(_ context.Context, _ *sdk.CallToolRequest, in nameIn) (*sdk.CallToolResult, map[string]any, error) {
			st, err := s.Apps().State(in.Name)
			if err != nil {
				return nil, nil, err
			}
			return nil, map[string]any{"state": st}, nil
		})

	type setStateIn struct {
		Name  string `json:"name" jsonschema:"app name"`
		Key   string `json:"key" jsonschema:"state key (lowercase id)"`
		Value any    `json:"value" jsonschema:"any JSON value; null deletes the key"`
	}
	sdk.AddTool(m, tool("set_app_state", "Set app state", "Set one key of an app's state; every page showing the app gets it live (thinframe.state.watch).", safeWrite),
		func(_ context.Context, _ *sdk.CallToolRequest, in setStateIn) (*sdk.CallToolResult, map[string]any, error) {
			b, err := json.Marshal(in.Value)
			if err != nil {
				return nil, nil, err
			}
			if err := s.SetAppState(in.Name, in.Key, b); err != nil {
				return nil, nil, err
			}
			return nil, map[string]any{"app": in.Name, "key": in.Key}, nil
		})

	type sendIn struct {
		App  string `json:"app" jsonschema:"app name"`
		Name string `json:"name" jsonschema:"event name the app listens for (thinframe.listen)"`
		Data any    `json:"data,omitempty"`
		ID   string `json:"id,omitempty" jsonschema:"only this frame; default every frame showing the app"`
	}
	sdk.AddTool(m, tool("send_to_app", "Send to app", "Push a one-off event to an app's open pages (thinframe.listen): show a message, start an animation, change a view. Not stored.", safeWrite),
		func(_ context.Context, _ *sdk.CallToolRequest, in sendIn) (*sdk.CallToolResult, map[string]any, error) {
			if !apps.ValidName(in.App) || in.Name == "" || len(in.Name) > 64 {
				return nil, nil, fmt.Errorf("app must be an app name and name 1..64 characters")
			}
			b, err := json.Marshal(in.Data)
			if err != nil || len(b) > api.MaxSource {
				return nil, nil, fmt.Errorf("data must be JSON of at most %d bytes", api.MaxSource)
			}
			s.SendToApp(in.ID, in.App, in.Name, b)
			return nil, map[string]any{"sent": true}, nil
		})

	type hookIn struct {
		Name       string            `json:"name" jsonschema:"the emit name that runs it (thinframe.emit(name, data))"`
		URL        string            `json:"url" jsonschema:"http(s) URL; {{frame}} {{app}} {{name}} are replaced"`
		Method     string            `json:"method,omitempty" jsonschema:"GET POST PUT PATCH DELETE (default POST)"`
		Headers    map[string]string `json:"headers,omitempty" jsonschema:"request headers, e.g. Authorization; stored on the server, never sent to pages, listed redacted"`
		Body       string            `json:"body,omitempty" jsonschema:"body template; {{data}} = the event data as JSON, {{frame}} {{app}} {{name}} = ids. Empty = {frame,app,name,data} as JSON"`
		TimeoutS   int               `json:"timeout_s,omitempty" jsonschema:"1..30, default 10"`
		ReturnBody bool              `json:"return_body,omitempty" jsonschema:"give the answer (up to 256 KB) back to the app as emit's body"`
	}
	sdk.AddTool(m, tool("set_hook", "Set hook", "Make an app event run an HTTP request from the server (Home Assistant, webhooks, any API). Secrets stay on the server; the app only knows the name.", openWorld),
		func(_ context.Context, _ *sdk.CallToolRequest, in hookIn) (*sdk.CallToolResult, map[string]any, error) {
			if err := s.SetHook(in.Name, api.Hook{Method: in.Method, URL: in.URL, Headers: in.Headers, Body: in.Body, TimeoutS: in.TimeoutS, ReturnBody: in.ReturnBody}); err != nil {
				return nil, nil, err
			}
			return nil, map[string]any{"hook": in.Name}, nil
		})
	sdk.AddTool(m, tool("list_hooks", "List hooks", "Hooks by name (header values redacted).", readOnly),
		func(_ context.Context, _ *sdk.CallToolRequest, _ empty) (*sdk.CallToolResult, map[string]any, error) {
			return nil, map[string]any{"hooks": s.Hooks()}, nil
		})
	sdk.AddTool(m, tool("delete_hook", "Delete hook", "Forget a hook; its event is then only logged.", destructive),
		func(_ context.Context, _ *sdk.CallToolRequest, in nameIn) (*sdk.CallToolResult, map[string]any, error) {
			return nil, map[string]any{"deleted": s.DeleteHook(in.Name)}, nil
		})

	type pollIn struct {
		Key     string            `json:"key" jsonschema:"source key it fills (apps: thinframe.on(key); widgets: props.source)"`
		URL     string            `json:"url" jsonschema:"http(s) URL fetched with GET"`
		EveryS  int               `json:"every_s" jsonschema:"seconds between fetches, 15..86400"`
		Headers map[string]string `json:"headers,omitempty" jsonschema:"request headers (tokens stay on the server)"`
		Pick    string            `json:"pick,omitempty" jsonschema:"dotted path into the JSON answer, e.g. data.items or list.0"`
	}
	sdk.AddTool(m, tool("set_poll", "Set poll", "Fetch a URL on a schedule into a source: live data for apps and widgets from any API. A JSON object is stored as is, other JSON as {value}, text as {text}.", openWorld),
		func(_ context.Context, _ *sdk.CallToolRequest, in pollIn) (*sdk.CallToolResult, map[string]any, error) {
			if err := s.SetPoll(in.Key, api.Poll{URL: in.URL, EveryS: in.EveryS, Headers: in.Headers, Pick: in.Pick}); err != nil {
				return nil, nil, err
			}
			return nil, map[string]any{"poll": in.Key, "first_fetch": "within 5 s; list_polls shows the outcome"}, nil
		})
	type keyIn struct {
		Key string `json:"key"`
	}
	sdk.AddTool(m, tool("list_polls", "List polls", "Polls with their last fetch, last success and last error (header values redacted).", readOnly),
		func(_ context.Context, _ *sdk.CallToolRequest, _ empty) (*sdk.CallToolResult, map[string]any, error) {
			return nil, map[string]any{"polls": s.Polls()}, nil
		})
	sdk.AddTool(m, tool("delete_poll", "Delete poll", "Stop a poll; its source keeps the last value.", destructive),
		func(_ context.Context, _ *sdk.CallToolRequest, in keyIn) (*sdk.CallToolResult, map[string]any, error) {
			return nil, map[string]any{"deleted": s.DeletePoll(in.Key)}, nil
		})
}

func isText(p string) bool {
	ct := apps.ContentType(p)
	return strings.HasPrefix(ct, "text/") || strings.Contains(ct, "json") || strings.Contains(ct, "svg")
}

func nonNil(s []string) []string {
	if s == nil {
		return []string{}
	}
	sort.Strings(s)
	return s
}
