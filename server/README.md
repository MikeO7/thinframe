# server/ — the brains

A single Go binary (stdlib HTTP + MCP SDK) in a scratch container. Runs on anything with Docker. The frame has no
logic of its own; it renders `/frame/<id>` and obeys `/api/frames/<id>/config`.

```sh
docker compose up -d --build        # from the repo root
# on the frame (once): doas tfctl set SERVER_URL http://nas.local:8080 FRAME_ID livingroom
```

`/frame/<id>` is rendered from the frame's `layout.json` with the templates in `internal/widgets/templates/`.
A layout change bumps `page_version`
and the page reloads itself. Nothing is deployed to the frame.

Tasks: `TASKS.md` section S.

## Pairing a new frame
Flash the card without a token and power the frame on: it finds the server at `SERVER_URL` and
shows a PAIR button. Tap it once; the server gives that frame its own token and the dashboard
opens. A paired frame cannot be paired again until you press "forget pairing" on `/admin`.
Endpoints: `POST /api/pair`, `POST /pair/{id}/accept`, `GET /api/pair/{id}?nonce=` (no auth; see
docs/decisions.md "One-press pairing").

`/admin` asks for a password when `FRAME_TOKEN` is set: any user name, the token as password
(HTTP Basic). Its forms refuse cross-origin posts. Without a token it is open, like `/api`.

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

## Tabs, the Obsidian theme, and private data
A layout may have `tabs` (`[{id,title,icon,parent,label}]`); each widget names its `tab`, and every
tab is its own `cols x rows` grid. A tab with `parent` is a second page of that tab, picked with a
segmented switch (`label` names it there). After `idle_s` seconds without a touch (default 90) the
page returns to the first tab. `theme: "obsidian"` is the dark Apple-style look; it uses Inter if
`DATA_DIR/assets/inter.woff2` exists (OFL, not shipped here), else DejaVu Sans.

Widgets `hero agenda cars rooms climate list tiles energy shows worldclock services` draw from
props plus pushed data (props per type: MCP `list_widgets`). Data comes from **sources**: named JSON
objects you push, kept in `DATA_DIR/sources` and replayed when a page connects.

| Endpoint | Auth | |
|---|---|---|
| `PUT /api/sources/{key}` | bearer | replace a source (JSON object, at most 256 KiB); pushed to every widget with `props.source == key` |
| `GET /api/sources`, `GET /api/sources/{key}` | bearer | list, read |
| `DELETE /api/sources/{key}` | bearer | forget |
| `GET /api/data/asset/{name}` | public, like the page | PNG, JPEG, WebP, GIF or woff2 from `DATA_DIR/assets`; widgets say `asset:<name>` |

`SERVICES_SSH=<host>` (native runs only: the container has no ssh) polls `docker ps` there every
`SERVICES_EVERY` (1m) into the source `services`.

What is yours stays out of this repo: names, rooms, containers and pictures go in the layout you PUT
and in `DATA_DIR`, never in Git. `examples/obsidian/` is a demo with made-up names:
`FRAME_TOKEN=... examples/obsidian/load.sh http://localhost:8080 demo`. Sources are server-wide,
so load the demo on a server whose source keys you are not using.

Controls (climate on, lock, scenes) are display-only: the page plane has no credentials, so a tap
on it cannot be trusted to act on a car or a house (docs/decisions.md).

## MCP
`/mcp`: streamable HTTP, stateless, `Authorization: Bearer $FRAME_TOKEN`. Speaks MCP
**2026-07-28** (`server/discover`, no handshake) and falls back per client to 2025-11-25, 2025-06-18,
2025-03-26 and 2024-11-05. Official Go SDK v1.8.0. Tools carry titles and read-only/destructive
hints, results are JSON objects (structuredContent), and browser requests from other sites are refused.

Claude Code:
```sh
claude mcp add --transport http thinframe http://nas.local:8080/mcp --header "Authorization: Bearer $FRAME_TOKEN"
```

Hermes Agent (`~/.hermes/config.yaml`):
```yaml
mcp_servers:
  thinframe:
    url: "http://nas.local:8080/mcp"
    headers:
      Authorization: "Bearer <FRAME_TOKEN>"
```

Any other client (Cursor, Codex, Claude Desktop through a remote connector, the MCP Inspector) works the same way:
URL `http://<host>:8080/mcp`, transport "streamable HTTP" (sometimes just "http"), the header above. There is no
OAuth. A client that insists on OAuth needs the header option.
Check it with `npx @modelcontextprotocol/inspector --cli http://nas.local:8080/mcp --transport http
--header "Authorization: Bearer $FRAME_TOKEN" --method tools/list`.

Then: "list my frames", "rotate kitchen to landscape", "update all frames to the latest system",
"what does the sim frame look like right now". Tool table in `docs/decisions.md`.

## Artifacts the container carries
`/api/artifacts/` serves `tfctl-armv7` and `system-<ver>.tar` with sha256 sidecars.
Versions are announced in every frame's config response; agents converge on their next poll.
