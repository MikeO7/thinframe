# server/ — the brains

A single Go binary (stdlib HTTP + MCP SDK) in a scratch container. Runs on anything with Docker. The frame has no
logic of its own; it renders `/frame/<id>` and obeys `/api/frames/<id>/config`.

```sh
docker compose up -d --build        # from the repo root
# on the frame (once): doas tfctl set SERVER_URL http://nas.local:41007 FRAME_ID livingroom
```

`/frame/<id>` is rendered from the frame's `layout.json` with the templates in `internal/widgets/templates/`.
A layout change bumps `page_version`
and the page reloads itself. Nothing is deployed to the frame.

Tasks: `TASKS.md` section S.

## Pairing a new frame
Flash the card without a token and power the frame on: it finds the server and shows a 6-digit
pairing code. Open `/admin` on a phone or computer; the frame is listed under "Waiting to pair".
Type the code, press pair, and within a few seconds the frame gets its own token and opens its
dashboard. (Or tell Claude the code: MCP `pair_frame`.) 5 wrong codes and the frame shows a new
one. A paired frame cannot be paired again until you press "forget pairing" on `/admin`.
Endpoints: `POST /api/pair` (answers the code), `GET /api/pair/{id}?nonce=` (the token, once,
after the code was typed), `POST /admin/{id}/pair` (admin auth); `POST /pair/{id}/accept` shows the
code to frames with an older tfctl. See docs/decisions.md "Pairing".

## Host names
Routes without a token (the dashboard, its data, apps, pairing) answer only to IP addresses,
`localhost`, single-label names (`nas`), names under `.local`, `.lan`, `.home.arpa` and
`.internal`, and the container's hostname; anything else gets 421 (this stops DNS rebinding).
If frames or browsers reach the server by another name, list it:
`SERVER_HOSTS=server.nox,frames.example.com`. The token routes and `/mcp` answer under any name.

`/admin` asks for a password when `FRAME_TOKEN` is set: any user name, the token as password
(HTTP Basic). Its forms refuse cross-origin posts. Without a token it is open, like `/api`.

## Data adapters
The page never calls a third party. The server fetches, caches for 10 minutes, and the page reads
`/api/stream/<id>` (SSE, pushed on connect and every 10 minutes) or the JSON endpoints:

| Endpoint | Source | Enabled by |
|---|---|---|
| `GET /api/data/photo/next?frame=<id>` | `PHOTOS_DIR` | `PHOTOS_DIR` |
| `GET /api/data/weather` | open-meteo (no key) | `WEATHER_LAT`, `WEATHER_LON` |
| `GET /api/data/calendar` | `.ics` feeds (`https://` or `webcal://`) and private calendars over CalDAV (iCloud), repeating events expanded, all merged | `CALENDAR_ICS_URL`: one or more URLs separated by spaces, commas or newlines; `CALDAV_USER`, `CALDAV_PASSWORD`, optional `CALDAV_CALENDARS`, `CALDAV_URL` (times in `TZ`) |
| `GET /api/data/camera/<id>` | UniFi Protect snapshot (JPEG), only cameras a `cameras` widget shows or a doorbell that rang | `PROTECT_URL`, `PROTECT_API_KEY`, `PROTECT_INSECURE=1` for the console's self-signed certificate |

