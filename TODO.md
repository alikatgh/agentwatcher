# AgentWatcher TODO

## Astra speed and UI increment — 2026-10-16

- [x] Move settings writes off the GPUI thread with one ordered writer, latest
  pending value, explicit retry, visible failures, and coordinated quit.
  Native termination defers until settings and owned work settle; cancelling
  the existing document confirmation cancels that native request.
- [x] Cache graph activity, lifecycle, and family state across camera frames.
  Keep the launch socket at 20 physical pixels with a 24-pixel target, and give
  the launch form its own bounded body scroll position.
- [x] Remove unused per-line viewer copies now that Zed owns text layout.
  A 4.5 MB / 100,000-line constructor fixture improved from 6.73 to 1.83 ms
  (3.68x), with zero extra constructor allocations.
- [x] Accelerate short-field credential redaction without changing patterns,
  rule order, or URL handling. A synthetic metadata-heavy JSON fixture improved
  1.80x; 10,407 old/new equivalence fixtures and independent review passed.
- [x] Distinguish required control outlines from quiet separators across light,
  dark, and named themes. Bound chat model/composer text and compact Settings.
- [x] Run focused graph, persistence, shutdown, content, theme, redaction, and
  source-packaging checks; correct independent review's native-quit findings.
- [x] Pass the final integrated production-plus-test-module check, including
  the corrected native quit flow, with bounded incremental compiler storage.
- [x] Link and package the v5 native executable with its matching source
  snapshot; verify strict signature, ZIP integrity, and binary hashes. Pass
  the actual root suite: 319 passed, five ignored.
- [x] Inspect native graph launch/skills and inspector scrolling, raw evidence,
  file tabs, wrapped reading, Unicode paste, undo/redo, exact CRLF save/recovery,
  narrow chat headers, Light/Dark/Tokyo Night, and Cmd-Q/dirty-document cancel.
  Fix dispatch-time Quit borrowing and narrow header overflow found in review.
- [ ] Complete Dock/OS termination, IME/accessibility, all theme/system states,
  and the remaining native interaction matrix recorded below.
- [ ] Measure packaged startup, typing, scrolling, graph frame times, and memory
  against the previous app. Component benchmarks do not establish app-wide
  speed or visual quality. Exact scope, hashes, and remaining checks are in
  [the verification record](docs/verification/2026-10-16-astra-speed-ui.json).

## Current themes, performance, and internal editing increment

- [x] Add all 24 article-demo themes plus Catppuccin Mocha, a searchable picker,
  shared cached semantic palettes, persistence validation, and palette commands.
- [x] Cull offscreen graph elements and wires, skip hidden activity targets, share
  status process collection, cache discovery sort keys, and reduce text copies.
- [x] Add the primary **Edit in AgentWatcher** action and background repository
  document loading through the existing safe editor and save/recovery flow.
- [x] Run core checks: 167 passed, 3 ignored; standalone theme checks: 9 passed;
  formatting and shell syntax passed; independent source review completed.
- [x] Build the complete native application and run its binary-module and
  library tests: 319 passed, five ignored. The v5 signed app launches with the
  actual Zed editor; exact artifacts and test logs are retained in
  `docs/verification/2026-10-16-native-final/`. Earlier ENOSPC and packaging
  failures remain historical evidence.
- [x] Render the theme picker and Light/Dark/Tokyo Night; exercise all-loaded
  graph fit/pan/zoom, internal editing, and narrow/wide chat surfaces.
- [ ] Finish all 25 rendered theme states and system appearance/relaunch behavior.
- [ ] Measure the final packaged app against the previous build. The earlier
  2026-10-15 redaction experiment showed no gain; the separately measured
  2026-10-16 short-field optimization above supersedes that candidate.
- [x] Move settings persistence off the GPUI thread with ordered saves and
  visible error reporting; native theme changes save and Cmd-Q exits correctly.

## Actual Zed Preview engine integration — native editor running

Implementation specification: [Integrating the Zed Preview engine](docs/zed-editor-integration.md).
Requested on 2026-10-15; implementation is authorized. Existing CodeView
surfaces now construct actual upstream Editor/Buffer entities by default, and
the production application builds and launches. The v5 root suite passes
319 tests, with five ignored; signed app/ZIP/source provenance is verified.
The broader Project/LSP, IME/accessibility, and surface-parity boundary below
is still open; embedding Editor does not implement every Zed IDE service.

- [x] Resolve the pinned Preview source and dependency graph at
  `aa9c16803dd841950e3d4f4c7ecd76f324e0ba44` (`v1.25.0-pre`), including selected
  root patches and upstream assets. Rust 1.98.1 is selected by the project's
  toolchain file; the global stable default remains unchanged. The app and
  engine use one path-based upstream GPUI source.
- [x] Add offline manifest preparation and prerequisite checks in
  `tools/zed_engine.py`, with 19 passing tests and independent review. The tool
  does not download, install, activate dependencies, or build the engine.
- [x] Add `tools/setup_zed_engine.py` for verified source bootstrap and reviewed
  patch application, with 19 passing tests. Initial setup is
  `python3 tools/setup_zed_engine.py --fetch`, followed by Cargo commands.
