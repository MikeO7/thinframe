# Apps

Source of the MCP apps installed on the server. The server keeps its own copy and the last 10
versions in `DATA_DIR/apps/<name>/` (docs/decisions.md, "Apps"); this folder is the backup and
the place to review changes. After changing an app here, install it with the MCP tool
`put_app {"name": "<folder>", "files": {...}, "note": "what changed"}` and check it with
`show_app` and `screenshot`.

| App | Version on the server | What |
|---|---|---|
| `cars/` | v4 (2026-10-26) | Tesla status and controls for two cars. Buttons go through the server hook `car` (Home Assistant script `script.thinframe_car`); the hook and its token live on the server, not here. |
