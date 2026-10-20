# server/ — the brains

A single Go binary (stdlib HTTP + MCP SDK) in a scratch container. Runs on anything with Docker. The frame has no
logic of its own; it renders `/frame/<id>` and obeys `/api/frames/<id>/config`.

```sh
docker compose up -d --build        # from the repo root
# on the frame (once): doas tfctl set SERVER_URL http://nas.local:8080 FRAME_ID livingroom
```

`/frame/<id>` is rendered from the frame's `layout.json` with the templates in `internal/widgets/templates/`
(`server/static/index.html` is an old prototype that no route serves). A layout change bumps `page_version`
and the page reloads itself. Nothing is deployed to the frame.

Tasks: `TASKS.md` section S.

## Data adapters
The page never calls a third party. The server fetches, caches for 10 minutes, and the page reads
`/api/stream/<id>` (SSE, pushed on connect and every 10 minutes) or the JSON endpoints:

| Endpoint | Source | Enabled by |
|---|---|---|
| `GET /api/data/photo/next?frame=<id>` | `PHOTOS_DIR` | `PHOTOS_DIR` |
| `GET /api/data/weather` | open-meteo (no key) | `WEATHER_LAT`, `WEATHER_LON` |
| `GET /api/data/calendar` | public `.ics` feed, no RRULE expansion | `CALENDAR_ICS_URL` (times in `TZ`) |

An unconfigured adapter answers 404. A failed fetch keeps showing the last good data.
The control socket's `hello` reply carries `time_unix`, `tz` (`$TZ`, else `/etc/localtime`, else UTC)
and `server_version`.

## MCP
`/mcp` (streamable HTTP, `Authorization: Bearer $FRAME_TOKEN`). Add to Claude Code:

```sh
claude mcp add --transport http thinframe http://nas.local:8080/mcp --header "Authorization: Bearer $FRAME_TOKEN"
```

Then: "list my frames", "rotate kitchen to landscape", "update all frames to the latest system",
"what does the sim frame look like right now". Tool table in `docs/decisions.md`.

## Artifacts the container carries
`/api/artifacts/` serves `tfctl-armv7`, `overlay.tar.gz`, `system-<ver>.tar` with sha256 sidecars.
Versions are announced in every frame's config response; agents converge on their next poll.
