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
