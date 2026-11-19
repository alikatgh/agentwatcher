# Integrating the Zed Preview engine into AgentWatcher

Status, **2026-10-16**: **the actual upstream editor now runs inside the packaged
native app. The v5 root suite passes 319 tests with five ignored; strict signing,
ZIP/source integrity, and the recorded native interactions pass. Project/LSP,
full IME/accessibility, upstream policy tests, and broad performance/parity
acceptance remain open.**

The root manifest and lockfile now use one GPUI source from Zed Preview
`aa9c16803dd841950e3d4f4c7ecd76f324e0ba44` (`v1.25.0-pre`). The checkout is
`rust/vendor/zed`; project Rust is 1.98.1, with the system's default Rust unchanged.
[`tools/setup_zed_engine.py`](../tools/setup_zed_engine.py) fetches and verifies
that pin and the explicitly recorded local patches. It preserves unexpected local
changes and rejects them rather than replacing them.

[`CodeView`](../rust/src/code_view.rs) now renders upstream `EditorElement` over
real `Editor`, `MultiBuffer`, and `Buffer` entities. Zed owns text mutation,
selection, input, layout, and undo across existing file tabs, evidence, changes,
history, context, console output, and chat composition. The former custom text
editor is removed from those runtime paths. The existing single-line `TextInput`
fields remain a separate migration item. There is no permanent optional Zed pane.

[`zed_editor_runtime.rs`](../rust/src/zed_editor_runtime.rs) initializes local
settings, theme and editor services; AgentWatcher's themes, fonts, and pane chrome
skin the embedded engine. No Project, LSP, extension host, cloud client, telemetry,
or AI service is initialized by this integration. Those capabilities still need
the service and storage work described below.

The immutable exact-source bridge now participates in revision-checked copy,
cut, Ask context, and explicit document serialization. Uniform LF/CRLF/bare-CR
conventions round-trip; mixed-ending files remain read only. Large read-only
sources use bounded pages with full-source background Find. Writable buffers
enforce 1 MiB / 4096-character line bounds before edits, an 8 MiB cumulative
edit-work budget, and 128 retained undo transactions. The edit-work budget is
not a measured heap cap. Safe saves and recovery still have one app-owned path.

The upstream policy patch and opt-in deferred native-quit patch are persisted
under `rust/zed-patches/` with six full-file hashes. The quit patch routes AppKit
termination through AgentWatcher's existing document, settings, and owned-work
gates; applications without the hook retain their upstream behavior. Independent
review found and verified fixes for clipboard
double dispatch, rejected transaction-history growth, atomic IME replacement,
read-only paging, and build/source integrity. Distribution tooling now produces
a matching source archive with pinned upstream files, the local patches, notices,
and build inputs; extracted archives can be verified without Git. The signed
v5 review app runs outside Documents because this Mac's FileProvider reapplies
disallowed Finder metadata to app bundles there. ZIP, matching build-source
snapshot, and input record are retained in `dist/rust-sol-speed-review-0.2.0-v5/`.
No notarized or public release is claimed.

Verification: all 70 preparation/setup/packaging tests passed again on October 16;
the exact-source bridge's 13 standalone tests passed earlier. Eleven extracted
native-quit state tests pass, including cancellation, reentry, duplicate requests,
and pending editor completion. They do not exercise AppKit's event loop. The
seven adapter helper tests also pass against unchanged extracted source and
actual ExactSource definitions. The final root library/binary suite passes
319 tests, with five ignored. The new macOS signing regression and 32 packaging
tests pass. Eight additional upstream policy tests remain blocked offline by
uncached `calloop` revision `3759371fee14c40066d64777e2a36b6ffcc22590` in the
upstream development workspace; the root suite does not execute them.

Native checks exercised Graph file opening without overlapping headers, wrapped
vertical reading, Unicode paste, undo/redo, exact CRLF saving and original-version
recovery, independent evidence and inspector scroll, launch skill scrolling,
300/463-point chat controls, Light/Dark/Tokyo Night, graph fit/pan at 20–25%,
clean Cmd-Q, and dirty-document Keep editing followed by undo. A synchronous
global Quit handler had attempted to update its already-borrowed window; deferring
that update fixes dispatch and passed fresh independent review and native Cmd-Q.
No real agent or provider API request was started during these checks.
Historical embedding evidence is in
[`2026-10-15-zed-embedding.json`](verification/2026-10-15-zed-embedding.json);
current code hashes, component measurements, and remaining checks are in
[`2026-10-16-astra-speed-ui.json`](verification/2026-10-16-astra-speed-ui.json).
Exact artifact hashes and retained build/test logs are in
[`2026-10-16-native-final/artifacts.json`](verification/2026-10-16-native-final/artifacts.json).

## 1. Intended result and completion boundary

AgentWatcher should become another native skin over the actual Zed editor
engine. The agent graph, provider and workspace identity, hierarchy, activity,
controls, and progressive disclosure remain AgentWatcher's product. Zed supplies
the text engine underneath its existing text surfaces.

The integrated release must use upstream `editor::Editor`, `language::Buffer`,
and `multi_buffer::MultiBuffer` for source editing, selectable evidence,
diffs, logs, and the chat composer. Appropriate single-line inputs should share
the engine through its single-line mode. A new optional editor pane, opening the
external Zed application, copying its colors, or extending the present custom
editor does not meet this objective.

The final release must establish all of the following:

- Existing source tabs and recorded snapshots use the engine by default.
- Existing evidence, context, change, history, console-output, and chat surfaces
  use the same engine without losing their individual identity or permissions.
- Real file buffers have language detection, syntax, and the agreed Project/LSP
  capabilities; read-only historical material remains a separate buffer class.
- Fonts, all named themes, focus, selection, search, shortcuts, pane geometry,
  and scrolling feel like one application.
- Existing explicit-save, conflict, recovery, byte-bound, redaction, and
  pane-scoped chat contracts remain effective.
- Graph navigation, agent inspection, and typing remain responsive while
  projects, history, and language services load.
- Source provenance, dependency versions, licenses, assets, packaging, failure
  behavior, and rollback are documented and verified against the built artifact.

A minimal real-engine spike is an intermediate milestone. It is not completion
of the whole integration.

