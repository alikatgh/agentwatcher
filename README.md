# AgentWatcher

A button-based macOS dashboard for watching and steering a continuous
background coding agent — no terminal required.

AgentWatcher connects to a long-running "worker" script (for example, an
OpenCode agent loop that completes one verified work increment at a time)
and shows, in a single dark dashboard:

- **Overview** — a live status indicator, today's landed increments with a
  day-over-day delta, and a smooth 1D/5D/1M activity chart of increments and
  agent runs.
- **Live Activity** — the streaming driver log.
- **Increments** — the project's committed work, newest first.
- **Sessions** — OpenCode sessions for the project.
- **Live Sessions** — the full OpenCode interface embedded in the app
  (pairing handled automatically).
- **Settings** — project folder, worker script and state folder.

Start, Pause and Force Stop buttons control the worker directly; everything
updates live every few seconds.

## Build

```sh
bash build.sh
```

Produces `dist/AgentWatcher.app` (ad-hoc signed) and
`dist/AgentWatcher-0.1.0.dmg` with a drag-to-Applications layout.

Requirements: macOS 15+, Xcode command line tools, and Python with Pillow
for icon generation (`/opt/homebrew/bin/python3` with `Pillow`).

## How it works

AgentWatcher is a thin, safe layer over the same commands a terminal user
would run:

- status: checks whether the worker process is alive and whether the STOP
  file exists
- Start: launches the worker under `caffeinate` (keeps the Mac awake)
- Pause: creates the worker's STOP file (the worker stops after its current
  increment)
- Force Stop: stops the worker process immediately
- Live Sessions: runs `opencode pair` and embeds the authenticated web
  interface in a WebKit view

No project files are modified by AgentWatcher.

## License

No license has been chosen yet. All rights reserved by the author until a
license is added.