- [x] Add the immutable exact-source bridge in `rust/src/editor_source.rs`,
  with 13 passing standalone tests and independent review. It preserves exact
  evidence selections across Zed's CRLF/bare-CR normalization and is now wired
  to revision-checked clipboard interception and global page offsets.
- [x] Replace custom CodeView rendering/editing with actual upstream Editor and
  Buffer entities across existing file/snapshot, evidence, diff, log, context,
  and pane-chat surfaces. The document worker remains the sole explicit save
  and recovery authority; no Project, LSP, or cloud service is initialized.
- [x] Preserve pre-mutation byte/line limits and Buffer plus Editor read-only
  capability. Uniform LF/CRLF/bare-CR serialization round-trips; mixed endings
  and partially loaded sources remain read-only. Bound undo transactions at
  128 and cumulative edit work at 8 MiB; the latter is not a heap-memory cap.
- [x] Add bounded read-only Prev/Next source pages and full-source background
  Find, so long lines do not hide later source. Selection, Ask context, and
  copying retain exact global source ranges and original line endings.
- [x] Add paired source-archive packaging with pinned upstream source, reviewed
  patches, build inputs, hashes, and GPL notices. Packaging has 32 passing Python
  tests. All 70 preparation/setup/packaging tests passed together (19/19/32).
- [x] Complete independent source review and correct clipboard propagation,
  mixed-cursor Cut, rejected native replacement, transaction metadata retention,
  and long-line access findings. No confirmed source defect remains in that
  reviewed scope. The recorded native interaction checks also pass; the full
  acceptance matrix remains open.
- [x] Execute the seven adapter helper tests using unchanged source helpers and
  actual ExactSource definitions: seven passed. This does not exercise GPUI.
- [ ] Execute the eight upstream policy tests with real GPUI/Buffer/Editor
  contexts. The upstream development workspace run is blocked offline by
  uncached `calloop` revision `3759371fee14c40066d64777e2a36b6ffcc22590`.
  Root tests and native interaction do not replace those tests.

The outcome is the actual upstream engine underneath AgentWatcher's existing
surfaces and graph workflow. An optional new pane, external-app launcher, or
custom editor styled to resemble Zed does not complete this item.

- [x] **P0 — Finish the native build and artifact baseline.** Actual executable,
  source archive, input manifest, notices, strict signing, and startup verified.
  The v5 review build uses dev/opt1/debug0/incremental=false. Documents
  FileProvider metadata prevented strict signing there; the runnable app is
  retained in managed review storage and ZIP/source copies in `dist/`.
- [ ] **P1 — Verify the migrated GPUI runtime.** The single-source platform and
  Editor integration compile. Verify graph, panels, fields, themes, focus,
  scrolling, and packaged startup in the native application. See sections 4–5
  and phase 1. Graph, catalog, settings, pane resize, theme switching, and
  native startup have passed; complete the remaining route/focus matrix.
- [ ] **P1 — Verify source/snapshot documents through the engine.** The entities
  and safe-save bridge are implemented. Exercise Unicode/IME, undo, exact
  copy/search, uniform and mixed line endings, rejected edits, stale reloads,
  conflicts, recovery, and close/quit behavior on the linked application. See
  sections 5–7 and phase 2. Unicode paste, wrapping/scrolling, undo/redo,
  byte-exact CRLF save/recovery, clean quit, and dirty quit-cancel passed.
- [ ] **P1 — Complete the remaining surface migration.** Existing CodeView
  source/evidence, dynamic tabs, composer, streamed output, context, and patch
  surfaces use the engine. Audit structured evidence source affordances and
  migrate appropriate single-line TextInput fields, preserving secret-field
  behavior, validation, submit actions, identity, redaction, Ask/draft scope,
  and selection/follow behavior. See section 6 and phase 3.
- [ ] **P1 — Add bounded, app-owned Project/language services.** Bundle agreed
  grammar support, keep offline editing useful, preserve trust, configure local
  LSP explicitly, and establish one storage authority before enabling formatting,
  refactors or other file mutations. Prove watcher/process/retry/shutdown bounds
  and no incidental execution/downloads/cloud activation. See sections 7–8,
  11–12 and phase 4.
- [ ] **P1 — Complete the shared skin and interaction quality.** Map all 25
  themes, typography, icons, syntax/diff/search/selection states, keyboard
  precedence, IME, accessibility and pane/scroll lifecycles. Measure against the
  current app's fixed performance baseline and interact with every affected
  route/mode. See sections 9–11, 14 and phase 5.
- [ ] **P1 — Verify the integrated artifact and release readiness.** Run the
  relevant tests and native rendered matrix, obtain fresh independent review of
  shared/storage/service/release changes, verify packaged assets/source pin and
  licenses, and rehearse rollback with prior apps/user files preserved. Report
  build, UI verification and release states separately. See sections 12–15 and
  phase 6. Engine integration is complete only after the whole acceptance
  boundary in section 1 is satisfied.