## 2. Verified baseline and prerequisites

The following records the **original pre-integration research baseline** on
2026-10-15. The implementation status above supersedes its local dependency,
toolchain, and allocation observations; retain it as historical evidence.

| Item | Observed evidence | Implication |
| --- | --- | --- |
| Installed reference | `/Applications/Zed Preview.app/Contents/Info.plist`: `CFBundleShortVersionString=1.25.0`, `CFBundleVersion=20261014.162249` | Use the matching Preview source rather than a moving `main` branch. |
| Matching upstream release | Immutable [`v1.25.0-pre`](https://github.com/zed-industries/zed/releases/tag/v1.25.0-pre), published 2026-10-14; GitHub tag API resolves to `aa9c16803dd841950e3d4f4c7ecd76f324e0ba44` | Pin this full revision throughout the initial integration. Installed binary revision equivalence was not independently extracted from the executable. |
| Pre-integration AgentWatcher GPUI | Root `Cargo.toml`: crates.io `gpui = "=0.2.2"`, `font-kit` and `runtime_shaders`, default features disabled | Published GPUI and upstream GPUI are different Cargo sources even when their package versions match. |
| Pinned upstream GPUI | Package version `0.2.2`; native platform code lives in separate platform crates | Migrate the complete app to one pinned GPUI source. Do not combine incompatible `Entity`, `Window`, or `App` types. |
| Toolchain | `/Users/s_avelova/.cargo/bin/rustc --version`: `1.94.0`; upstream `rust-toolchain.toml`: `1.98.1` | Use upstream's declared toolchain first. Older-toolchain compatibility has not been tested. |
| Shell PATH | Plain `rustc` was unavailable; the absolute Cargo-bin path worked | Check tool paths explicitly rather than diagnosing a missing toolchain from PATH alone. |
| Disk headroom | `df -h . /private/tmp`: approximately **1.1 GiB available**, Data volume shown at 100% capacity | Do not begin a new upstream checkout, toolchain install, or build with this observed headroom. Measure an appropriate build volume first. |
| Workspace state | Existing modified files and substantial untracked Rust/Swift work | Preserve the entire working tree. Agree exact implementation ownership before editing. |
| License records | Inspected upstream editor/project/workspace/language manifests declare `GPL-3.0-or-later`; GPUI/platform manifests declare `Apache-2.0` | Update the actual distribution's license/provenance record before shipping. Current GPUI-only notices are insufficient for the expanded engine graph. |

No build-size, build-time, memory-use, or minimum-free-space number was measured
for the upstream engine. Those remain unknown. Set a workspace-specific storage
budget after measuring a disposable build on a suitable volume; do not present
an invented size estimate as an established requirement. Never free space by
removing, moving, or hiding existing user files or old builds.

Useful prerequisite commands, **for a later implementation session**:

```sh
git status --short
/Users/s_avelova/.cargo/bin/rustc --version
/Users/s_avelova/.cargo/bin/rustup toolchain list
df -h . /private/tmp
plutil -p '/Applications/Zed Preview.app/Contents/Info.plist'
```

Apply the `session-storage` workflow before allocating checkout, dependency,
toolchain, target, or packaged-output paths. Record their exact ownership,
baseline sizes, retention conditions, and after sizes. An isolated worktree
must contain the relevant uncommitted Rust baseline before it is usable; a
worktree from the current committed HEAD alone will not include untracked work.
Select Rust 1.98.1 explicitly in AgentWatcher's future `rust-toolchain.toml` or
with `cargo +1.98.1`; a git dependency's toolchain file does not select the
parent application's compiler. Do not change global Rust defaults for this task.

## 3. Upstream dependency architecture

The [`editor` manifest at the pin](https://github.com/zed-industries/zed/blob/aa9c16803dd841950e3d4f4c7ecd76f324e0ba44/crates/editor/Cargo.toml)
has a `test-support` feature but no lean standalone-editor default feature.
`default-features = false` does not remove its mandatory Project, Workspace,
client, RPC, DB, Git, UI, Markdown, language, and theme dependencies. The crate
is not published to crates.io.

This is the meaningful source-level dependency shape, not a complete resolved
Cargo graph:

```text
AgentWatcher skin and agent model
  ├─ gpui + gpui_platform → gpui_macos → gpui_apple
  └─ editor
      ├─ language → language_core, text, rope, tree-sitter
      ├─ multi_buffer → language / text
      ├─ theme / theme_settings / ui / ui_input / settings
      ├─ workspace → project / session / client / DB / remote
      └─ project → worktree / Git / LSP / terminal / node_runtime /
                   prettier / extension / client / remote
```

`Project=None` removes the need to construct a live Project for an initial
buffer. It does **not** remove these mandatory dependencies from compilation.
The inspected normal/build macOS manifest closure contains **99 upstream
workspace crates**, including mandatory Tree-sitter Wasm/Wasmtime dependencies.
This is a source-level closure, not a resolved Cargo lockfile. Exact dependency
and patch evidence is recorded in the
[pinned dependency report](verification/2026-10-15-zed-dependencies.json).
Adding `languages` for built-in language initialization adds its own adapters,
grammar options, Node integration, and Python-environment dependencies. Inspect
the resolved graph before enabling optional capabilities. No complete
`cargo metadata` graph or compile feasibility check was run during this research.

Recommended dependency strategy:

1. Use one immutable upstream revision for all Zed crates.
2. Prefer upstream crates with a small AgentWatcher integration layer. Do not
   start by extracting thousands of editor lines into a new local editor fork.
3. Start with git dependencies if resolution works cleanly. If upstream build
   assets or local patches require a source checkout, use one registered,
   revision-pinned checkout and path dependencies, preserving upstream layout.
4. Keep any necessary upstream patch small, recorded, independently reviewed,
   and accompanied by a reason and a rebase test.
5. Resolve and inspect the macOS dependency graph before building. Do not build
   the complete Zed application or its servers merely to obtain editor types.

The following shows the dependency **shape**, not a complete tested manifest:

```toml
[dependencies]
gpui = { git = "https://github.com/zed-industries/zed", rev = "aa9c16803dd841950e3d4f4c7ecd76f324e0ba44", default-features = false }
gpui_platform = { git = "https://github.com/zed-industries/zed", rev = "aa9c16803dd841950e3d4f4c7ecd76f324e0ba44", default-features = false, features = ["font-kit", "runtime_shaders"] }
zed_editor = { package = "editor", git = "https://github.com/zed-industries/zed", rev = "aa9c16803dd841950e3d4f4c7ecd76f324e0ba44", default-features = false }
language = { git = "https://github.com/zed-industries/zed", rev = "aa9c16803dd841950e3d4f4c7ecd76f324e0ba44" }
```

Add `multi_buffer`, `settings`, `theme`, `theme_settings`, `release_channel`, and
then the actual Project/language-service dependencies as directly used.

Development builds need a direct aliased `util` dependency with its upstream
`debug-embed` feature. Otherwise upstream settings assets can resolve against
AgentWatcher's repository at runtime instead of the pinned Zed assets. This
feature already exists upstream; no source fork is required for that fix:

```toml
zed_util = { package = "util", git = "https://github.com/zed-industries/zed", rev = "aa9c16803dd841950e3d4f4c7ecd76f324e0ba44", default-features = false, features = ["debug-embed"] }
```

The selected macOS closure needs five root patches: `tree-sitter-language`,
`async-task`, `async-process`, `notify`, and `notify-types`. The local `scratch`
patch is a Corgi-specific behavior override and is not required by the ordinary
Cargo route. Keep the exact revisions in `rust/zed-engine.toml`; recheck this
selection against Cargo resolution before the application migration.

Cargo root patches need deliberate handling. An external dependency's
`[patch.crates-io]` table does not become AgentWatcher's root patch table.
The pinned [upstream workspace manifest](https://github.com/zed-industries/zed/blob/aa9c16803dd841950e3d4f4c7ecd76f324e0ba44/Cargo.toml)
contains exact Git forks for, among others, Tree-sitter's language ABI,
`async-task`, `async-process`, and `notify`; it also contains a local `scratch`
patch under `tooling/corgi/patches/scratch`. Determine which patches the selected
macOS graph actually needs, preserve their exact revisions, and supply any
reachable local patch from the same pinned source. Do not blindly copy a local
path that does not exist in AgentWatcher or substitute an unrelated crates.io
release. Check native build scripts, packaged assets, and submodule requirements
as part of dependency resolution; they were not fully audited here.

## 4. Proposed integration boundary

Introduce a narrow runtime/adapter boundary, provisionally named
`zed_runtime.rs` and `editor_surface.rs`. These names are proposals; no such
modules were added by this documentation change.

```text
Workspace: graph, agent identity, panels, application commands
  ├─ ZedRuntime: shared settings/theme/language/project service lifetime
  ├─ EditorSurface: stable source identity + policy + upstream Editor entity
  ├─ DocumentSession: original file version + explicit save/recovery state
  └─ existing workers: history, process, Git and filesystem collection
```

`EditorSurface` should be policy and presentation glue. Upstream owns text,
anchors, transactions, undo/redo, selection, layout, wrapping, cursor, and IME.
Do not maintain a second mutable String as the editing authority or run the
custom `EditHistory` alongside upstream undo. Immutable exact-source evidence
and explicit saved-version snapshots are legitimate separate data.

Use explicit surface policies:

| Surface policy | Backing model | Mutation / service behavior |
| --- | --- | --- |
| Local source document | One engine Buffer per document; Project-backed once safe storage integration is ready | Edits allowed within current bounds; saves use the single explicit storage authority; scoped LSP only. |
| Recorded evidence / snapshot | Buffer created from already-redacted immutable evidence | Read only; no file association, watchers, executable language services, or disk writes. |
| Git patch | Read-only Buffer or MultiBuffer excerpts with deliberate diff presentation | Copy preserves the supplied patch; applying/staging changes remains a separate explicit operation. |
| Streaming log / console output | Read-only bounded Buffer with incremental updates | Never interpreted as terminal input, a file, a command, or executable language configuration. |
| Pane chat composer | Small local Buffer; auto-height editor | Current byte cap and per-pane drafts; no Project/LSP or automatic transmission. |
| Single-line field | Single-line editor with field policy | Preserve validation, length caps, change events, submit behavior, and secret masking. |

Keep upstream service initialization separate from activation. Compiling client,
RPC, terminal, or remote crates is not permission to connect accounts, start
terminals, scan extra directories, execute hooks, or download tools.

## 5. GPUI migration and minimal genuine engine milestone

Migrate the entire UI to upstream GPUI before mixing in editor entities. Matching
package version numbers do not unify different Cargo sources. Verify the graph
has one effective GPUI source for the application and editor.

The pinned platform crate exposes `gpui_platform::application()`.
Replace the current `Application::new()` entry point in `rust/src/main.rs` and
move `runtime_shaders` to `gpui_platform`. This retains runtime Metal shader
compilation as the initial build strategy; its actual launch must still be
verified. Update imports and APIs from compiler evidence rather than assuming
the newer upstream GPUI is source-compatible with the published crate.

The upstream editor's tests initialize a settings store, theme settings,
release-channel metadata, then editor registrations. For a production spike,
the likely sequence is:

1. Create the native platform application with AgentWatcher's asset source.
2. Register the bundled IBM Plex Sans and JetBrains Mono fonts.
3. Call `settings::init(cx)` for the upstream default settings store.
4. Initialize `theme_settings` and install the AgentWatcher theme mapping.
   `LoadThemes::JustBase` is useful for the spike; it does not supply all named
   AgentWatcher themes.
5. Initialize required release-channel metadata and `zed_editor::init(cx)`.
6. Load validated editor keybindings and AgentWatcher-specific commands.
7. Construct the existing Workspace and its engine-backed surfaces.

This sequence is source-supported but has **not** been compiled or proved
sufficient for every production action. Check action paths that reach global
Workspace/AppState services, dialogs, persistence, menus, and assets. Disable
unavailable commands explicitly; do not allow them to panic on missing globals.

Confirmed constructors and operations at the pin include `Buffer::local`,
`Editor::for_buffer` with an optional Project, `Editor::for_multibuffer`,
single-line and auto-height constructors, `Editor::set_read_only`,
`Editor::set_text`, `Editor::text`, and `Editor::set_masked`. The constructor
requires a `&mut Window` as well as its entity context. This differs from today's
`CodeView::new(cx)` signature.

Pass Window through creation paths deliberately. `Workspace::new` already has
one. `ChatState::new`, `ToolsState::new`, and asynchronous file-result handling
will need signature/lifecycle changes or carefully staged creation inside the
window context. Avoid repeatedly creating Editor entities during rendering.

The first milestone must open a representative file in an **existing** source
tab using a real upstream Buffer/Editor, allow Unicode/IME input, undo, selection,
copy, search, theme changes, and safe explicit save. It should use local buffers
without an active Project initially, while retaining the existing storage
worker. An isolated compile-only example is useful evidence but does not satisfy
this milestone's visible behavior.

## 6. Current call-site migration inventory

This inventory is from the working tree on 2026-10-15. Line numbers will move
during concurrent theme/performance work; field and method names are the stable
search keys. There are **11 persistent CodeView fields plus dynamic file and
snapshot tabs**, whose current shared tab limit is eight.

| Current location / identity | Current purpose | Required migration |
| --- | --- | --- |
| `main.rs` — `working_view` | Working-tree changes | Read-only patch engine surface; keep exact supplied diff and path identity. |
| `main.rs` — `commit_view` | Selected increment/commit diff | Separate buffer/session identity; never inherit another commit's scroll or selection accidentally. |
| `main.rs` — `dock_view` | Docked diff | Independent editor view; share document data only where identity actually matches. |
| `main.rs` — `detail_view` | Raw record/evidence fallback | Read-only redacted evidence; exact selection and copying. |
| `main.rs` — `log_view` | Worker log | Bounded streaming read-only editor; preserve selection and pause/follow behavior. |
| `main.rs` — `catalog_detail` | Agent/skill catalog details | Read-only evidence with existing open/edit/context actions preserved. |
| `main.rs` — `context_view` | Agent context / file context | Keep provider, workspace, agent, and context identity in the containing UI. |
| `workspace_editor.rs` — `EditorTab.view`, file creation in `receive_editor` | Editable local files | Engine-backed DocumentSession; explicit save, stale reload rejection, close/quit confirmation, recovery. |
| `workspace_editor.rs` — `open_workspace_snapshot` | Recorded snapshot tabs | Read only; separate from live file buffers even if names match. |
| `chat_panel.rs` — `question` | Editable multiline prompt | Auto-height engine surface; current 2 KiB cap, draft isolation, input acknowledgment and explicit send. |
| `chat_panel.rs` — `preview` | Exact outbound context preview | Read only; no recursive Ask action or automatic send. |
| `chat_panel.rs` — `transcript` | Pane-scoped conversation history | Read only; separate history identities and no recursive Ask action. |
| `tools_panel.rs` — `console_view` | Plain-text PTY output | Read only; preserve pause on selection and explicit resume/follow. Do not replace the terminal process lifecycle as an incidental editor change. |

Additional consumers that must migrate with the adapter:

- `main.rs::code_view` selects and updates the reused evidence/diff/log views;
  `apply_preferences`, viewer zoom actions, and Ask subscriptions affect them.
- `graph_inspector.rs` displays agent context and updates the shared
  `context_view`; its context identity must survive graph selection changes.
- `catalog_page.rs` sets and renders `catalog_detail` for installed catalog
  entries and file-related inspection.
- `workspace_editor.rs` reads `source()` for dirty detection, save snapshots,
  and chat context; observes edit revisions to reject stale reloads; freezes
  editing during quit; updates fonts/zoom; owns close/save/reload confirmations.
- `tools_panel.rs::poll` checks active selection before following new output,
  changes source by display revision, and scrolls only when appropriate.
- `chat_panel.rs` reads the composer, observes edits, isolates scopes, applies
  redaction, and updates preview/transcript. Each of those contracts needs an
  explicit upstream mapping.
- `code_view.rs` currently owns input handling, edit history, selection,
  navigation, status, literal find, diff rows, fonts, Ask, and scrolling. Replace
  its engine duties; retain only justified AgentWatcher policy/chrome.
- `content.rs::CodeContent` now transports exact immutable source bytes and
  the long-line editing flag. Zed owns line layout and previews; the obsolete
  duplicate per-line representation has been removed. Never reconstruct
  original evidence by joining display lines.
- `style.rs::source_line` is the current lexical highlighter. Engine surfaces
  should use language-aware upstream highlighting. Any remaining non-editor use
  needs a deliberate owner, not an unnoticed second code-rendering path.

`EventView` in `event_view.rs` is a **separate structured evidence renderer**, not
one of the CodeView entities. Preserve its useful diagrams/structured sections,
redaction and copy contracts. Where it exposes source/text evidence, reuse the
engine surface instead of introducing another selectable-code implementation.
Graph/card labels and ordinary explanatory text do not need to become editors.

Single-line input inventory for later parity work:

| Location | Fields / controls |
| --- | --- |
| `main.rs` | Recorded-work search, command palette search, project folder, worker script, state folder, skill search, task, optional title, workspace folder. |
| `chat_panel.rs` | API key, model, endpoint. The key is secret and currently memory-only. |
| `tools_panel.rs` | Explicit shell command entry, currently capped at 8,191 bytes. |
| `code_view.rs` | Literal find field, owned by each old CodeView. |
| `input.rs` | Shared TextInput implementation, validation/input behavior, popup/autocomplete-related helper. |

Use upstream single-line mode where it preserves field behavior. Secret fields
require a separately verified masking, clipboard, accessibility, persistence,
and logging policy; a visually masked Editor alone does not establish secrecy.

Refresh this inventory during implementation:

```sh
rg -n 'CodeView::new|Entity<CodeView>|TextInput::new|Entity<TextInput>' rust/src
rg -n 'set_content_for|selected_text|selection_in_progress|CodeEdited|AskContext' rust/src
rg -n 'source_line|EntityInputHandler|EditHistory' rust/src
```

## 7. File ownership, save, recovery, and conflicts

Choose one authoritative storage path before enabling any Project-backed writes.
The proposed first path retains `documents.rs` and its existing background
worker. The engine owns the draft; DocumentSession owns the baseline file
version and recovery metadata. A save request captures an immutable engine
snapshot plus document identity and revision, then performs file I/O away from
the GPUI thread.

Current safeguards to preserve, as inspected in `documents.rs` and
`workspace_editor.rs`:

- Regular local UTF-8 text only, up to 1 MiB, excluding NUL-containing data.
- Version comparison includes source content, device and inode; changed or
  replaced files cause a conflict and retain the draft.
- Save rejects linked/replaced, unsupported, or read-only files and unsuitable
  recovery storage; it does not silently choose a less safe write path.
- macOS atomic exchange, file metadata preservation, fsync and recovery handling
  protect the prior version, including concurrent-writer cases covered by tests.
- Recovery is bounded at 256 versions / 64 MiB; reaching the bound preserves the
  draft and produces an actionable failure.
- Reload results are revision-checked; edits made during a reload survive.
- Save completion uses the submitted text, so later edits remain dirty.
- Close and quit confirmations remain tied to the actual current drafts.

The current 4,096-character per-line preview restriction also makes some local
tabs read only. Retain that behavior until the new engine's long-line latency,
copy fidelity, byte bounds and save behavior have been measured; any relaxation
is a separate, explicit acceptance change.

The pinned upstream text engine normalizes both **CRLF and bare CR to LF** on
initialization and insertion; BOMs and Unicode U+2028/U+2029 are unchanged.
See [`LineEnding::normalize`](https://github.com/zed-industries/zed/blob/aa9c16803dd841950e3d4f4c7ecd76f324e0ba44/crates/text/src/text.rs#L3663-L3682).
The implemented `ExactSource` bridge stores one normalized end offset per CRLF.
Mapping a valid normalized UTF-8 boundary adds the count of preceding collapsed
CRLF bytes using binary search. A selected LF can consequently recover the
original LF, CR, or CRLF. Invalid, reversed, and non-boundary ranges are rejected.
Unchanged LF text aliases the original allocation.

This is an immutable source-version bridge, not a draft serializer. Before using
its selection map, the adapter must prove the engine revision still matches the
bridge's normalized text; an edit invalidates the old map. Intercept evidence
copy/export so upstream's normalized clipboard text cannot replace original
bytes. Test this against the actual pinned Buffer and clipboard, including IME,
combining characters, astral Unicode, empty files and final newlines. Define and
test the local-file serialization policy separately so an open/save cycle cannot
silently normalize a file.

Three upstream policies need implementation evidence before replacing the
current editor. First, no pre-mutation byte/line validation hook was established
by the API investigation; preserve the existing bounds with a narrowly scoped
Buffer mutation-policy hook if necessary. Applying an edit and then undoing it
does not bound the transient allocation. Second, historical buffers need both
Editor read-only state and `Buffer::set_capability(ReadOnly, cx)`; an editor-only
toggle is insufficient. Programmatic mutations must stay adapter-controlled.
Third, no bounded undo-history pruning API was established. Resolve this in the
engine rather than keeping a second mutable custom editor/history authority.
Streaming replacements also need a tested retention policy so output growth
does not accumulate unbounded history.

When adding Project, select and test one storage integration:

1. **Preferred first implementation:** Project provides language/worktree
   context, while an explicit bridge routes every supported save through the
   existing DocumentSession/storage worker. Intercept or disable upstream save,
   save-as, autosave, formatting-on-save, WorkspaceEdit, and file actions until
   they participate in the same transaction and recovery rules.
2. If this cannot preserve coherent Project dirty state and file versions,
   implement a narrowly scoped upstream-compatible file/storage adapter or
   move DocumentSession's safeguards into one Project storage authority.
   Prove the old conflict/recovery suite still passes before removing anything.

Do not keep a writable local-buffer copy and a separate writable Project buffer
for the same file. Do not assume intercepting Cmd-S covers autosave, refactors,
formatting, rename, or code actions. Multi-file language actions must be previewed
and committed through a tested transaction or remain disabled. A partial
multi-file failure must not be reported as a successful refactor.

## 8. Project, languages, grammar, and LSP services

The verified upstream `Project::local` constructor needs a client, NodeRuntime,
UserStore, LanguageRegistry, filesystem, optional environment, flags, and app
context. Worktrees can be found/created asynchronously and `open_buffer` returns
a task. This is a service integration, not just a source tab constructor.

Use an app-owned service container with a bounded Project per actively opened
workspace, referenced only while needed. Resolve canonical document/workspace
identity before sharing buffers. Historical events from another agent or
workspace must not attach to the currently open live Project by filename alone.

Language plan:

1. Register bundled deterministic language configurations and grammar queries
   for the agreed initial set: Rust, Markdown, JSON, TOML, shell, Python,
   JavaScript/TypeScript/TSX, HTML, and CSS. This is a proposed initial set; verify
   native/extension availability and licenses for each at the pinned revision.
2. Use a shared LanguageRegistry and asynchronously load the detected language.
   Assign it to the Buffer; cancel stale detections when document identity changes.
3. The pinned `languages::init` can register built-in adapters; the
   `load-grammars` feature enables native grammar registration. It also brings
   download-capable adapters. Decide whether to use the full initializer or a
   selected registry setup from source; measure dependencies and offline behavior.
4. Start with no automatic executable downloads or language server launches.
   Syntax, selection, editing, copy and local save must work offline.
5. Add explicitly configured, locally installed language servers for trusted
   live workspaces. Define process count, startup/request/idle timeouts,
   concurrency, restart/backoff bounds, output retention and shutdown ownership.
6. Verify definition/reference/rename/completion/diagnostics/formatting against
   real files. Expose only implemented safe commands in the command palette.

Respect upstream worktree trust. `LocalProjectFlags::default()` enables trust
initialization and global-config watching. Configure these deliberately; do not
disable trust merely to make a test pass. Avoid silently importing the user's
Zed settings or watching/modifying their Zed configuration. App-specific config,
DB, cache, logs, extensions and service downloads belong to explicit
AgentWatcher-owned paths, whose initialization must be checked before activation.

Do not automatically execute repository tasks, formatters, hooks, shell
environment scripts, extensions, MCP servers, or project-defined language server
commands when a graph node, recorded event, or snapshot is selected. Current
process discovery and historical inspection must remain bounded, passive work.

## 9. Theme, typography, icons, and assets

Keep AgentWatcher's theme identifiers and current settings authoritative.
`themes.rs`, `settings_panel.rs`, `backend.rs`, bundled theme resources and
`style.rs` already define appearance and font choices. Generate a corresponding
upstream theme/settings representation from that same resolved palette rather
than maintaining two independently selected themes.

Map every relevant role: background and foreground, selected text, inactive
selection, cursor, current line, line numbers, gutter, whitespace, indent guides,
find matches, syntax categories, diagnostics, diffs, scrollbars, menus, popovers,
focus, hover and disabled states. Upstream syntax styling has richer categories
than today's keyword/string/number/type colors; choose deliberate defaults for
all categories and test real language examples.

The theme/font bridge should update stable entities only when resolved settings
change. Do not recreate a Buffer or Editor to change appearance. Preserve
selection, undo, draft, scroll anchors and focus across all 25 named themes and
system/light/dark behavior. Reuse the current theme work's tested tokens and
contrast decisions; do not replace them while adding the engine.

Use bundled IBM Plex Sans for UI and the current JetBrains Mono font for code.
The actual registered code family is `JetBrainsMono Nerd Font Mono`; verify font
selection and fallback on the built app rather than assuming a display name.
Carry upstream font licenses, theme attribution, and all newly required icon
licenses into the packaged app. AgentWatcher's current AssetSource supplies a
small named icon set; upstream editor menus/gutters may request additional
assets. Audit those paths and provide licensed assets or deliberate supported
renderers. Missing icons must not be discovered only after release.

## 10. Keyboard, IME, accessibility, and interaction

Install editor keybindings intentionally after action registration. Upstream
settings includes a macOS keymap asset and parsing/loading helpers. Filter or
override actions that assume the full Zed Workspace before binding them into
AgentWatcher. Do not bind the whole Zed keymap and leave unsupported commands
reachable.

Define precedence with current Workspace commands: Cmd-Shift-P and Cmd-K/command palette,
Cmd-O/open, Cmd-S/save, Cmd-F/find, Cmd-G/find next, Cmd-A/C/X/V, undo/redo,
zoom, Escape, tab switching, pane focus and chat submit. Arrow keys must navigate
the editor while focused; graph/palette navigation must work when those surfaces
own focus. Tab insertion versus focus navigation must be explicit by mode.

Check on the rendered native app:

- Drag selection across lines and outside the viewport; autoscroll; word/line
  selection; multiple selections if exposed; copy exact text after scrolling.
- Japanese/Chinese composition, dead keys, composed accents, emoji, replacement
  ranges and candidate-window placement; undo a committed composition as expected.
- Cmd shortcuts on alternative keyboard layouts and modifier changes during a
  drag; contextual menus; forward delete and word/line movement.
- VoiceOver labels, values, selection/read-only state and actionable names;
  keyboard-only traversal; adequate focus and selection contrast; system text
  scaling and high contrast where supported by GPUI.
- Secure-field behavior: no unmasked value in accessibility output, copied
  previews, persisted draft, crash/error log or unrelated pane context.

Accessibility support is an acceptance check, not inferred from GPUI or Zed
being native. Record any upstream limitation and block the affected parity claim.

## 11. Streaming, scroll, state, and performance

Preserve two identities: a document/buffer identity and a particular view identity.
A live file may share a Buffer across two views, but each view owns its selection,
scroll and focus. A historical snapshot is a different document even when its
label matches a file. Keep per-agent/provider/workspace/session keys through Ask
actions and context previews.

For streaming evidence and logs, append/update the existing read-only buffer by
content revision. Do not call full `set_text` on every polling tick, reparse
unchanged evidence, or reset scroll from a render method. Follow the bottom only
when the user was following it; selecting or inspecting older output pauses
following. Preserve surviving anchors when trimming bounded retention, and
indicate truncation rather than silently claiming complete history.

Keep filesystem, Git, process discovery, telemetry/history parsing and expensive
source preparation off the GPUI thread. Use existing latest-result/epoch guards
and bounded queues. Project worktree scanning, language detection and diagnostics
must not introduce unbounded UI callbacks or duplicate the current collectors.

Measure the existing app and migrated app using the same fixed fixtures and
hardware: cold/warm launch, open file, graph selection, typing/IME, huge line,
rapid theme changes, log burst, repository refresh, window resize, tab switches,
and idle CPU. Record frame/input latency, retained entities/tasks/processes,
memory, bytes processed and disk growth. Set numerical regression thresholds
from that baseline before the implementation is declared ready; none were
measured for the engine during this research.

Lifetimes must be explicit: unsubscribe observers, cancel pending requests,
release views/buffers, stop owned language-server processes, stop filesystem
watchers, and bound cached Projects when tabs/workspaces close. Do not terminate
agents or processes discovered by AgentWatcher as a side effect of closing an
editor Project. Never delete a real worktree directory to release an upstream
Worktree entity.

## 12. Privacy, tokens, networking, and failure isolation

The engine must work without a Zed account. The fact that its crates include a
client does not require activating Zed cloud, collaboration, telemetry, AI,
automatic updates, or remote-development services. Construct the required
services with app-owned configuration and verify actual network/process behavior.
Do not satisfy constructors by copying credentials from the installed Zed app.

Preserve current AgentWatcher BYOK scope: keys remain memory-only, outgoing
context is previewable and redacted, sending is explicit, and histories/drafts
stay associated with their pane. The editor integration does not authorize
additional API requests, automatic completion, or variable-charge exposure.
Apply release-safety checks to any capability that later activates such work.

Treat recorded tool output, patches, links and repository configuration as data
with separate trust. Selection/copy must not run anything. Links and commands
must preserve the existing explicit action boundary. Historical raw payloads
must not bypass redaction simply because upstream buffers can display them.

Contain failures:

| Failure | Required behavior |
| --- | --- |
| Missing grammar / language service | Preserve readable/editable local text; show a specific limited language status; no repeated download loop. |
| LSP crash / hang | Bounded retry and cancellation; draft and graph remain usable; restart is discoverable. |
| External file mutation | Keep draft and original conflict evidence; explicit reload/compare/resolve. |
| Save/recovery full or unavailable | Leave draft dirty; retain existing file; explain the concrete storage failure. |
| Stale load, reload or diagnostic result | Ignore by document/project identity and revision; never overwrite a newer draft. |
| Engine service unavailable during development | Clearly fail the engine milestone; retain the previous build for recovery without labeling a custom-editor fallback as completed integration. |
| App quit with dirty tabs or pending save | Preserve explicit confirmation and single-save ownership; do not mark the app safe to quit from an older revision. |
| Network disabled | Graph/history/local edit/copy/save continue; optional services report unavailable. |

## 13. Phased execution and acceptance evidence

Phase 0 source/dependency/toolchain preparation and production source integration
for phases 1–3 are implemented. The complete native app builds, packages, launches,
and passes its root tests and recorded rendered checks. Full IME/accessibility,
surface parity, upstream policy tests, and Project/language services remain
unmet exit conditions. Keep changes reviewable; phases define engineering
dependencies, not permission gates or a reason to stop an explicitly authorized
implementation halfway.

| Phase | Work and likely files | Required evidence / exit condition |
| --- | --- | --- |
| 0 — Baseline and feasibility | Preserve current tree; storage/toolchain/source plan; pin dependency/asset/license graph; record current rendered behaviors. Root manifest/lock/build scripts only after the baseline is safe. | Enough measured storage; exact upstream pin; graph resolves; affected patches identified; current app baseline recorded; no user-file changes as a setup side effect. |
| 1 — One GPUI runtime | Migrate dependencies and native app entry point; fix affected imports/APIs; package existing assets. | Whole existing app builds and launches on one effective GPUI source; navigation, graph, theme changes and existing fields still function. |
| 2 — Real engine and safe documents | Runtime initialization, editor adapter, DocumentSession, existing file/snapshot tabs and save/reload/quit wiring. | Engine identity established; edit/IME/undo/copy/search work; current document conflict/recovery tests plus adapter race tests pass; exact no-edit bytes preserved. |
| 3 — All current text surfaces | Migrate the inventory, composer, streaming output, raw evidence, structured evidence's text affordances and appropriate single-line inputs. | No legacy custom editor in those runtime paths; every route and mode visually interacted with; independent identities, Ask context, caps and selection/follow behavior verified. |
| 4 — Project and language capabilities | App-owned services, scoped worktrees, grammar registry, trusted local LSP, unified mutation/save authority. | Offline syntax/editing works; actual Project-backed files support the agreed language actions; no hidden writes/exec/downloads; watcher/process bounds and shutdown tests pass. |
| 5 — Unified skin and quality | Theme/font/icon mapping, keyboard precedence, accessibility, pane geometry, performance and lifecycle. | All 25 themes plus system behavior rendered; full interaction matrix and baseline comparison pass; security/privacy failures exercised; missing parity reported. |
| 6 — Packaging and release readiness | Notices/assets, exact binary provenance, release safety, fresh read-only review, package and recovery drill. | Reviewer findings resolved; integrated tests pass; packaged app launches and matches source revision; previous app remains accessible; release states reported separately. |

A development feature switch can isolate migration work, but the final product
must default to the engine in its existing surfaces. Do not ship a second
permanent editor path and call the broader integration complete.

## 14. Verification matrix

Run the smallest relevant check while implementing; run the integrated matrix
after the interacting pieces are assembled. Do not turn documentation completion
into a claim that these future checks passed.

| Area | Meaningful checks |
| --- | --- |
| Dependency/runtime | Lockfile pin and root patches; no mixed GPUI sources; native platform/shaders; action/globals/assets; installed reference/source provenance. |
| Document storage | Existing `documents` tests; stale save/reload; edit during save; disk mutation before/at/after commit; hardlink/symlink/replacement; read only; recovery cap/failure; metadata and exact newline/Unicode round trips. |
| Editing | Undo/redo transactions; paste/cut/select; IME; empty/large/long-line files; caps under paste/undo/LSP edits; read-only enforcement from both UI and action APIs. |
| Evidence | Redaction before buffer creation; exact-copy fixtures; agent/workspace/provider identity; snapshots never writable or linked to active files. |
| Chat/console | Draft scope switches; explicit send preview; key secrecy; byte caps; selection pauses stream; resume; retention trimming; no accidental shell input. |
| Project/LSP | Trusted/untrusted roots; no auto-run/download; path containment; diagnostics/completion/navigation; mutation transaction failures; bounded start/restart/cancel/shutdown; offline operation. |
| Theme/UI | Every named theme and system switch; real syntax/diff/search/selection; popovers and disabled states; typography fallback; focus/keyboard; narrow/wide and resized panes. |
| Performance/lifecycle | Fixed baseline fixtures; graph/history refresh during typing; log bursts; unchanged-content updates; tab/workspace churn; bounded tasks/processes/cache/disk. |
| Distribution | Packaged resources and licenses; exact binary/source/lock provenance; signing verification; launch/interact packaged artifact; older build retained; no unrequested cloud/billable activation. |

Existing commands, after dependency and environment prerequisites are met:

```sh
/Users/s_avelova/.cargo/bin/cargo fmt --check
/Users/s_avelova/.cargo/bin/cargo test --locked documents::tests
/Users/s_avelova/.cargo/bin/cargo test --locked
bash build-rust.sh
```

Add targeted adapter/Project tests to the actual test layout, then run them by
their real names. Use `cargo tree --locked` and target-specific `cargo metadata`
to inspect the resolved graph once dependencies exist. Updating `Cargo.lock`
requires an intentional dependency-resolution step before `--locked` checks can
pass. Run build/packaging only with session-owned output paths and sufficient
headroom. Build success alone does not prove rendering, IME, or accessibility.

For release readiness, use a fresh read-only reviewer for the shared editor,
storage/concurrency, service and release changes. Give the exact diff, user
acceptance criteria and commands; independently verify the integrated artifact
after fixing confirmed findings.

## 15. Rollback and upgrade policy

Keep previous signed/packaged app builds and source work accessible. Do not
overwrite the previous Swift app or edit Swift sources for this Rust migration.
Avoid changing persisted user formats until needed; any change must be versioned
and reversible. Do not share an upstream Zed database/cache namespace with the
installed application.

Before activation, rehearse switching back to the previous app without deleting
drafts, recovery versions, configuration, history or workspaces. A rollback
restores the previous executable/runtime configuration; it must not replace a
user project with an older copied tree. Account for open file descriptors and
pending saves rather than terminating a process blindly.

For later upstream upgrades, compare the same acceptance fixtures against the
current pinned engine. Review GPUI/API, keymap, settings, grammar, storage,
security and dependency-patch changes together. Advance one exact pin only after
the integrated artifact passes; record the source/tag, toolchain, lockfile,
patched files, licenses and known gaps. Preview updates do not authorize
unattended dependency upgrades or additional runtime services.

## 16. Primary source map and research limits

All upstream source links below are pinned to
`aa9c16803dd841950e3d4f4c7ecd76f324e0ba44`. The working-tree inventory is dated,
not tied to a clean repository commit; recheck it before implementation.

| Source | What was inspected |
| --- | --- |
| [Immutable Preview release](https://github.com/zed-industries/zed/releases/tag/v1.25.0-pre) and [tag API](https://api.github.com/repos/zed-industries/zed/git/ref/tags/v1.25.0-pre) | Release date, matching version, exact commit. |
| [Toolchain](https://github.com/zed-industries/zed/blob/aa9c16803dd841950e3d4f4c7ecd76f324e0ba44/rust-toolchain.toml) | Declared Rust toolchain. |
| [Workspace manifest](https://github.com/zed-industries/zed/blob/aa9c16803dd841950e3d4f4c7ecd76f324e0ba44/Cargo.toml) | Workspace paths and dependency patches. |
| [Editor manifest](https://github.com/zed-industries/zed/blob/aa9c16803dd841950e3d4f4c7ecd76f324e0ba44/crates/editor/Cargo.toml) | Features, mandatory dependencies, declared license. |
| [GPUI manifest](https://github.com/zed-industries/zed/blob/aa9c16803dd841950e3d4f4c7ecd76f324e0ba44/crates/gpui/Cargo.toml), [platform manifest](https://github.com/zed-industries/zed/blob/aa9c16803dd841950e3d4f4c7ecd76f324e0ba44/crates/gpui_platform/Cargo.toml), [platform entry point](https://github.com/zed-industries/zed/blob/aa9c16803dd841950e3d4f4c7ecd76f324e0ba44/crates/gpui_platform/src/gpui_platform.rs) | Platform split, features, application constructor. |
| [Editor implementation](https://github.com/zed-industries/zed/blob/aa9c16803dd841950e3d4f4c7ecd76f324e0ba44/crates/editor/src/editor.rs) and [editor tests](https://github.com/zed-industries/zed/blob/aa9c16803dd841950e3d4f4c7ecd76f324e0ba44/crates/editor/src/editor_tests.rs) | Constructors, read-only/masking/text operations, events, test initialization. |
| [Settings initialization](https://github.com/zed-industries/zed/blob/aa9c16803dd841950e3d4f4c7ecd76f324e0ba44/crates/settings/src/settings.rs) and [keymap loading](https://github.com/zed-industries/zed/blob/aa9c16803dd841950e3d4f4c7ecd76f324e0ba44/crates/settings/src/keymap_file.rs) | Default settings store and keymap assets/parser. |
| [Theme implementation](https://github.com/zed-industries/zed/blob/aa9c16803dd841950e3d4f4c7ecd76f324e0ba44/crates/theme/src/theme.rs) and [theme-settings initialization](https://github.com/zed-industries/zed/blob/aa9c16803dd841950e3d4f4c7ecd76f324e0ba44/crates/theme_settings/src/theme_settings.rs) | Theme globals, initialization and active theme update. |
| [Project implementation](https://github.com/zed-industries/zed/blob/aa9c16803dd841950e3d4f4c7ecd76f324e0ba44/crates/project/src/project.rs), [manifest](https://github.com/zed-industries/zed/blob/aa9c16803dd841950e3d4f4c7ecd76f324e0ba44/crates/project/Cargo.toml), [Workspace manifest](https://github.com/zed-industries/zed/blob/aa9c16803dd841950e3d4f4c7ecd76f324e0ba44/crates/workspace/Cargo.toml) | Local Project dependencies, flags and async worktree/buffer access. |
| [Buffer](https://github.com/zed-industries/zed/blob/aa9c16803dd841950e3d4f4c7ecd76f324e0ba44/crates/language/src/buffer.rs), [LanguageRegistry](https://github.com/zed-industries/zed/blob/aa9c16803dd841950e3d4f4c7ecd76f324e0ba44/crates/language/src/language_registry.rs), [languages initialization](https://github.com/zed-industries/zed/blob/aa9c16803dd841950e3d4f4c7ecd76f324e0ba44/crates/languages/src/lib.rs), [grammar manifest](https://github.com/zed-industries/zed/blob/aa9c16803dd841950e3d4f4c7ecd76f324e0ba44/crates/grammars/Cargo.toml) | Local buffers, async language loading, adapter/grammar registration choices. |
| [Zed app initialization](https://github.com/zed-industries/zed/blob/aa9c16803dd841950e3d4f4c7ecd76f324e0ba44/crates/zed/src/main.rs) | Production service-init ordering and the distinction from a minimal editor. |

License statements above report source declarations, not a legal conclusion
about AgentWatcher's distribution. Record the complete resolved dependency and
asset set and determine the applicable distribution terms before publication.

The original research checked selected source APIs and installed app metadata.
Implementation additionally resolved the lockfile, compiled the production app's
source with actual upstream Editor/Buffer, exercised preparation and packaging
fixtures, and received independent source review. The native v5 app also links,
packages, launches, and passes the recorded interactions and root suite. The
additional upstream policy tests, full IME/VoiceOver, Project/LSP behavior,
complete performance comparison, and public-distribution verification remain
outstanding. See the dated evidence files above for exact results and limits.
