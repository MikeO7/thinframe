# server/ — the brains

A single Rust binary in a scratch container. Runs on anything with Docker. The frame has no
logic of its own; it renders `/frame/<id>` and obeys `/api/frames/<id>/config`.

```sh
cd server && docker compose up -d --build
# on the frame (once): doas tfctl set SERVER_URL http://nas.local:8080 FRAME_ID livingroom
```

Dashboard pages live in `server/static/`. Edit, rebuild (or bind-mount `static/` in dev), the frame
reloads on the next config poll when `page_version` changes. Nothing is deployed to the frame.

Tasks: `TASKS.md` section S.

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
