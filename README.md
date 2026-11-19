# AgentWatcher

**Visuals first. Make running agents simple to see and control.**

Our product vision is progressive disclosure: a calm visual overview first,
then deeper activity, evidence, and controls when the user asks. All power-user
functionality remains accessible through inspection, search, shortcuts, and the
command palette. Visual polish, readability, and speed are core requirements.

A native **Rust + GPUI** macOS application for watching coding agents. The interface uses a compact Zed-style workspace: project navigation, recorded work in the center, and a resizable Changes/Details dock.

The current application, collectors, process controls, and text input are written in Rust. There is no Swift or web view in the Rust build. The previous Swift implementation remains in `Sources/` untouched for reference; `build.sh` is its legacy build script.

- **Dashboard** opens first with a timestamped 1h/24h activity chart, a connected parent/child agent map, and the selected agent’s outcomes. Select a time slice to reveal its actual agents and their recorded relationships; select a node to inspect its scoped work. Open controls and traces on request. The Agent list and all prior history remain accessible. Outcome segments and count labels preview their scoped events on hover and open matching traces on click. Charts count loaded recording windows and label their coverage.
- **Graph** is a live agent node canvas with a focused family view, an All loaded agents overview, provider filters, connected parent/child ports, pan/zoom, draggable nodes, ten deterministic arrangement presets, and a selected-agent inspector. Owned runs and recent explicit lifecycle evidence drive live borders and branch signals. Recent work without lifecycle evidence is labelled Activity; completed/stopped recordings stay static. Recorded handoffs retain their separate evidence-based signals. Hover a node to turn its output socket into **+**: it opens a right sidebar for the next task, name, selected skills, workspace, access, and time limit. **Start agent** runs Codex inside Graph, streams its events, and links the new node to its launch origin. The toolbar **+ Agent** starts an independent task. Owned runs expose their actual state and Stop control. The inspector shows current and recorded AGENTS.md instructions, observed skill reads/announcements, separately labelled skill versions, and evidence links. A click opens the agent’s right sidebar with work and Start/Continue controls; double-click focuses the node. Activity marks open exact event evidence. Visual / Raw / Tree tabs show structured tool inputs and outputs, a connected expandable data tree, and selectable source text. Tree and Visual keep separate zoom and scroll positions. Scroll zooms around the pointer; left-drag pans.
- **Live Activity** follows the selected agent with readable event times, searchable trajectory and tools, earlier/newer history windows, and selectable evidence. Turn off Following latest to read without automatic scrolling.
- **Processes** shows scoped agent processes. Stopping requires confirmation and rechecks the identity captured when the process was selected.
- **Increments** provides commit history and a virtualized, numbered patch viewer.
- **Changes** provides working-tree/staged changes, literal filename handling, and a persistent dock.
- **Agent history** browses prior Codex and DeepSeek agents across workspaces, with provider filters, parent/child links, search, older pages, and rescanning. OpenCode discovery/export runs only when explicitly selected.
- **Agents & Skills** catalogs installed skills, agent profiles, current instruction hierarchy, and recorded agents. Search by scope, inspect source and model/workspace details, and explicitly choose skills for a new Graph agent. Current files and recorded usage remain separately labelled.
- **Pane chat** opens from **Ask** or a viewer's status strip. It has separate You/Assistant messages, per-message copying, a selectable transcript, previewable context, and a pinned native multiline composer. Choose OpenAI, Anthropic, Google Gemini, OpenRouter, DeepSeek, Mistral, Groq, xAI, Ollama, or a custom endpoint using a supported API format. Enter the exact model identifier and your provider's key; no vendor key prefix is assumed. Keys stay in memory and clear when the provider, endpoint, or API format changes. Conversations and drafts are scoped to the workspace and pane/document. Every Send names the destination and requires explicit context-sharing and possible-usage acknowledgement. Requests have no tools, retries, automatic follow-ups, or background sends. Stop also cancels work waiting in the local queue.
- **Workspace files and Console** open through Explore tools or the command palette. The file tree expands folders on demand and opens ordinary files in the native editor. Console starts only through its own Start button, accepts explicit commands, and displays selectable plain-text PTY output. Selection pauses its display; Resume catches up. These tools stay closed by default.
- **Settings** exposes 61 working controls across General, Appearance, Viewers, Graph, Activity, Privacy, Launching, and Advanced. Overview shows a small selection; categories and search reveal the full set. Workers never start automatically.

Git, process collection, and history parsing run on bounded background workers. Each worker runs one request and retains only the latest pending request. Local recording changes are checked each second; the overview retains up to 64 small summaries and reads at most two changed recordings per pass. Large traces use bounded, navigable windows. The UI does not wait for collection before drawing or navigating.

