// Package mcp exposes the server to Claude over MCP (streamable HTTP at /mcp).
// Every tool is a thin wrapper over *api.Server, so REST, admin and MCP share one code path.
package mcp

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"strings"

	sdk "github.com/modelcontextprotocol/go-sdk/mcp"

	"github.com/MikeO7/thinframe/server/internal/api"
	"github.com/MikeO7/thinframe/server/internal/store"
	"github.com/MikeO7/thinframe/server/internal/widgets"
)

// Instructions tell an MCP client (Claude Code, Hermes, ...) what this server is for.
const Instructions = "thinframe manages wall-mounted picture frames. Each frame shows a dashboard " +
	"rendered by this server. Start with list_frames. Change a frame with set_frame_config " +
	"(rotation, brightness, schedule, splash) or set_layout (see list_widgets). frame_action reboots, " +
	"updates, rolls back or runs bench (touch to paint, result in get_logs, samples in list_frames); omitting id targets every frame and needs confirm=true. " +
	"For anything the widgets cannot do, write an app: any web page (HTML/CSS/JS) with live data, buttons and server-side API calls, " +
	"shown full screen (show_app) or as a widget. Read app_guide, then put_app, show_app and screenshot to see the result."

// Handler returns the /mcp http.Handler with bearer auth matching the REST API.
// Streamable HTTP, stateless: speaks MCP 2026-07-28 (server/discover, per-request _meta) and the
// older initialize handshake (2025-11-25 back to 2024-11-05), negotiated per client by the SDK.
func Handler(s *api.Server, version string) http.Handler {
	server := sdk.NewServer(&sdk.Implementation{Name: "thinframe", Title: "thinframe", Version: version},
		&sdk.ServerOptions{Instructions: Instructions})
	register(server, s)
	h := sdk.NewStreamableHTTPHandler(func(*http.Request) *sdk.Server { return server }, &sdk.StreamableHTTPOptions{
		Stateless: true,
		// The spec requires Origin validation: a browser page from another site is refused.
		// CLI clients send no Origin and pass.
		CrossOriginProtection: http.NewCrossOriginProtection(),
		MaxRequestBodyBytes:   8 << 20, // put_app carries up to 4 MB of files, base64 for binaries
	})
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !s.TokenOK(r) {
			w.Header().Set("WWW-Authenticate", `Bearer realm="thinframe", error="invalid_token"`)
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}
		h.ServeHTTP(w, r)
	})
}

func ptr[T any](v T) *T { return &v }

// Hints for clients that ask before running a tool. Nothing here reaches outside the server and
// its frames, so OpenWorld is false everywhere.
var (
	readOnly    = &sdk.ToolAnnotations{ReadOnlyHint: true, OpenWorldHint: ptr(false)}
	safeWrite   = &sdk.ToolAnnotations{DestructiveHint: ptr(false), IdempotentHint: true, OpenWorldHint: ptr(false)}
	replaces    = &sdk.ToolAnnotations{DestructiveHint: ptr(true), IdempotentHint: true, OpenWorldHint: ptr(false)}
	destructive = &sdk.ToolAnnotations{DestructiveHint: ptr(true), OpenWorldHint: ptr(false)}
)

func tool(name, title, desc string, a *sdk.ToolAnnotations) *sdk.Tool {
	ann := *a
	ann.Title = title
	return &sdk.Tool{Name: name, Title: title, Description: desc, Annotations: &ann}
}

type idIn struct {
	ID string `json:"id" jsonschema:"frame id, e.g. kitchen"`
}
type empty struct{}

// Results are JSON objects: structuredContent and outputSchema must be objects for clients on
// protocol versions before 2026-07-28 (the Python and TypeScript SDKs validate this).
type framesOut struct {
	Frames []frameOut `json:"frames"`
}

type frameOut struct {
	ID      string         `json:"id"`
	Name    string         `json:"name,omitempty"`
	Online  bool           `json:"online"`
	Config  map[string]any `json:"config"`
	Last    any            `json:"last_heartbeat,omitempty"`
	Pending int            `json:"pending_actions"`
}

