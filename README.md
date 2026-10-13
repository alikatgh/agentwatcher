# AgentWatcher

[![Build](https://github.com/alikatgh/agentwatcher/actions/workflows/build.yml/badge.svg)](https://github.com/alikatgh/agentwatcher/actions/workflows/build.yml)

A native macOS dashboard for watching continuous background coding agents —
no terminal required. The design language, theme and UI patterns are taken
directly from OpenCode's open-sourced macOS client.

## Features

- **Overview** — live status with a pulsing indicator, today's increments and
  a day-over-day delta, and a 1D/5D/1M activity chart with a live endpoint
- **Live Activity** — every agent run as a card; the selected run opens in the
  right sidebar: **Output** (filtered, line-numbered log) and **Context**
  (the session's message list with expandable JSON)
- **Processes** — every live process belonging to the agent (worker loop,
  keep-awake helper, OpenCode runs) with CPU, memory, elapsed time and a
  clean SIGTERM stop button
- **Increments** — every committed increment with message, author, changed
  files and a colorized diff; double-click copies the hash
- **Changes** — the agent's working tree, live: branch and upstream,
  per-file `+N −M` counts with a filter, and a line-numbered diff viewer
  with prev/next — refreshed every couple of seconds while the agent edits
- **Sessions** — filterable list; double-click opens the full step-by-step
  transcript: message cards, collapsible tool calls with pretty input and
  captured output, colored edit diffs, expand/collapse all
- **Live Sessions** — the complete OpenCode interface embedded, paired
  automatically with the one-time link
- **Global right sidebar** — the Changes panel persists on every view
  (⌘B to toggle)
- **Keyboard shortcuts** — ⌘1–⌘8 jump between views, ⌘R refreshes
- **Menu bar watcher** — status, latest commit, start/pause without opening
  the window

## Build

```sh
bash build.sh
```

Produces `dist/AgentWatcher.app` (ad-hoc signed) and
`dist/AgentWatcher-0.1.0.dmg` with a drag-to-Applications layout.
Requires macOS 15+ and Xcode command line tools; the icon generation
additionally uses Python with Pillow (skipped gracefully if absent).

## CI and releases

Every push builds the app and DMG on GitHub Actions, verifies the ad-hoc
signature and DMG checksum, and uploads the DMG as an artifact. Pushing a
`v*` tag publishes a GitHub Release with the DMG attached.

## How it works

AgentWatcher is a thin, safe layer over the same commands a terminal user
would run: status via the worker process and its STOP file, start under
`caffeinate`, pause by creating the STOP file, stop with SIGTERM, live
sessions via `opencode pair`, transcripts via `opencode session export`,
and all repository data via read-only `git` queries. It never modifies your
project files.

## License

No license has been chosen yet. All rights reserved by the author until a
license is added. Portions of the visual design are inspired by OpenCode,
which is MIT licensed.
