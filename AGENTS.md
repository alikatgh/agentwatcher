# Visuals first

**Visuals first is the agenda. We are investing heavily in visual quality.**

## Product vision

**Graph is the agent control surface. See what agents are doing visually, release
work from graph nodes, and follow inputs, outputs, handoffs, and subagents as they
happen. Prompt entry is a deeper configuration detail, not the primary workflow.**

**Win users with simplicity, readability, world-class visual polish, and speed.**
Start with a calm visual overview that a human can grasp immediately. Reveal
additional information only when requested, and make every next level easy to
discover. Keep the complete functionality for power users through direct
inspection, search, keyboard shortcuts, and the command palette. Simplicity is
progressive disclosure, not removal of capabilities.

Default screens must not dump raw payloads, developer controls, dense metrics,
or Git patches. Selecting an agent reveals its activity; selecting an event
reveals its evidence. Secondary tools and repository details open on request.
Every level must remain polished, readable, and responsive.

Agents stay separate and identifiable throughout the UI. Keep provider,
workspace, parent/child relationships, and the selected agent visible while
inspecting its trace. Global history must include previous agents and subagents
across workspaces, including DeepSeek; source filters must not erase history.
Code and evidence must support real text selection and exact copying.
Use Zed's IBM Plex Sans UI font and the installed Zed configuration's JetBrains
Mono code font consistently; bundle their upstream font licenses.

AgentWatcher exists to simplify seeing and controlling running coding agents.
Every change must improve that experience. Clear agent identity, status, activity,
hierarchy, and controls come before adding more features or exposing raw data.

- Build the application entirely in Rust with GPUI. Do not edit the Swift files.
- Use the installed Zed app and the supplied screenshots as the standard for
  visual craft: typography, density, pane proportions, separators, controls,
  icons, selected states, and keyboard interaction.
- Give real agent activity a useful visual form. Do not substitute text tables,
  giant counters, placeholder charts, or ornamental dashboard cards for design.
- Preserve useful workflow behavior while making running agents easier to see.
- Render and interact with the complete application before calling UI work done.
  Compilation and passing tests do not establish visual quality.
- Keep Git, process discovery, and history parsing off the GPUI thread. Bound
  collection work and preserve responsive input and navigation while loading.
- Preserve existing files, dirty work, old builds, and the previous Swift app.

Rust entry point: `rust/src/main.rs`. Build: `bash build-rust.sh`.
Tests: `cargo test --locked`. Formatting: `cargo fmt --check`.