func toOut(s *api.Server, f store.Frame) frameOut {
	return frameOut{ID: f.ID, Name: f.Name, Online: s.Online(f), Config: s.ConfigFor(f), Last: f.Last, Pending: len(f.Pending)}
}

func register(m *sdk.Server, s *api.Server) {
	registerApps(m, s)
	sdk.AddTool(m, tool("list_frames", "List frames", "List every known frame with online state, last heartbeat (ip, memory, temperature, latency, versions) and config.", readOnly),
		func(_ context.Context, _ *sdk.CallToolRequest, _ empty) (*sdk.CallToolResult, framesOut, error) {
			fs := s.Store.List()
			out := framesOut{Frames: make([]frameOut, len(fs))}
			for i, f := range fs {
				out.Frames[i] = toOut(s, f)
			}
			return nil, out, nil
		})

	sdk.AddTool(m, tool("get_frame", "Get frame", "Full detail for one frame: config, layout, pending actions, last 20 events.", readOnly),
		func(_ context.Context, _ *sdk.CallToolRequest, in idIn) (*sdk.CallToolResult, map[string]any, error) {
			f, err := s.Store.Get(in.ID)
			if err != nil {
				return nil, nil, err
			}
			ev := f.Events
			if len(ev) > 20 {
				ev = ev[len(ev)-20:]
			}
			out := map[string]any{"frame": toOut(s, f), "layout": f.Layout, "pending": f.Pending, "events": ev}
			return nil, out, nil
		})

	type setCfgIn struct {
		ID         string   `json:"id" jsonschema:"frame id"`
		Name       *string  `json:"name,omitempty" jsonschema:"human name"`
		Rotation   *int     `json:"rotation,omitempty" jsonschema:"quarter turns 0..3; 3 = landscape with the cable at the bottom, 1 = cable at the top"`
		Brightness *int     `json:"brightness,omitempty" jsonschema:"0..100"`
		ScreenOff  *string  `json:"screen_off,omitempty" jsonschema:"HH:MM local, empty to disable"`
		ScreenOn   *string  `json:"screen_on,omitempty" jsonschema:"HH:MM local"`
		URL        *string  `json:"url,omitempty" jsonschema:"override page URL; empty = this server's dashboard"`
		Tags       []string `json:"tags,omitempty"`
		SplashText *string  `json:"splash_text,omitempty" jsonschema:"one line under the boot logo; shown from the next boot"`
		SplashBG   *string  `json:"splash_bg,omitempty" jsonschema:"boot splash background #rrggbb"`
		SplashPNG  *string  `json:"splash_png,omitempty" jsonschema:"boot logo as base64 PNG, at most 48 KiB; empty stops sending it (the frame keeps its last logo)"`
		Debug      *bool    `json:"debug,omitempty" jsonschema:"true: tfctl logs everything (socket traffic, config writes, cog with G_MESSAGES_DEBUG=all) and posts a report on every connect"`
	}
	sdk.AddTool(m, tool("set_frame_config", "Set frame config", "Change one or more config fields on a frame. Pushed to the frame immediately if it is online.", safeWrite),
		func(_ context.Context, _ *sdk.CallToolRequest, in setCfgIn) (*sdk.CallToolResult, map[string]any, error) {
			patch := map[string]any{}
			if in.Name != nil {
				patch["name"] = *in.Name
			}
			if in.Rotation != nil {
				patch["rotation"] = *in.Rotation
			}
			if in.Brightness != nil {
				patch["brightness"] = *in.Brightness
			}
			if in.ScreenOff != nil {
				patch["screen_off"] = *in.ScreenOff
			}
			if in.ScreenOn != nil {
				patch["screen_on"] = *in.ScreenOn
			}
			if in.URL != nil {
				patch["url"] = *in.URL
			}
			if in.Tags != nil {
				patch["tags"] = in.Tags
			}
			if in.SplashText != nil {
				patch["splash_text"] = *in.SplashText
			}
			if in.SplashBG != nil {
				patch["splash_bg"] = *in.SplashBG
			}
			if in.SplashPNG != nil {
				patch["splash_png"] = *in.SplashPNG
			}
			if in.Debug != nil {
				patch["debug"] = *in.Debug
			}
			if len(patch) == 0 {
				return nil, nil, fmt.Errorf("nothing to change")
			}
			f, err := s.SetConfig(in.ID, patch)
			if err != nil {
				return nil, nil, err
			}
			out := s.ConfigFor(f)
			return nil, out, nil
		})

	type actIn struct {
		ID      string `json:"id,omitempty" jsonschema:"frame id; omit with confirm=true to target every frame"`
		Name    string `json:"name" jsonschema:"one of: reboot, restart-kiosk, reload-page, screen, update-software, update-system, rollback, screenshot, bench, report (the frame posts a debug report: versions, config, mounts, processes, dmesg, this and the previous boot's log; read it with get_report)"`
		Arg     string `json:"arg,omitempty" jsonschema:"argument, e.g. on|off for screen"`
		Confirm bool   `json:"confirm,omitempty" jsonschema:"required true when id is omitted (applies to all frames)"`
	}
	type actionsOut struct {
		Actions []store.Action `json:"actions"`
	}
	sdk.AddTool(m, tool("frame_action", "Frame action", "Send a one-shot action to a frame (or all frames with confirm=true). Returns the queued actions; the frame acks each over its socket.", destructive),
		func(_ context.Context, _ *sdk.CallToolRequest, in actIn) (*sdk.CallToolResult, actionsOut, error) {
			var args map[string]string
			if in.Arg != "" {
				args = map[string]string{"arg": in.Arg}
			}
			if in.ID == "" {
				if !in.Confirm {
					return nil, actionsOut{}, fmt.Errorf("refusing to act on all frames without confirm=true")
				}
				out := actionsOut{Actions: []store.Action{}}
				for _, f := range s.Store.List() {
					a, err := s.Act(f.ID, in.Name, args)
					if err != nil {
						return nil, actionsOut{}, err
					}
					out.Actions = append(out.Actions, a)
				}
				return nil, out, nil
			}
			a, err := s.Act(in.ID, in.Name, args)
			if err != nil {
				return nil, actionsOut{}, err
			}
			return nil, actionsOut{Actions: []store.Action{a}}, nil
		})

	sdk.AddTool(m, tool("list_widgets", "List widgets", "Widget types available for layouts, with their props.", readOnly),
		func(_ context.Context, _ *sdk.CallToolRequest, _ empty) (*sdk.CallToolResult, map[string]any, error) {
			out := map[string]any{
				"types":   widgets.Types,
				"props":   widgets.Props,
				"images":  "http(s) URL or asset:<file> from DATA_DIR/assets",
				"tabs":    "optional tabs [{id,title,icon,parent}] (icons: " + strings.Join(widgets.IconNames(), ", ") + "); widgets set tab; every tab shares cols x rows; idle_s returns to the first tab; nav left (default, a rail on the left) or bottom",
				"themes":  "light, dark, obsidian",
				"apps":    "type app shows an app (put_app; see app_guide) in the cell: any HTML/CSS/JS, live data and buttons",
				"grid":    "cols x rows (1..12 each); widgets have x,y,w,h in cells and must not overlap",
				"example": store.DefaultLayout(),
			}
			return nil, out, nil
		})

	type layoutIn struct {
		ID     string       `json:"id" jsonschema:"frame id"`
		Layout store.Layout `json:"layout" jsonschema:"the complete layout to set (cols, rows, theme, widgets[])"`
	}
	sdk.AddTool(m, tool("set_layout", "Set layout", "Replace a frame's dashboard layout. Validated (grid bounds, overlaps, known types). The page reloads immediately.", replaces),
		func(_ context.Context, _ *sdk.CallToolRequest, in layoutIn) (*sdk.CallToolResult, store.Layout, error) {
			f, err := s.SetLayout(in.ID, in.Layout)
			if err != nil {
				return nil, store.Layout{}, err
			}
			return nil, f.Layout, nil
		})

	type logsIn struct {
		ID string `json:"id"`
		N  int    `json:"n,omitempty" jsonschema:"number of events, default 50"`
	}
	sdk.AddTool(m, tool("get_logs", "Get logs", "Recent events for a frame: connections, config pushes, actions and their acks, frame-reported events.", readOnly),
		func(_ context.Context, _ *sdk.CallToolRequest, in logsIn) (*sdk.CallToolResult, map[string]any, error) {
			f, err := s.Store.Get(in.ID)
			if err != nil {
				return nil, nil, err
			}
			n := in.N
			if n <= 0 || n > len(f.Events) {
				n = len(f.Events)
			}
			ev := f.Events[len(f.Events)-n:]
			var sb strings.Builder
			for _, e := range ev {
				fmt.Fprintf(&sb, "%s %-10s %s\n", e.At.Format("2006-01-02 15:04:05"), e.Kind, e.Text)
			}
			return &sdk.CallToolResult{Content: []sdk.Content{&sdk.TextContent{Text: sb.String()}}}, map[string]any{"events": ev}, nil
		})

	type reportIn struct {
		ID   string `json:"id" jsonschema:"frame id"`
		Name string `json:"name,omitempty" jsonschema:"report file name from list_reports; omitted = the newest"`
		Tail int    `json:"tail,omitempty" jsonschema:"return only the last N bytes (a full report can be 300 KB)"`
	}
	sdk.AddTool(m, tool("list_reports", "List reports", "Debug reports a frame has posted (newest first). A frame posts one on the report action, and on every connect while its debug config is true.", readOnly),
		func(_ context.Context, _ *sdk.CallToolRequest, in idIn) (*sdk.CallToolResult, map[string]any, error) {
			list, err := s.ListReports(in.ID)
			if err != nil {
				return nil, nil, err
			}
			return nil, map[string]any{"reports": list}, nil
		})
	sdk.AddTool(m, tool("get_report", "Get report", "The text of one debug report (newest by default): tfctl versions and slot, thinframe.conf, uEnv, mounts, df, ps, dmesg, iwctl, this boot's log and the previous boot's log.", readOnly),
		func(_ context.Context, _ *sdk.CallToolRequest, in reportIn) (*sdk.CallToolResult, map[string]any, error) {
			name := in.Name
			if name == "" {
				name = "latest"
			}
			b, err := s.ReadReport(in.ID, name)
			if err != nil {
				return nil, nil, err
			}
			if in.Tail > 0 && in.Tail < len(b) {
				b = b[len(b)-in.Tail:]
			}
			return &sdk.CallToolResult{Content: []sdk.Content{&sdk.TextContent{Text: string(b)}}}, map[string]any{"bytes": len(b)}, nil
		})

	type srcIn struct {
		Key  string         `json:"key" jsonschema:"source key (lowercase letters, digits, - and _); widgets read it with props.source"`
		Data map[string]any `json:"data" jsonschema:"the whole object the widgets receive as data (see list_widgets props)"`
	}
	sdk.AddTool(m, tool("set_source", "Set source", "Replace a data source and push it to every widget that reads it. Kept in DATA_DIR/sources across restarts.", replaces),
		func(_ context.Context, _ *sdk.CallToolRequest, in srcIn) (*sdk.CallToolResult, map[string]any, error) {
			b, err := json.Marshal(in.Data)
			if err != nil {
				return nil, nil, err
			}
			if err := s.SetSource(in.Key, b); err != nil {
				return nil, nil, err
			}
			return nil, map[string]any{"key": in.Key, "bytes": len(b)}, nil
		})
	sdk.AddTool(m, tool("list_sources", "List sources", "Data sources the server holds: key, size, last update.", readOnly),
		func(_ context.Context, _ *sdk.CallToolRequest, _ empty) (*sdk.CallToolResult, map[string]any, error) {
			return nil, map[string]any{"sources": s.Sources()}, nil
		})

	sdk.AddTool(m, tool("release_status", "Release status", "Which tfctl/system versions this server carries and which frames are behind.", readOnly),
		func(_ context.Context, _ *sdk.CallToolRequest, _ empty) (*sdk.CallToolResult, map[string]any, error) {
			out := s.ReleaseStatus()
			return nil, out, nil
		})
}