**Calendars.** Repeating events (daily, weekly, monthly, yearly; "every 2nd Tuesday", "last
Friday", skipped and moved dates) show for the next 60 days, at the same local time across DST.
A feed that fails is logged and the others still show. What is supported: docs/decisions.md
"Calendar recurrence".

A private iCloud calendar, without publishing it: the server signs in over CalDAV the way an
iPhone does. Make an app-specific password at account.apple.com (Sign-In and Security,
App-Specific Passwords; it can be revoked there at any time and cannot open the Apple ID
settings), then put these in the server's env file, never in Git:

    CALDAV_USER=you@icloud.com
    CALDAV_PASSWORD=abcd-efgh-ijkl-mnop
    CALDAV_CALENDARS=Family,Work        # optional: names as in Calendar; default all

`CALDAV_URL` defaults to `https://caldav.icloud.com`; Fastmail, Nextcloud and other CalDAV
servers work with their URL. Reminders lists are skipped. Changes show within 10 minutes. The
password is sent only to the configured host and its sibling hosts (iCloud keeps each account on
a `pNN-caldav.icloud.com` host), never where a redirect points elsewhere. [U] against the owner's
account: tested against a fake server shaped like iCloud's answers.

A Google calendar, also private: in Google Calendar on the web, Settings, the calendar under
"Settings for my calendars", Integrate calendar, copy "Secret address in iCal format". Put it in
`CALENDAR_ICS_URL` (several links separated by spaces). Only someone with the link can read it,
and Reset there makes a new one; treat it like a password [D]. Google needs OAuth for CalDAV, so
the secret address is the simpler way in.

A public iCloud calendar link (Share Calendar, Public Calendar, the `webcal://` link) also works
in `CALENDAR_ICS_URL`, but anyone with it can read the calendar, and iCloud refreshes it on its
own schedule [D]. CalDAV is private and current.

**Cameras and the doorbell.** Make an API key in Protect (Settings, Control Plane, Integrations).
The `cameras` widget shows snapshots every `interval_s` (default 4 s); a tap opens one camera over
the page. A doorbell ring needs no widget: every page shows the doorbell camera live for 90 s,
and every connected frame gets a `wake` action (an off screen on for 2 minutes, like a touch;
needs a tfctl with `wake`, installed with `update-software`; older frames ignore it).

An unconfigured adapter answers 404. A failed fetch keeps showing the last good data.
The control socket's `hello` reply carries `time_unix`, `tz` (`$TZ`, else `/etc/localtime`, else UTC)
and `server_version`.

## Tabs, the Obsidian theme, and private data
A layout may have `tabs` (`[{id,title,icon,parent,label}]`); each widget names its `tab`, and every
tab is its own `cols x rows` grid. A tab with `parent` is a second page of that tab, picked with a
segmented switch (`label` names it there). After `idle_s` seconds without a touch (default 90) the
page returns to the first tab. `nav` puts the tab bar in a rail on the left (`left`, the default,
which leaves the grid the full height) or as a pill at the bottom (`bottom`). `theme: "aurora"` puts glass cards over soft coloured light that changes with the tab (by its
icon) and, on Home, with the time of day; it builds on obsidian. `theme: "obsidian"` is the dark Apple-style look; it uses Inter if
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

**Old data.** Each push counts as fresh, changed or not. When a widget's source has not been
pushed for a while, the page dims the card and writes "updated 40 min ago" under it: after
`props.stale_s` seconds if the widget sets it (`0` never), else after three missed runs of the
poll that fills the source (at least 15 minutes), else after an hour. A source that is pushed
only when it changes should set `stale_s`. When the page's stream has been down for 30 s it shows
"Offline" at the top.

`SERVICES_DOCKER=<socket>` (in a container: a read-only socket proxy's socket; `scripts/server-deploy.sh`
runs one that allows only `GET /containers/json`, since even a `:ro` mount of the host socket is
full Docker API access) or
`SERVICES_SSH=<host>` (native runs only: the container has no ssh) reads the container list every
`SERVICES_EVERY` (1m) into the source `services`. An unchanged list is not re-sent.

With `SONARR_URL`, `SONARR_API_KEY`, `RADARR_URL` and `RADARR_API_KEY` all set, the server builds
the sources `shows` (the next 7 days from the Sonarr and Radarr calendars, and the 6 newest imports,
one per show or movie) and `today` (episodes and movies out today, plus sunrise and sunset at
`WEATHER_LAT`, `WEATHER_LON`) every `FEEDS_EVERY` (5m), in the `TZ` time zone. It logs once when the
refresh starts failing and once when it works again. The keys go only in the server's env file.

`forecast` shows the week from the weather adapter (`WEATHER_LAT`, `WEATHER_LON`; `WEATHER_UNITS=f`
for °F); the hero shows the current temperature, icon and today's high and low, and with
`props.health` set to a services source its badge reads "21 / 22 containers up".

Car controls (climate, lock, frunk, trunk, charge port) show state from the `cars` source
(`locked`, `frunk`/`trunk`/`port` = `open`, `charging`, `climate {on,set,note}`); a tap says
controls are not connected yet.

## Production on a Docker host
`scripts/server-deploy.sh HOST` builds the image on the Mac, loads it on HOST over ssh and runs it
with compose in `~/thinframe` (host networking for discovery, the Docker socket read-only for the
services widget, the host's time zone). `~/thinframe/env` is made once and then yours to edit;
`~/thinframe/data` holds frames, sources and assets. Run it again to ship a new build. Keep a
local server for testing on another machine with `DISCOVERY=off`, so frames only find production.

What is yours stays out of this repo: names, rooms, containers and pictures go in the layout you PUT
and in `DATA_DIR`, never in Git. `examples/obsidian/` is a demo with made-up names:
`FRAME_TOKEN=... examples/obsidian/load.sh http://localhost:41007 demo`. Sources are server-wide,
so load the demo on a server whose source keys you are not using.

Controls (climate on, lock, scenes) are display-only: the page plane has no credentials, so a tap
on it cannot be trusted to act on a car or a house (docs/decisions.md).

## Apps: any web page, sandboxed
MCP (or anything with the token) can put a whole web page on a frame: `put_app` stores a folder of
HTML/CSS/JS/images as a new version, `show_app` shows it full screen, the widget type `app` puts it
in a grid cell, `screenshot` returns what the panel shows. Pages run sandboxed and get
`window.thinframe` (live sources, buttons that run server-side hooks, shared state, events from
MCP). The MCP tool `app_guide` is the full reference; the design is in `docs/decisions.md` ("Apps").

| Endpoint | Auth | |
|---|---|---|
| `GET /apps/{name}/{path}` | public, sandboxed (CSP) | the app's files; the SDK is injected into HTML (with `?frame=` it carries that frame's emit key, and the document gets no CORS header) |
| `GET /apps-sdk.js` | public | `window.thinframe` |
| `GET /fonts/{name}` | public | bundled fonts (`manrope.woff2`, OFL) |
| `GET /static/{name}` | public | the dashboard's CSS, JS and grain image at content-hashed names, cached for a year (`immutable`); `/frame/{id}` links them, and inlines them for tfctl's saved copy (User-Agent `tfctl`) or `?inline=1` |
| `GET /api/data/source/{key}` | public | one source, for apps |
| `GET /api/apps/{name}/state`, `POST .../state/{key}` | public, rate limited | app state (JSON, null deletes) |
| `POST /api/frames/{id}/emit?k=` | the frame's emit key, rate limited | `{app,name,data}`; runs the hook `name`. The key (HMAC of `FRAME_TOKEN` and the id) is only in the page and full-screen apps the server serves; no token, no key |
| `POST /api/frames/{id}/screenshot?w=&h=` | frame | raw RGB from the frame |
| `GET /api/frames/{id}/screenshot` | bearer | the newest screenshot, PNG |

## MCP
`/mcp`: streamable HTTP, stateless, `Authorization: Bearer $FRAME_TOKEN`. Speaks MCP
**2026-07-28** (`server/discover`, no handshake) and falls back per client to 2025-11-25, 2025-06-18,
2025-03-26 and 2024-11-05. Official Go SDK v1.8.0. Tools carry titles and read-only/destructive
hints, results are JSON objects (structuredContent), and browser requests from other sites are refused.

`MCP_TOKENS` adds bearer tokens that open `/mcp` and nothing else (comma or space separated, at least
32 characters each). Give each MCP client its own, and remove one to revoke that client without
changing `FRAME_TOKEN`.

Claude Code:
```sh
claude mcp add --transport http thinframe http://nas.local:41007/mcp --header "Authorization: Bearer $FRAME_TOKEN"
```

Hermes Agent (`~/.hermes/config.yaml`):
```yaml
mcp_servers:
  thinframe:
    url: "http://nas.local:41007/mcp"
    headers:
      Authorization: "Bearer <FRAME_TOKEN>"
```

Any other client (Cursor, Codex, Claude Desktop through a remote connector, the MCP Inspector) works the same way:
URL `http://<host>:41007/mcp`, transport "streamable HTTP" (sometimes just "http"), the header above. There is no
OAuth. A client that insists on OAuth needs the header option.
Check it with `npx @modelcontextprotocol/inspector --cli http://nas.local:41007/mcp --transport http
--header "Authorization: Bearer $FRAME_TOKEN" --method tools/list`.

Then: "list my frames", "rotate kitchen to landscape", "update all frames to the latest system",
"what does the sim frame look like right now". Tool table in `docs/decisions.md`.

## Artifacts the container carries
`/api/artifacts/` serves `tfctl-armv7` and `system-<ver>.tar` with sha256 sidecars.
Versions are announced in every frame's config response; agents converge on their next poll.
