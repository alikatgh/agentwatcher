//! Graph deployment and per-agent inspection. All filesystem/provider work runs off-thread.
use super::*;
use agentwatcher::agent_engine::{LaunchRequest, ParentKey, RunSnapshot, SandboxMode};
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};

fn run_key(run: &RunSnapshot) -> (String, String) {
    ("codex".into(), format!("graph/{}", run.launch_id))
}
fn run_session(run: &RunSnapshot) -> Session {
    Session {
        source: "codex".into(),
        id: run_key(run).1,
        title: run.title.clone(),
        project: run.workspace.to_string_lossy().into_owned(),
        parent_id: run
            .parent
            .as_ref()
            .map(|p| format!("{}/{}", p.source, p.id))
            .unwrap_or_default(),
        agent: "Graph launch".into(),
        updated: run.started_at.clone(),
        path: String::new(),
    }
}
fn run_snapshot(run: &RunSnapshot) -> Snapshot {
    Snapshot {
        session: run_session(run),
        records: run.records.clone(),
        usage: run.usage.clone(),
        model: "Codex · Graph engine".into(),
        error: run.error.clone(),
        warnings: if run.truncated {
            vec!["Live output reached its retention limit.".into()]
        } else {
            vec![]
        },
        ..Snapshot::default()
    }
}
fn text(value: impl Into<String>, color: u32) -> Div {
    div()
        .w_full()
        .min_w_0()
        .text_size(px(12.))
        .line_height(px(18.))
        .text_color(rgb(color))
        .child(value.into())
}
fn section(p: Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .w_full()
        .min_w_0()
        .flex_shrink_0()
        .p(px(14.))
        .gap(px(8.))
        .border_b_1()
        .border_color(rgb(p.border))
}
fn context_signature(session: &Session, snapshot: &Snapshot) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    (
        &session.source,
        &session.id,
        &session.project,
        &session.agent,
        &snapshot.session.source,
        &snapshot.session.id,
        &snapshot.session.project,
        &snapshot.error,
        snapshot.window_start,
        snapshot.window_end,
        snapshot.has_earlier,
    )
        .hash(&mut hash);
    let mut remaining = 4 * 1024 * 1024;
    for record in snapshot.records.iter().take(4096) {
        (&record.id, &record.kind, &record.time, &record.tool_status).hash(&mut hash);
        for text in [&record.summary, &record.detail] {
            let bytes = text.as_bytes();
            let amount = bytes.len().min(256 * 1024).min(remaining);
            bytes[..amount].hash(&mut hash);
            remaining -= amount;
        }
        if remaining == 0 {
            break;
        }
    }
    hash.finish()
}
impl Workspace {
    pub(super) fn request_engine_quit(&mut self, cx: &mut Context<Self>) {
        if self.editor.is_busy() {
            self.editor.quit_accepted = false;
            self.message =
                "A local file operation is finishing. Quit again when it completes.".into();
            cx.notify();
            return;
        }
        if self.editor.has_dirty(cx) && !self.editor.quit_accepted {
            self.editor.quit_confirm = true;
            cx.notify();
            return;
        }
        self.begin_settings_quit(cx);
        self.quitting = true;
        self.chat.shutdown();
        self.tools.terminal.shutdown();
        self.editor.freeze(cx);
        self.engine.shutdown();
        if self.engine.is_idle() && self.tools.terminal.is_idle() && self.settings_quit_ready() {
            cx.quit();
        } else {
            self.message = "Stopping the active Graph agent before closing…".into();
            cx.notify();
        }
    }