Settings writes now use one ordered background writer with a latest pending
value, visible failures, and explicit retry. Normal quit waits for accepted
settings, document confirmation, and owned work; native termination uses the
same deferred flow. Graph camera frames reuse cached activity and lifecycle
state. The launch sidebar has an independent scroll body, and its graph socket
stays a fixed physical size. Required control outlines use a separate semantic
color from decorative separators in every appearance.

UI text uses IBM Plex Sans, matching Zed's bundled UI family. Code uses JetBrains Mono Nerd Font Mono, matching the installed Zed configuration. Fonts and upstream licenses are bundled. Light and dark appearances have separate semantic colors for surfaces, selection, focus, syntax, additions, and deletions.

The application embeds the actual Zed Preview `Editor` and `language::Buffer` by default in existing CodeView file, snapshot, evidence, diff, log, and pane-chat surfaces. All use the same pinned upstream GPUI source at Zed `v1.25.0-pre`, revision `aa9c16803dd841950e3d4f4c7ecd76f324e0ba44`. The October 16 v5 native app linked, packaged, passed strict signature verification, and launched. Its root Rust suite passed 319 tests with five ignored. Native checks exercised file opening from Graph, wrapped vertical reading, Unicode paste, undo/redo, exact CRLF saving and recovery, evidence scrolling, narrow chat controls, theme switching, and dirty-document quit cancellation. See [the artifact record](docs/verification/2026-10-16-native-final/artifacts.json) and [the integration record](docs/zed-editor-integration.md) for scope and remaining work.

The [2026-10-16 speed verification](docs/verification/2026-10-16-astra-speed-ui.json)
records a 3.68x improvement in a 4.5 MB source-constructor fixture, zero extra
allocations in 10,000 warm graph-cache reads, and 1.80x faster redaction of a
synthetic metadata-heavy JSON fixture. These measure components, not packaged
startup, input latency, rendering, or app-wide performance. Native interaction
checks now pass for the recorded cases; a complete performance comparison and
the broader accessibility, IME, and service matrix remain pending.

## Build and run

Requires macOS, Xcode command-line tools, Git, Python 3.11 or later, and Rust 1.98.1. `rust-toolchain.toml` selects that compiler for this project without changing the user's global stable default. Bootstrap the pinned Zed checkout before running Cargo; later setup runs verify the source and reviewed local patches. GPUI and the editor share that checkout. Embedded Metal shaders compile through the system Metal runtime; the separate Xcode Metal compiler component is not required.

```sh
python3 tools/setup_zed_engine.py --fetch
cargo check --locked --bin agentwatcher
cargo run --locked -- --project /absolute/path/to/project
cargo test --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
```

Formatting and linting require the `rustfmt` and `clippy` components for Rust 1.98.1. All 70 preparation, setup, and source-packaging Python tests passed together: 19, 19, and 32 tests respectively. The new macOS signing regression and the 32 source-packaging tests passed after the signing fix. The final root suite (`cargo test --offline --locked --bin agentwatcher --lib -j1 -- --test-threads=1`) passed 319 tests, with five ignored. Eight additional upstream policy tests require the upstream development workspace; their offline run is blocked by an uncached pinned `calloop` Git dependency. Root tests and native interaction checks do not substitute for those tests.

Create an ad hoc signed app and zip:

```sh
bash build-rust.sh
```

The script writes to a new dated folder under `dist/` and refuses to overwrite an existing output folder. It pairs the app zip with a matching source archive containing the pinned Zed source, local patches, build inputs, and GPL notices, and reports artifact SHA-256 hashes. `AW_OUTPUT_DIR` chooses a different new destination; `CARGO_TARGET_DIR` chooses the compiler output location. `AW_PROFILE=dev` uses the optimized development profile for a quicker local build. Neither build route compiles Swift.

On this Mac, Documents FileProvider reapplies disallowed Finder metadata to app bundles, causing strict signature verification to fail. The verified v5 runnable app is therefore retained outside Documents in the session's review-deliverable directory; its ZIP, matching build-source snapshot, and source-input record are in `dist/rust-sol-speed-review-0.2.0-v5/`. Use an output location outside that FileProvider for a runnable bundle. Existing apps and source metadata were preserved. This is an ad hoc signed local review build, not a notarized or published release.

A read-only performance diagnostic reports timings and counts without printing recorded messages:

```sh
cargo run --locked -- --diagnose --project /absolute/path/to/project
```

## Keyboard

