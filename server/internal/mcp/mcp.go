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

// Handler returns the /mcp http.Handler with bearer auth matching the REST API.
func Handler(s *api.Server) http.Handler {
	server := sdk.NewServer(&sdk.Implementation{Name: "thinframe", Version: "0.1.0"}, nil)
	register(server, s)
	h := sdk.NewStreamableHTTPHandler(func(*http.Request) *sdk.Server { return server }, &sdk.StreamableHTTPOptions{Stateless: true})
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !s.TokenOK(r) {
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}
		h.ServeHTTP(w, r)
	})
}

type idIn struct {
	ID string `json:"id" jsonschema:"frame id, e.g. kitchen"`
}
type empty struct{}

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

func text(v any) *sdk.CallToolResult {
	b, _ := json.MarshalIndent(v, "", "  ")
	return &sdk.CallToolResult{Content: []sdk.Content{&sdk.TextContent{Text: string(b)}}}
}

func register(m *sdk.Server, s *api.Server) {
	sdk.AddTool(m, &sdk.Tool{Name: "list_frames", Description: "List every known frame with online state, last heartbeat (ip, memory, temperature, latency, versions) and config."},
		func(_ context.Context, _ *sdk.CallToolRequest, _ empty) (*sdk.CallToolResult, []frameOut, error) {
			fs := s.Store.List()
			out := make([]frameOut, len(fs))
			for i, f := range fs {
				out[i] = toOut(s, f)
			}
			return text(out), out, nil
		})

	sdk.AddTool(m, &sdk.Tool{Name: "get_frame", Description: "Full detail for one frame: config, layout, pending actions, last 20 events."},
		func(_ context.Context, _ *sdk.CallToolRequest, in idIn) (*sdk.CallToolResult, any, error) {
			f, err := s.Store.Get(in.ID)
			if err != nil {
				return nil, nil, err
			}
			ev := f.Events
			if len(ev) > 20 {
				ev = ev[len(ev)-20:]
			}
			out := map[string]any{"frame": toOut(s, f), "layout": f.Layout, "pending": f.Pending, "events": ev}
			return text(out), out, nil
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
	}
	sdk.AddTool(m, &sdk.Tool{Name: "set_frame_config", Description: "Change one or more config fields on a frame. Pushed to the frame immediately if it is online."},
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
			if len(patch) == 0 {
				return nil, nil, fmt.Errorf("nothing to change")
			}
			f, err := s.SetConfig(in.ID, patch)
			if err != nil {
				return nil, nil, err
			}
			out := s.ConfigFor(f)
			return text(out), out, nil
		})

	type actIn struct {
		ID      string `json:"id,omitempty" jsonschema:"frame id; omit with confirm=true to target every frame"`
		Name    string `json:"name" jsonschema:"one of: reboot, restart-kiosk, reload-page, screen, update-software, update-system, rollback, screenshot"`
		Arg     string `json:"arg,omitempty" jsonschema:"argument, e.g. on|off for screen"`
		Confirm bool   `json:"confirm,omitempty" jsonschema:"required true when id is omitted (applies to all frames)"`
	}
	sdk.AddTool(m, &sdk.Tool{Name: "frame_action", Description: "Send a one-shot action to a frame (or all frames with confirm=true). Returns the queued action; the frame acks it over its socket."},
		func(_ context.Context, _ *sdk.CallToolRequest, in actIn) (*sdk.CallToolResult, any, error) {
			var args map[string]string
			if in.Arg != "" {
				args = map[string]string{"arg": in.Arg}
			}
			if in.ID == "" {
				if !in.Confirm {
					return nil, nil, fmt.Errorf("refusing to act on all frames without confirm=true")
				}
				var out []store.Action
				for _, f := range s.Store.List() {
					a, err := s.Act(f.ID, in.Name, args)
					if err != nil {
						return nil, nil, err
					}
					out = append(out, a)
				}
				return text(out), out, nil
			}
			a, err := s.Act(in.ID, in.Name, args)
			if err != nil {
				return nil, nil, err
			}
			return text(a), a, nil
		})

	sdk.AddTool(m, &sdk.Tool{Name: "list_widgets", Description: "Widget types available for layouts, with their props."},
		func(_ context.Context, _ *sdk.CallToolRequest, _ empty) (*sdk.CallToolResult, any, error) {
			out := map[string]any{
				"types": widgets.Types,
				"props": map[string]string{
					"photo": "interval_s (seconds between photos, default 600)", "text": "text", "iframe": "src (URL)",
					"weather": "place (label)", "clock": "", "status": "",
				},
				"grid":    "cols x rows (1..12 each); widgets have x,y,w,h in cells and must not overlap",
				"example": store.DefaultLayout(),
			}
			return text(out), out, nil
		})

	type layoutIn struct {
		ID     string       `json:"id" jsonschema:"frame id"`
		Layout store.Layout `json:"layout" jsonschema:"the complete layout to set (cols, rows, theme, widgets[])"`
	}
	sdk.AddTool(m, &sdk.Tool{Name: "set_layout", Description: "Replace a frame's dashboard layout. Validated (grid bounds, overlaps, known types). The page reloads immediately."},
		func(_ context.Context, _ *sdk.CallToolRequest, in layoutIn) (*sdk.CallToolResult, store.Layout, error) {
			f, err := s.SetLayout(in.ID, in.Layout)
			if err != nil {
				return nil, store.Layout{}, err
			}
			return text(f.Layout), f.Layout, nil
		})

	type logsIn struct {
		ID string `json:"id"`
		N  int    `json:"n,omitempty" jsonschema:"number of events, default 50"`
	}
	sdk.AddTool(m, &sdk.Tool{Name: "get_logs", Description: "Recent events for a frame: connections, config pushes, actions and their acks, frame-reported events."},
		func(_ context.Context, _ *sdk.CallToolRequest, in logsIn) (*sdk.CallToolResult, any, error) {
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
			return &sdk.CallToolResult{Content: []sdk.Content{&sdk.TextContent{Text: sb.String()}}}, ev, nil
		})

	sdk.AddTool(m, &sdk.Tool{Name: "release_status", Description: "Which tfctl/system versions this server carries and which frames are behind."},
		func(_ context.Context, _ *sdk.CallToolRequest, _ empty) (*sdk.CallToolResult, any, error) {
			out := s.ReleaseStatus()
			return text(out), out, nil
		})
}