    pub(super) fn engine_run(&self, source: &str, id: &str) -> Option<&RunSnapshot> {
        if source != "codex" {
            return None;
        }
        let launch_id = id.strip_prefix("graph/")?;
        self.engine_runs
            .iter()
            .find(|run| run.launch_id == launch_id)
    }
    pub(super) fn request_catalog(&mut self, cx: &mut Context<Self>) {
        self.request_catalog_for(self.settings.project.clone(), cx);
    }
    fn request_catalog_for(&mut self, workspace: String, cx: &mut Context<Self>) {
        self.catalog_generation += 1;
        self.catalog_loading = true;
        self.catalog_workspace = workspace.clone();
        self.catalog_worker
            .request((self.catalog_generation, workspace));
        cx.notify();
    }
    pub(super) fn launch_catalog_skill(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate(7, window, cx);
        self.begin_graph_creation(None, cx);
        self.launch_skills = vec![id];
        self.launch_skill_picker = true;
        cx.notify();
    }
    pub(super) fn begin_graph_creation(
        &mut self,
        parent: Option<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        let session = parent
            .as_ref()
            .and_then(|key| {
                self.known_sessions
                    .iter()
                    .find(|s| s.source == key.0 && s.id == key.1)
            })
            .cloned();
        self.graph.launch_parent = parent;
        self.graph.new_source = "codex".into();
        self.graph.creating_agent = true;
        self.graph.inspector_open = true;
        self.graph.clear_launch_interaction();
        self.graph
            .launch_scroll
            .set_offset(gpui::point(px(0.), px(0.)));
        self.launch_acknowledged = false;
        self.launch_error.clear();
        self.launch_write = self.settings.preferences.launch_write_access;
        self.launch_minutes = self.settings.preferences.launch_deadline_minutes;
        self.launch_skills.clear();
        self.launch_skill_picker = false;
        self.launch_skill_search
            .update(cx, |v, cx| v.set_value("", cx));
        let workspace = session
            .map(|s| s.project)
            .unwrap_or_else(|| self.settings.project.clone());
        if self.catalog_workspace != workspace {
            self.request_catalog_for(workspace.clone(), cx);
        }
        self.launch_workspace
            .update(cx, |v, cx| v.set_value(workspace, cx));
        cx.notify();
    }
    fn choose_launch_workspace(&mut self, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Agent workspace".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = prompt.await {
                if let Some(path) = paths.first() {
                    let path = path.to_string_lossy().into_owned();
                    let _ = this.update(cx, |this, cx| {
                        if this.catalog_workspace != path {
                            this.request_catalog_for(path.clone(), cx);
                        }
                        this.launch_workspace
                            .update(cx, |v, cx| v.set_value(path, cx));
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }
    pub(super) fn start_graph_agent(&mut self, cx: &mut Context<Self>) {
        if self.settings.preferences.monitor_only {
            self.launch_error =
                "Monitor only is enabled. Turn it off in Launching settings to run an agent."
                    .into();
            cx.notify();
            return;
        }
        if !self.launch_skills.is_empty()
            && (self.catalog_loading
                || self.catalog_workspace != self.launch_workspace.read(cx).value())
        {
            self.launch_error = "Workspace changed. Open Choose skills to refresh its available skills before starting.".into();
            cx.notify();
            return;
        }
        let task = match agentwatcher::launch_skills::compose_task(
            self.launch_task.read(cx).value(),
            &self.launch_skills,
            &self.catalog,
        ) {
            Ok(task) => task,
            Err(error) => {
                self.launch_error = error;
                cx.notify();
                return;
            }
        };
        let request = LaunchRequest {
            task,
            title: self.launch_title.read(cx).value().into(),
            workspace: self.launch_workspace.read(cx).value().into(),
            sandbox: if self.launch_write {
                SandboxMode::WorkspaceWrite
            } else {
                SandboxMode::ReadOnly
            },
            parent: self.graph.launch_parent.as_ref().map(|key| ParentKey {
                source: key.0.clone(),
                id: key.1.clone(),
            }),
            timeout: Duration::from_secs(self.launch_minutes * 60),
            acknowledge_possible_paid_credit_usage: self.launch_acknowledged,
        };
        match self.engine.start(request) {
            Ok(info) => {
                self.launch_acknowledged = false;
                self.launch_error.clear();
                self.engine_revision = u64::MAX;
                self.poll_graph_engine(cx);
                let key = ("codex".to_owned(), format!("graph/{}", info.launch_id));
                self.graph.select_launch(key.clone());
                self.source = key.0;
                self.session_id = key.1;
                self.epoch += 1;
                self.clear_details();
                self.sync_selected_engine(cx);
                self.request_agent_context(cx);
                self.message = "Agent started in Graph. Follow its work in the sidebar.".into();
            }
            Err(error) => self.launch_error = error,
        }
        cx.notify();
    }
    pub(super) fn poll_graph_engine(&mut self, cx: &mut Context<Self>) {
        let revision = self.engine.revision();
        if revision == self.engine_revision {
            return;
        }
        self.engine_revision = revision;
        self.engine_runs = self.engine.snapshot();
        let sessions = self.engine_runs.iter().map(run_session).collect::<Vec<_>>();
        self.merge_sessions(&sessions);
        self.integrate_engine_activity();
        self.sync_selected_engine(cx);
        self.updated = Instant::now();
        cx.notify();
    }
    pub(super) fn sync_selected_engine(&mut self, cx: &mut Context<Self>) {
        let Some(run) = self.engine_run(&self.source, &self.session_id).cloned() else {
            return;
        };
        let selected = self
            .selected_record
            .and_then(|i| self.snapshot.records.get(i))
            .map(|r| r.id.clone());
        self.snapshot = run_snapshot(&run);
        self.loading = false;
        self.record_search = self
            .snapshot
            .records
            .iter()
            .map(|r| format!("{} {} {}", r.kind, r.summary, r.detail).to_lowercase())
            .collect();
        self.selected_record = retained_record_index(&self.snapshot.records, selected.as_deref());
        if let Some(index) = self.selected_record {
            let record = &self.snapshot.records[index];
            self.detail_title = record.kind.clone();
            self.detail = lines(record.detail.clone());
            self.visual_detail
                .update(cx, |v, cx| v.set_record(record, cx));
        }
        self.filter_records();
        self.request_agent_context(cx);
    }
    pub(super) fn integrate_engine_activity(&mut self) {
        let mut owned = Vec::new();
        for run in &self.engine_runs {
            let key = run_key(run);
            let signature = agentwatcher::activity::event_signature(&run.records);
            let old = self.agent_activity.iter().find(|a| a.key == key);
            let changed = old.is_some_and(|a| a.revision != signature);
            if changed {
                self.activity_changes.insert(key.clone(), Instant::now());
            }
            let records = run
                .records
                .iter()
                .filter(|r| !r.is_infrastructure)
                .collect::<Vec<_>>();
            let tail = records
                .iter()
                .rev()
                .take(24)
                .rev()
                .copied()
                .collect::<Vec<_>>();
            let mut timeline = BTreeMap::<u64, [u32; 6]>::new();
            for r in &records {
                if let Some(at) = agentwatcher::activity::timestamp_seconds(&r.time) {
                    timeline.entry(at / 60).or_default()
                        [agentwatcher::activity::classify(r).index()] += 1;
                }
            }
            owned.push(AgentActivity {
                key,
                text: records
                    .last()
                    .map(|r| r.summary.chars().take(160).collect())
                    .unwrap_or_else(|| run.task.chars().take(160).collect()),
                model: format!("Graph · {}", run.status.label()),
                usage: run.usage.clone(),
                samples: tail
                    .iter()
                    .map(|r| agentwatcher::activity::classify(r))
                    .collect(),
                sample_ids: tail.iter().map(|r| r.id.clone()).collect(),
                timeline: timeline
                    .into_iter()
                    .map(|(minute, counts)| agentwatcher::activity::ActivityBucket {
                        minute,
                        counts,
                    })
                    .collect(),
                event_count: records.len(),
                tool_count: records.iter().filter(|r| r.kind == "tool").count(),
                error_count: records.iter().filter(|r| r.tool_status == "Error").count(),
                last_event_time: records.last().map(|r| r.time.clone()).unwrap_or_default(),
                loaded: true,
                initial_load: old.is_none(),
                changed,
                revision: signature,
                ..AgentActivity::default()
            });
        }
        self.agent_activity
            .retain(|a| !owned.iter().any(|b| a.key == b.key));
        owned.append(&mut self.agent_activity);
        owned.truncate(64);
        self.agent_activity = owned;
    }
    pub(super) fn request_agent_context(&mut self, _cx: &mut Context<Self>) {
        let Some(key) = self.graph.selected.as_ref() else {
            return;
        };
        let Some(session) = self
            .known_sessions
            .iter()
            .find(|s| s.source == key.0 && s.id == key.1)
            .cloned()
        else {
            return;
        };
        let empty_snapshot = Snapshot::default();
        let snapshot = if self.snapshot.session.source == key.0 && self.snapshot.session.id == key.1
        {
            &self.snapshot
        } else {
            &empty_snapshot
        };
        let signature = context_signature(&session, snapshot);
        let request = (key.0.clone(), key.1.clone(), signature);
        if self.context_requested.as_ref() == Some(&request) {
            return;
        }
        let snapshot = snapshot.clone();
        if self
            .context_requested
            .as_ref()
            .is_none_or(|old| old.0 != key.0 || old.1 != key.1)
        {
            self.context = None;
            self.context_document = None;
        }
        self.context_requested = Some(request.clone());
        self.context_worker.request((request, session, snapshot));
    }
    pub(super) fn render_graph_launch(
        &self,
        width: f32,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let active = self.engine_runs.iter().any(|r| r.status.is_active());
        let parent = self.graph.launch_parent.as_ref().and_then(|key| {
            self.known_sessions
                .iter()
                .find(|s| s.source == key.0 && s.id == key.1)
        });
        let mut access = div().flex().gap(px(4.)).flex_wrap();
        for (write, label) in [(false, "Read only"), (true, "Workspace write")] {
            access = access.child(
                action(
                    label,
                    p,
                    cx.listener(move |this, _, _, cx| {
                        this.launch_write = write;
                        cx.notify();
                    }),
                )
                .when(self.launch_write == write, |s| {
                    s.bg(rgb(p.selected)).text_color(rgb(p.text))
                }),
            );
        }
        let mut limits = div().flex().flex_wrap().gap(px(4.));
        let mut minute_choices = vec![1, 5, 10, self.launch_minutes];
        minute_choices.sort_unstable();
        minute_choices.dedup();
        for minutes in minute_choices {
            limits = limits.child(
                action(
                    format!("{minutes} min"),
                    p,
                    cx.listener(move |this, _, _, cx| {
                        this.launch_minutes = minutes;
                        cx.notify();
                    }),
                )
                .when(self.launch_minutes == minutes, |s| {
                    s.bg(rgb(p.selected)).text_color(rgb(p.text))
                }),
            );
        }
        div().flex().flex_col().w(px(width)).h_full().min_w_0().min_h_0().flex_shrink_0().overflow_hidden().bg(rgb(p.base))
            .child(div().flex().items_center().w_full().min_w_0().h(px(36.)).flex_shrink_0().px(px(12.)).gap(px(8.)).border_b_1().border_color(rgb(p.border))
                .child(icon("agent",p.accent)).child(text("New agent",p.text).flex_1())
                .child(action("×",p,cx.listener(|this,_,_,cx|{this.graph.creating_agent=false;this.graph.inspector_open=false;cx.notify();}))))
            .child(div().id("graph-launch-form").flex().flex_col().flex_1().min_h_0().w_full().min_w_0().overflow_y_scroll().track_scroll(&self.graph.launch_scroll).bg(rgb(p.panel))
                .child(section(p).child(text("Release the next step",p.text).text_size(px(16.)).font_weight(FontWeight::MEDIUM))
                    .when_some(parent.filter(|_|self.settings.preferences.launch_show_context),|s,parent|s.child(text(format!("From {} · {}",source_label(&parent.source),agent_label(parent)),p.muted).line_clamp(2)))
                    .when(self.settings.preferences.launch_show_context,|s|s.child(text("A new task, linked to this node in Graph. It gets its own instructions and trace.",p.faint))))
                .child(section(p).bg(rgb(p.base))
                    .child(text("Engine",p.muted)).child(div().flex().items_center().gap(px(6.)).child(icon("agent",p.accent)).child(text("Codex",p.text)))
                    .child(text("Runs here using your installed Codex and ChatGPT sign-in.",p.faint))
                    .child(text("Task",p.text).font_weight(FontWeight::MEDIUM)).child(self.launch_task.clone())
                    .child(text("Agent name",p.muted)).child(self.launch_title.clone())
                    .child(div().flex().items_center().child(text("Workspace",p.muted).flex_1()).child(action("Browse…",p,cx.listener(|this,_,_,cx|this.choose_launch_workspace(cx)))))
                    .child(self.launch_workspace.clone()))
                .child(self.render_launch_skills(p,cx))
                .child(section(p).bg(rgb(p.base)).child(text("Access",p.muted)).child(access).child(text("Time limit",p.muted)).child(limits)
                    .child(text("One task at a time. No automatic restart or subagent spawning. Closing AgentWatcher stops its active run.",p.faint))))
                .child(section(p).bg(rgb(p.raised))
                    .child(div().id("graph-launch-account-consent").flex().gap(px(8.)).cursor_pointer()
                        .on_click(cx.listener(|this,_,_,cx|{this.launch_acknowledged=!this.launch_acknowledged;cx.notify();}))
                        .child(div().flex_shrink_0().w(px(18.)).h(px(18.)).rounded(px(3.)).border_1().border_color(rgb(if self.launch_acknowledged{p.focus}else{p.border})).bg(rgb(if self.launch_acknowledged{p.selected}else{p.control})).text_center().child(if self.launch_acknowledged{"✓"}else{""}))
                        .child(text("I accept possible paid-credit usage for this run.",p.text).flex_1()))
                    .child(text("Your provider's plan and credits apply. The time limit does not guarantee a billing cap.",p.faint))
                    .when(!self.launch_error.is_empty(),|s|s.child(text(self.launch_error.clone(),p.red)))
                    .when(active,|s|s.child(text("An agent is already running. Select it to follow its work or stop it.",p.muted)))
                    .when(self.settings.preferences.monitor_only,|s|s.child(text("Monitor only is enabled in Launching settings.",p.muted)))
                    .child(action("Start agent →",p,cx.listener(|this,_,_,cx|this.start_graph_agent(cx)))
                        .w_full().h(px(32.)).bg(rgb(p.accent)).text_color(rgb(if p.light{0xffffff}else{p.panel}))
                        .when(self.settings.preferences.monitor_only||active||!self.launch_acknowledged||self.launch_task.read(cx).value().trim().is_empty(),|s|s.opacity(0.45))))
    }
    fn render_launch_skills(&self, p: Palette, cx: &mut Context<Self>) -> Div {
        let mut body = section(p).bg(rgb(p.panel)).child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    text(
                        format!("Skills · {} selected", self.launch_skills.len()),
                        p.text,
                    )
                    .flex_1(),
                )
                .child(action(
                    if self.launch_skill_picker {
                        "Done"
                    } else {
                        "Choose…"
                    },
                    p,
                    cx.listener(|this, _, _, cx| {
                        this.launch_skill_picker = !this.launch_skill_picker;
                        let workspace = this.launch_workspace.read(cx).value().to_owned();
                        if this.launch_skill_picker && this.catalog_workspace != workspace {
                            this.request_catalog_for(workspace, cx);
                        }
                        cx.notify();
                    }),
                )),
        );
        for id in &self.launch_skills {
            let skill = self.catalog.skills.iter().find(|s| &s.id == id);
            let id = id.clone();
            body = body.child(
                div()
                    .id(SharedString::from(format!("selected-launch-skill:{id}")))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        text(
                            skill
                                .map(|s| format!("{} · {}", s.name, s.scope))
                                .unwrap_or_else(|| "Unavailable skill".into()),
                            p.accent,
                        )
                        .flex_1()
                        .min_w_0()
                        .line_clamp(1),
                    )
                    .child(action(
                        "×",
                        p,
                        cx.listener(move |this, _, _, cx| {
                            this.launch_skills.retain(|s| s != &id);
                            cx.notify();
                        }),
                    )),
            );
        }
        if self.launch_skill_picker {
            let query = self.launch_skill_search.read(cx).value().to_lowercase();
            let skills: Vec<_> = self
                .catalog
                .skills
                .iter()
                .filter(|s| {
                    format!("{} {} {}", s.name, s.description, s.scope)
                        .to_lowercase()
                        .contains(&query)
                })
                .cloned()
                .collect();
            body=body.child(self.launch_skill_search.clone())
                .child(text("Choose up to 8. Instructions are read only after Start; tool dependencies must be available in this run.",p.faint));
            if self.catalog_loading {
                body = body.child(text("Reading installed skills…", p.muted));
            }
            let rows = uniform_list(
                "launch-skill-choices",
                skills.len(),
                cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                    range
                        .filter_map(|index| skills.get(index))
                        .map(|skill| {
                            let id = skill.id.clone();
                            let selected = this.launch_skills.contains(&id);
                            div()
                                .id(SharedString::from(format!("choose-{id}")))
                                .h(px(58.))
                                .w_full()
                                .min_w_0()
                                .px(px(8.))
                                .flex()
                                .flex_col()
                                .justify_center()
                                .gap(px(3.))
                                .cursor_pointer()
                                .when(selected, |s| s.bg(rgb(p.selected)))
                                .hover(move |s| {
                                    s.bg(rgb(if selected { p.selected } else { p.raised }))
                                })
                                .child(
                                    text(
                                        format!(
                                            "{} {} · {}",
                                            if selected { "✓" } else { "+" },
                                            skill.name,
                                            skill.scope
                                        ),
                                        p.text,
                                    )
                                    .line_clamp(1),
                                )
                                .child(
                                    text(skill.description.clone(), p.faint)
                                        .text_size(px(10.))
                                        .line_clamp(1),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if this.launch_skills.contains(&id) {
                                        this.launch_skills.retain(|s| s != &id);
                                    } else if this.launch_skills.len()
                                        < agentwatcher::launch_skills::MAX_SELECTED_SKILLS
                                    {
                                        this.launch_skills.push(id.clone());
                                    } else {
                                        this.launch_error =
                                            "Choose up to 8 skills for one agent.".into();
                                    }
                                    cx.notify();
                                }))
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .h(px(180.))
            .flex_shrink_0()
            .border_1()
            .border_color(rgb(p.border))
            .bg(rgb(p.base))
            .w_full();
            body = body.child(rows);
        }
        body
    }
    pub(super) fn render_graph_agent_controls(
        &self,
        key: &(String, String),
        p: Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let Some(run) = self.engine_run(&key.0, &key.1) else {
            return self.render_agent_controls(p, cx).w_full().min_w_0();
        };
        let id = run.launch_id.clone();
        let parent = key.clone();
        div()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(6.))
                    .child(
                        div()
                            .size(px(6.))
                            .rounded_full()
                            .bg(rgb(if run.status.is_active() {
                                p.green
                            } else if run.error.is_some() {
                                p.red
                            } else {
                                p.faint
                            })),
                    )
                    .child(text(format!("{} · in Graph", run.status.label()), p.text).flex_1())
                    .when(run.status.is_active(), |s| {
                        s.child(
                            action(
                                "Stop",
                                p,
                                cx.listener(move |this, _, _, cx| {
                                    this.message = this
                                        .engine
                                        .stop(&id)
                                        .err()
                                        .unwrap_or_else(|| "Stopping this Graph agent…".into());
                                    cx.notify();
                                }),
                            )
                            .text_color(rgb(p.red)),
                        )
                    }),
            )
            .child(
                action(
                    "+ Next agent",
                    p,
                    cx.listener(move |this, _, _, cx| {
                        this.begin_graph_creation(Some(parent.clone()), cx)
                    }),
                )
                .bg(rgb(p.control)),
            )
            .when_some(run.thread_id.as_ref(), |s, id| {
                s.child(text(
                    format!("Provider session · {}", short_id(id)),
                    p.faint,
                ))
            })
            .when_some(run.error.as_ref(), |s, error| {
                s.child(text(error.clone(), p.red))
            })
    }
    pub(super) fn render_graph_agent_context(&self, p: Palette, cx: &mut Context<Self>) -> Div {
        let Some(context) = self.context.as_ref() else {
            return section(p)
                .child(text("Instructions & skills", p.text))
                .child(text("Reading this agent's context…", p.faint));
        };
        let mut body = section(p).gap(px(6.)).child(
            action(
                format!(
                    "{} Instructions · {}",
                    if self.context_guidance_open {
                        "▾"
                    } else {
                        "▸"
                    },
                    context.guidance.len()
                ),
                p,
                cx.listener(|this, _, _, cx| {
                    this.context_guidance_open = !this.context_guidance_open;
                    cx.notify();
                }),
            )
            .justify_start()
            .px(px(0.))
            .text_color(rgb(p.text)),
        );
        if self.context_guidance_open {
            if context.guidance.is_empty() {
                body = body.child(text(
                    "No applicable AGENTS.md was found in the workspace hierarchy.",
                    p.faint,
                ));
            }
            for doc in &context.guidance {
                let id = doc.id.clone();
                let title = format!("{} · {}\n{}", doc.source.label(), name(&doc.path), doc.path);
                let content = doc.text.clone();
                let path = doc.path.clone();
                let current = doc.source == agent_context::GuidanceSource::CurrentFilesystem;
                body = body.child(
                    div()
                        .id(SharedString::from(format!("guidance-row-{}", doc.id)))
                        .flex()
                        .flex_col()
                        .w_full()
                        .min_w_0()
                        .gap(px(2.))
                        .child(
                            action(
                                name(&doc.path),
                                p,
                                cx.listener(move |this, _, window, cx| {
                                    if current {
                                        this.open_workspace_file(path.clone(), window, cx);
                                    } else {
                                        this.open_workspace_snapshot(
                                            format!("{title}\n{id}"),
                                            lines(content.clone()),
                                            window,
                                            cx,
                                        );
                                    }
                                }),
                            )
                            .justify_start()
                            .px(px(0.))
                            .text_color(rgb(p.accent)),
                        )
                        .child(
                            text(
                                if self.settings.preferences.hide_workspace_paths {
                                    name(&doc.path)
                                } else {
                                    doc.path.clone()
                                },
                                p.faint,
                            )
                            .text_size(px(10.))
                            .line_clamp(2),
                        )
                        .child(
                            text(format!("{} · {}", doc.source.label(), doc.scope), p.muted)
                                .text_size(px(10.)),
                        )
                        .when_some(doc.trace.first(), |s, trace| {
                            s.child(self.context_evidence_link(trace, p, cx))
                        })
                        .when_some(doc.error.as_ref(), |s, error| {
                            s.child(text(error.clone(), p.red))
                        }),
                );
            }
            body=body.child(text("Current files describe today's guidance. Recorded versions show what this trace actually captured.",p.faint).text_size(px(11.)));
        }
        body = body.child(
            action(
                format!(
                    "{} Skills observed · {}",
                    if self.context_skills_open {
                        "▾"
                    } else {
                        "▸"
                    },
                    context.skills.len()
                ),
                p,
                cx.listener(|this, _, _, cx| {
                    this.context_skills_open = !this.context_skills_open;
                    cx.notify();
                }),
            )
            .justify_start()
            .px(px(0.))
            .text_color(rgb(p.text)),
        );
        if self.context_skills_open {
            if context.skills.is_empty() {
                body=body.child(text("No skill use is recorded in the loaded trace. Available skills are not proof of use.",p.faint));
            }
            for skill in &context.skills {
                let mut row = div()
                    .id(SharedString::from(format!("skill-row-{}", skill.id)))
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .w_full()
                    .min_w_0()
                    .child(text(skill.name.clone(), p.text).font_weight(FontWeight::MEDIUM))
                    .child(text(skill.kind.label(), p.muted).text_size(px(10.)))
                    .when_some(skill.path.as_ref(), |s, path| {
                        s.child(
                            text(
                                if self.settings.preferences.hide_workspace_paths {
                                    name(path)
                                } else {
                                    path.clone()
                                },
                                p.faint,
                            )
                            .text_size(px(10.))
                            .line_clamp(2),
                        )
                    });
                let mut versions = div().flex().flex_wrap().gap(px(4.));
                if let Some(recorded) = &skill.recorded_text {
                    let content = recorded.clone();
                    let key = format!("Recorded skill · {}\n{}", skill.name, skill.id);
                    versions = versions.child(
                        action(
                            "Recorded",
                            p,
                            cx.listener(move |this, _, _, cx| {
                                this.context_document = Some((key.clone(), lines(content.clone())));
                                cx.notify();
                            }),
                        )
                        .text_color(rgb(p.accent)),
                    );
                }
                if let Some(doc) = &skill.current_document {
                    let content = doc.text.clone();
                    let key = format!("Current skill file · {}\n{}", skill.name, doc.path);
                    versions = versions.child(
                        action(
                            "Current file",
                            p,
                            cx.listener(move |this, _, _, cx| {
                                this.context_document = Some((key.clone(), lines(content.clone())));
                                cx.notify();
                            }),
                        )
                        .text_color(rgb(p.accent)),
                    );
                }
                row = row
                    .child(versions)
                    .when_some(skill.trace.first(), |s, trace| {
                        s.child(self.context_evidence_link(trace, p, cx))
                    });
                body = body.child(row);
            }
            body=body.child(text("Observed in this recording window; a read does not prove the skill is still active.",p.faint).text_size(px(11.)));
        }
        if let Some((key, content)) = &self.context_document {
            let copy = content.source().to_owned();
            self.context_view.update(cx, |v, cx| {
                v.set_content_for(key, content.clone(), cx);
                v.set_palette(p, cx);
                v.set_font(style::CODE_FONT, cx);
            });
            body = body
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            text(key.lines().next().unwrap_or("Document"), p.text)
                                .flex_1()
                                .line_clamp(2),
                        )
                        .child(action(
                            "Copy",
                            p,
                            cx.listener(move |_, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                            }),
                        ))
                        .child(action(
                            "×",
                            p,
                            cx.listener(|this, _, _, cx| {
                                this.context_document = None;
                                cx.notify();
                            }),
                        )),
                )
                .child(
                    div()
                        .h(px(280.))
                        .w_full()
                        .min_w_0()
                        .flex_shrink_0()
                        .border_1()
                        .border_color(rgb(p.border))
                        .child(self.context_view.clone()),
                );
        }
        for warning in context.warnings.iter().take(2) {
            body = body.child(
                text(warning.clone(), p.faint)
                    .text_size(px(10.))
                    .line_clamp(3),
            );
        }
        if context.truncated {
            body = body.child(text(
                "Context preview is bounded. Open the full trace for additional evidence.",
                p.faint,
            ));
        }
        body
    }
    fn context_evidence_link(
        &self,
        trace: &agent_context::TraceReference,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let record = trace.record_id.clone();
        let key = self.graph.selected.clone();
        action(
            format!("Read evidence · {} →", display_time(&trace.time)),
            p,
            cx.listener(move |this, _, window, cx| {
                if let Some(key) = &key {
                    this.open_graph_event(&key.0, key.1.clone(), record.clone(), window, cx);
                }
            }),
        )
        .justify_start()
        .px(px(0.))
        .text_color(rgb(p.accent))
    }
}

#[cfg(test)]
mod tests {
    use super::{Record, Session, Snapshot, context_signature};
    #[test]
    fn context_revision_includes_identity_and_infrastructure_instructions() {
        let session = Session {
            source: "codex".into(),
            id: "same".into(),
            project: "/workspace".into(),
            ..Session::default()
        };
        let empty = Snapshot::default();
        let mut verified = Snapshot {
            session: session.clone(),
            ..Snapshot::default()
        };
        assert_ne!(
            context_signature(&session, &empty),
            context_signature(&session, &verified)
        );
        let before = context_signature(&session, &verified);
        verified.records.push(Record {
            id: "context".into(),
            kind: "context".into(),
            detail: "AGENTS.md instruction".into(),
            is_infrastructure: true,
            ..Record::default()
        });
        assert_ne!(before, context_signature(&session, &verified));
        let before = context_signature(&session, &verified);
        verified.records[0].detail = "Updated AGENTS.md".into();
        assert_ne!(before, context_signature(&session, &verified));
    }
}
