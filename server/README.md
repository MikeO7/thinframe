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