| Shortcut | Action |
| --- | --- |
| ⇧⌘P or ⌘K | Command palette |
| ⌘1–⌘9 | Switch views (⌘1 Dashboard, ⌘8 Graph, ⌘9 Agents & Skills) |
| ⌘O | Open workspace |
| ⌘R | Refresh |
| ⌘B | Toggle navigation |
| ⌥⌘I | Toggle Changes/Details dock |
| ⇧⌘E | Toggle workspace files |
| ⌘` | Toggle local Console |
| ⇧⌘L | Ask about the current pane |
| ⇧⌘O / ⌘S | Open local text file / save current file |
| ⌘[ / ⌘] | Back / Forward |
| ⌘F | Find in the focused text viewer; otherwise search recorded work / changed files |
| ⌘G / ⇧⌘G, F3 / ⇧F3 | Next / previous text match |
| ⌘+ / ⌘− / ⌘0 (also Control) | Increase / decrease / reset the focused viewer font |
| ⌥← / ⌥→ (with ⇧ to select) | Move by word |
| Page Up / Page Down (with ⇧ to select) | Page through the focused text viewer |
| ⌘A / ⌘C | Select all in the current text page / copy the exact selection |

Drag the pane separators to resize; double-click to reset. In the command palette, type to filter, use arrows and Return to run a command, and Escape to dismiss.

Settings is a searchable preference browser with 61 controls. Preferences include viewer fonts and zoom steps, motion, graph visibility and flow speed, refresh intervals, trace density and previews, privacy display choices, sidebar limits, and explicit launch defaults. Monitor only blocks new launches and continuations while leaving local inspection available.

Click a current AGENTS.md or use **Open in editor** in the catalog to open it in workspace file tabs while retaining the right agent inspector. The adapter delegates multiline input, Unicode/IME, selection, cut/copy/paste, undo/redo, layout, and scrolling to the actual upstream Zed engine. **⌘S** remains an explicit save through AgentWatcher's document worker; **⇧⌘O** opens another local file. Recorded versions have read-only capability on both Buffer and Editor. Navigation retains unsaved tabs; closing or quitting asks about unsaved changes. Native open, wrap/scroll, Unicode paste, undo/redo, CRLF save/recovery, and quit-cancel checks passed using an isolated synthetic file; full IME, accessibility, and clipboard-mode coverage remains pending. No Zed Project, language server, or cloud service is initialized, and single-line TextInput fields have not yet migrated to the engine.

Editable files are limited to 1 MiB and 4096 Unicode scalar values per line. Larger sources and longer lines use bounded read-only pages, with **Prev**, **Next**, full-source background Find, and exact **Copy full**; a partially loaded source cannot be edited. Uniform LF, CRLF, and bare-CR conventions round-trip through the adapter; mixed line endings remain read-only. Up to eight tabs and 128 undo transactions are retained. A cumulative 8 MiB edit-work budget bounds retained engine work and rejects further edits at its limit; this is not an 8 MiB heap-memory cap. Saves retain the existing single document-worker authority: verify the loaded source, atomically exchange the file, preserve macOS metadata, and retain the displaced inode in `~/Library/Application Support/AgentWatcher/Recovery/`. **Previous version** reveals it. Recovery retains up to 256 versions / 64 MiB and blocks further saves at the limit rather than deleting user versions. Safe saving currently requires the recovery directory and source on the same volume. No source file is changed until Save.

Viewer status strips expose position, type, search, exact copying, paging when needed, and font zoom. Embedded text wraps to the editor width for vertical reading. Visual and Tree evidence use one inherited content font size throughout their fields, labels, types, and values.

Console uses `/bin/zsh -f +m`, a minimal environment, and a plain-text PTY rather than a complete VT terminal emulator. It retains at most 256 KiB, stops after 30 minutes or 8 MiB of output work, and bounds input and its queue. Quit stops its owned process group. Commands have full local shell access and may perform external or billable actions; deliberately daemonized commands can escape process-group containment. Collaboration, voice, language servers, and debugger integration are not implemented by this local-tools increment.

Real changed files have an Open with menu for installed editors, Finder, Ghostty, and Terminal. Terminal apps open the containing folder; deleted or out-of-workspace paths report an error.

Interaction references: Zed for typography, viewer shortcuts, and workspace controls; [Magpie](https://github.com/yetone/magpie/tree/331c27703a7236fda9ff4981833f28500003edab) for visual-to-evidence navigation and compact provider/model context. AgentWatcher's graph and pane composition remain its own Rust application code around the embedded Zed text engine; no Magpie code or assets are bundled.

## Local data and limits

Rust settings use a separate `agentwatcher-rust.json` file in `~/Library/Application Support/AgentWatcher/`. First launch can read individual workspace keys from the prior app; it does not change the prior app's preferences. Reading recordings does not call a model or modify agent configuration. Credential-like values are redacted in displayed telemetry, process commands, logs, and patches.

The quick Codex catalog covers recent recordings; Agent history progressively scans older date folders without that date cutoff. Each trace window retains up to 16 MiB / 20,000 events; Earlier and Newer navigate available recording windows. DeepSeek compression uses an existing system or Homebrew zstd decoder, with an eight-second deadline and 1 GiB decoded-work limit. Larger or unavailable recordings report their limit. OpenCode requires an installed compatible CLI and does not support local byte-window paging.

Local monitoring, graphs, animations, history, and catalog browsing make **zero model requests and consume zero model tokens**. Explicit agent execution uses the provider and consumes its normal allowance or credits. Selecting skills adds only their names and source paths to the submitted task; their full instruction bodies are not preloaded by AgentWatcher. Up to eight skills may be selected. Missing run-time tool dependencies must be reported by the agent; catalog presence alone does not install or enable a tool.

Pane chat is a separate, explicitly submitted API feature. Supported formats are OpenAI Responses, OpenAI-compatible Chat Completions, Anthropic Messages, Gemini generateContent, and Ollama Chat. Custom endpoints require HTTPS; HTTP is allowed only on exact localhost, 127.0.0.1, or [::1]. Credentials, queries, fragments, redirects, and proxy inheritance are rejected. The selected destination receives only an explicit request; there is no automatic model discovery. Ollama can run without a key. Providers with other authentication or API formats need an adapter; a key alone does not make incompatible APIs interchangeable.

Each request sends at most 8 KiB of pane context, 16 KiB of retained conversation, and a 2 KiB question. It requests at most 512 output tokens, one request at a time, with a 60-second deadline and no automatic retry. This is not an invoice ceiling. Provider usage and incomplete responses are shown when supplied; questions, answers, drafts, and API keys are not persisted by AgentWatcher. Up to 16 pane conversations are retained in memory; New clears the current conversation and releases its slot. An unexpectedly large reply is visibly shortened rather than disappearing. Failed questions remain available alongside a newer draft. Closing chat does not cancel an active request; reopening it restores Stop. No real API request is needed for local monitoring or UI testing.

The chat workflow takes inspiration from Zed's [Agent Panel](https://zed.dev/docs/ai/agent-panel) and [provider configuration](https://zed.dev/docs/ai/use-api-access): quiet conversation controls, explicit context, a pinned composer, and clear provider/model identity. This increment does not include streaming, provider model discovery, saved keys, chat tools, or full Zed Markdown rendering. Keys are memory-only rather than saved to Keychain.

The catalog reads known instruction roots with bounded bytes, entries, depth, and time. It does not inspect authentication stores or execute skill scripts. Discovery can be incomplete when a limit is reached; warnings are visible. Installed files are not evidence that a particular agent read them.

Graph launches currently support the installed Codex 0.162.1 CLI with ChatGPT authentication. Each explicit launch accepts one task, requires a per-run acknowledgement of possible paid-credit usage, has a 1–10 minute deadline, allows one owned run at a time, and stops at bounded output. These bounds are not a billing cap. No agents restart or spawn subagents automatically. Unsupported project runtime configuration fails visibly. Graph launch links describe AgentWatcher origins, not provider-native subagents. Codex keeps its canonical session history; in-memory Graph launch metadata is scoped to this app run. Normal app quit stops and reaps its owned run before exiting.

Prior Codex and OpenCode recordings can continue in Terminal using their recorded workspace and session ID. Stop is available only for a verified dedicated process; shared provider processes cannot be attributed safely to one agent. The installed DeepSeek desktop app exposes only an external open action, so its sessions continue and stop inside DeepSeek. Configured worker controls remain in Settings and Worker log. No agent starts automatically.

Provider-policy release gate: this review uses the user's installed official CLI for local tasks. No credentials are copied, shared, or scraped, and no provider limits are bypassed. Account restrictions cannot be guaranteed away. Before commercial or hosted distribution, implement the provider-approved sign-in and integration route and re-check the applicable rules; OpenAI explicitly disallows reusing App Server authentication for commercial/hosted services. [OpenAI integration rules](https://learn.chatgpt.com/docs/app-server), [Codex skill invocation](https://learn.chatgpt.com/docs/build-skills).

CI and tagged release workflows now build the Rust app. Local compilation, local UI verification, hosted CI, notarization, and publication are separate checks; a local app is not a notarized public release.

GPUI retains its Apache-2.0 attribution; the embedded upstream Zed editor and related crates include GPL-3.0-or-later components. Their notices and license texts are recorded in `rust/NOTICE` and `rust/LICENSES/`, and packaging pairs the binary with matching source. No project-wide license has been chosen for the remainder of AgentWatcher; distribution readiness and applicable terms remain part of the release acceptance work.
