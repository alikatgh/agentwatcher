# Roadmap — AgentWatcher & Nyurgun Bootur

**The thesis.** Agents are the future; watching them closely is the game
changer. AgentWatcher becomes the mission control for autonomous coding
agents — the window, the guardrails, and the proof. Nyurgun Bootur, a Sakha
epic brought to Nintendo-level craft by a 24/7 agent loop, is the flagship
demonstration that AI-authored work can reach human craft.

---

## Track A — AgentWatcher: dashboard → mission control

### A1 · Foundation — **done (0.1.x)**
- [x] Native macOS dashboard: Overview, Live Activity, Processes, Increments,
      Changes, Sessions, Live Sessions
- [x] OpenCode design language adopted wholesale (theme tokens, expandable
      cards, right sidebars, line-numbered diffs)
- [x] Global persistent right sidebar, keyboard shortcuts (⌘1–8, ⌘R, ⌘B),
      window state restoration, menu-bar watcher
- [x] CI + tagged-release automation on GitHub Actions

### A2 · Multiplicity — **next (0.2)**
- [ ] Multiple workers/projects in one window with a project switcher
- [ ] Run history database (SQLite): every run, session, cost and token
      count — searchable across days
- [ ] Native notifications on run finish/fail, plus per-run deep links into
      the transcript and the live session
- [ ] First-run onboarding wizard: detect → configure → start, one click
- [ ] Budget tracking per project, surfacing subscription usage limits
      before they bite

### A3 · Fleet — **(0.3)**
- [ ] Remote workers over SSH: the same watcher for cloud agents
- [ ] Alerting: webhooks/Slack on failure and stall detection
      ("agent stuck > N minutes")
- [ ] Policy guardrails: per-project allow/deny lists for shell commands,
      an approval queue for risky actions
- [ ] iOS companion: read-only dashboards and menu-bar status against the
      same server API

### A4 · Mission control — **(1.0)**
- [ ] Multi-machine fleets with a live activity map
- [ ] Audit log: every command, edit, commit and its verification status
- [ ] Plugin system for custom panels (test dashboards, art review queues)
- [ ] Hosted and self-hosted distributions; notarized builds

---

## Track B — Nyurgun Bootur: prototype → Nintendo-level

### B1 · Game feel — **in progress via the 24/7 loop**
- [x] Grounding shadows, hit flashes, combat feedback, scene-transition
      fades, motion dust, HUD damage/health feedback (rescue + VI/VII)
- [ ] VI–VIII motion parity; landing/dash polish everywhere; enemy death
      choreography; screen-edge danger states everywhere

### B2 · Character craft
- [ ] R5 face-scale/hip/gait overhaul; animation blending
      (idle ↔ run ↔ attack ↔ hurt)
- [ ] Per-chapter unique animations for story beats

### B3 · Art to final standard
- [ ] Fix browser automation; regenerate every atlas family at source
      resolution through the project image conversation
- [ ] 4K presentation pipeline; final character and world review

### B4 · Audio
- [ ] VoiceStudio offline setup; dialogue takes for all chapters with
      original voice designs
- [ ] YuE Olonkho-influenced score; approved takes bound to subtitles

### B5 · Native and release
- [ ] Fix the macOS launch blocker; native play review; 60 FPS budget
- [ ] Fluent Sakha review and cultural approval
- [ ] Full-epic adaptation (chapter IX research), notarized public release

---

## Track C — The loop (infrastructure)

- [ ] Orchestrator: multiple projects, priorities, per-project budgets
- [ ] Verification upgrades: golden-run comparisons, performance gates,
      flake detection
- [ ] Failure playbook: automatic retry with narrower scope, then a human
      escalation report

---

## Milestones

| Milestone | Target | Definition of done |
| --- | --- | --- |
| M1 | this week | A2 multi-project + run database; B1 complete |
| M2 | this month | A3 fleet basics; B2/B3 underway with regenerated art |
| M3 | this quarter | A4 preview; B4/B5 with verified native play |
| M4 | this year | AgentWatcher 1.0; Nyurgun public release |

Every checkbox lands the same way: implemented, verified with evidence,
independently reviewed, committed, pushed — never claimed ahead of proof.
