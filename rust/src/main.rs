mod agent_graph;
mod catalog_page;
mod chat_panel;
mod code_view;
mod content;
mod dashboard;
mod dashboard_chart;
mod event_view;
mod file_actions;
mod graph_inspector;
mod input;
mod settings_panel;
mod style;
mod tools_panel;
mod visuals;
mod workspace_editor;
mod zed_editor_runtime;

use agentwatcher::{
    activity::{ActivityCollector, ActivityKind, AgentActivity},
    agent_catalog, agent_context, agent_control, agent_engine,
    backend::{self, *},
    runtime::{LatestWorker, PersistenceStatus, PersistenceWorker, retained_record_index},
    telemetry::{self, *},
};
use code_view::CodeView;
use content::CodeContent;
use event_view::EventView;
use file_actions::FileTarget;
use gpui::{prelude::*, *};
use input::TextInput;
use std::{
    path::Path,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
use style::{Assets, Palette, icon};

actions!(
    workspace,
    [
        Quit,
        OpenWorkspace,
        Refresh,
        CommandPalette,
        ToggleSidebar,
        ToggleDock,
        Back,
        Forward,
        Overview,
        Activity,
        Processes,
        Increments,
        Changes,
        Sessions,
        SettingsView,
        Graph,
        AgentsSkills,
        SaveFile,
        OpenFile,
        AskPane,
        SendPaneChat,
        ToggleFiles,
        ToggleConsole,
        Escape,
        PaletteUp,
        PaletteDown,
        PaletteRun,
        Find
    ]
);
const PANES: [(&str, &str); 9] = [
    ("Dashboard", "overview"),
    ("Live Activity", "activity"),
    ("Processes", "process"),
    ("Increments", "increments"),
    ("Changes", "changes"),
    ("Agent history", "sessions"),
    ("Settings", "settings"),
    ("Graph", "agent"),
    ("Agents & Skills", "agent"),
];
type FileJob = Option<(u64, String, String, FileTarget)>;
type TaggedSettings = (u64, Settings);
type TelemetryJob = (
    u64,
    String,
    String,
    String,
    Option<Session>,
    Option<u64>,
    bool,
);
type StatusJob = (u64, Settings, Option<Session>);
type DiffJob = (u64, u64, String, String, bool);
enum Message {
    Directory(
        u64,
        String,
        String,
        Result<Vec<agentwatcher::local_tools::FileEntry>, String>,
    ),
    Repository(u64, RepositorySnapshot),
    Status(
        u64,
        WorkerState,
        Result<Vec<ProcessInfo>, String>,
        Option<ProcessInfo>,
    ),
    Telemetry(u64, Vec<Session>, Snapshot, Vec<String>, Duration),
    Diff(u64, u64, String, bool, Result<String, String>),
    Operation(u64, Result<(), String>),
    History(HistoryPage),
    Terminal(u64, Result<std::path::PathBuf, String>),
    Activities(Vec<AgentActivity>),
    AgentContext((String, String, u64), agent_context::AgentContext),
    Catalog(u64, agent_catalog::Catalog),
    Editor(u64, Result<workspace_editor::EditorResult, String>),
    Chat(
        u64,
        String,
        Result<agentwatcher::pane_chat::ChatReply, String>,
    ),
    FileTargets(Vec<FileTarget>),
    FileOpened(u64, String, Result<(), String>),
}
/// Carry provider identity through retained reads and catalog refreshes.
fn select_recording<'a>(
    source: &str,
    id: &str,
    retained: Option<&'a Session>,
    sessions: &'a [Session],
) -> Option<&'a Session> {
    let matches_source = |s: &&Session| source == "automatic" || s.source == source;
    if !id.is_empty() {
        retained
            .filter(|s| s.id == id && matches_source(s))
            .or_else(|| sessions.iter().filter(matches_source).find(|s| s.id == id))
    } else {
        sessions
            .iter()
            .filter(matches_source)
            .find(|s| s.parent_id.is_empty())
            .or_else(|| sessions.iter().filter(matches_source).next())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EvidenceMode {
    Visual,
    Raw,
    Tree,
}

#[derive(Clone, Copy)]
struct NativeQuitReadiness {
    requested: bool,
    settings_ready: bool,
    processes_idle: bool,
    editor_busy: bool,
    has_dirty_documents: bool,
    documents_approved: bool,
}

impl NativeQuitReadiness {
    fn can_terminate(self) -> bool {
        self.requested
            && self.settings_ready
            && self.processes_idle
            && !self.editor_busy
            && (!self.has_dirty_documents || self.documents_approved)
    }
}

fn should_resume_native_quit(
    pending: bool,
    quitting: bool,
    editor_busy: bool,
    confirmation_open: bool,
) -> bool {
    pending && !quitting && !editor_busy && !confirmation_open
}

struct Workspace {
    settings: Settings,
    settings_writer: PersistenceWorker<Settings>,
    settings_progress: PersistenceStatus,
    settings_submission_error: Option<String>,
    settings_quit_discard: bool,
    applied_preferences: Preferences,
    pending_graph_record: Option<(String, String, String)>,
    pane: usize,
    history: Vec<usize>,
    history_index: usize,
    focus: FocusHandle,
    search: Entity<TextInput>,
    palette_input: Entity<TextInput>,
    project_input: Entity<TextInput>,
    worker_input: Entity<TextInput>,
    state_input: Entity<TextInput>,
    _subscriptions: Vec<Subscription>,
    working_view: Entity<CodeView>,
    commit_view: Entity<CodeView>,
    dock_view: Entity<CodeView>,
    detail_view: Entity<CodeView>,
    visual_detail: Entity<EventView>,
    evidence_mode: EvidenceMode,
    file_targets: Vec<FileTarget>,
    file_menu: Option<(String, String, Point<Pixels>)>,
    log_view: Entity<CodeView>,
    query: String,
    palette_query: String,
    palette_open: bool,
    palette_index: usize,
    repository: RepositorySnapshot,
    repository_loading: bool,
    worker: WorkerState,
    worker_log: Arc<CodeContent>,
    process_list: Vec<ProcessInfo>,
    agent_process: Option<ProcessInfo>,
    process_error: String,
    sessions: Vec<Session>,
    known_sessions: Vec<Session>,
    agent_activity: Vec<AgentActivity>,
    activity_changes: std::collections::HashMap<(String, String), Instant>,
    history_cursor: Option<String>,
    history_loading: bool,
    history_complete: bool,
    history_project_only: bool,
    history_source: usize,
    history_warnings: Vec<String>,
    snapshot: Snapshot,
    record_search: Vec<String>,
    trace_windows: Vec<Option<u64>>,
    trace_index: usize,
    trace_time_range: Option<(u64, u64)>,
    trace_outcome: Option<ActivityKind>,
    source: String,
    session_id: String,
    record_mode: usize,
    records: Vec<usize>,
    selected_record: Option<usize>,
    selected_commit: String,
    selected_file: String,
    diff: Arc<CodeContent>,
    diff_title: String,
    commit_diff: Arc<CodeContent>,
    commit_title: String,
    commit_revision: u64,
    detail: Arc<CodeContent>,
    detail_title: String,
    sidebar: bool,
    tools_expanded: bool,
    agent_details: bool,
    repository_details: bool,
    dashboard_hours: u64,
    dashboard_minute: Option<u64>,
    dashboard_all_slice_agents: bool,
    dashboard_expanded: bool,
    dashboard_list: bool,
    viewer_font_sizes: [f32; 9],
    graph: agent_graph::GraphState,
    editor: workspace_editor::EditorState,
    chat: chat_panel::ChatState,
    tools: tools_panel::ToolsState,
    catalog: agent_catalog::Catalog,
    catalog_tab: usize,
    catalog_scope: String,
    catalog_selected: Option<String>,
    catalog_loading: bool,
    catalog_detail: Entity<CodeView>,
    catalog_generation: u64,
    catalog_workspace: String,
    catalog_worker: LatestWorker<(u64, String)>,
    launch_skills: Vec<String>,
    launch_skill_picker: bool,
    launch_skill_search: Entity<TextInput>,
    next_lifecycle_tick: Instant,
    launch_task: Entity<TextInput>,
    launch_title: Entity<TextInput>,
    launch_workspace: Entity<TextInput>,
    launch_write: bool,
    launch_acknowledged: bool,
    launch_minutes: u64,
    launch_error: String,
    engine: agent_engine::Engine,
    engine_revision: u64,
    quitting: bool,
    engine_runs: Vec<agent_engine::RunSnapshot>,
    context: Option<agent_context::AgentContext>,
    context_requested: Option<(String, String, u64)>,
    context_worker: LatestWorker<((String, String, u64), Session, Snapshot)>,
    context_guidance_open: bool,
    context_skills_open: bool,
    context_document: Option<(String, Arc<CodeContent>)>,
    context_view: Entity<CodeView>,
    dock: bool,
    dock_tab: usize,
    log_mode: bool,
    selected_pid: Option<u32>,
    selected_process: Option<ProcessInfo>,
    confirmation: Option<ProcessInfo>,
    worker_confirmation: bool,
    epoch: u64,
    diff_revision: u64,
    loading: bool,
    operation_busy: bool,
    message: String,
    updated: Instant,
    next_refresh: Instant,
    next_live_refresh: Instant,
    follow_activity: bool,
    event_revision: usize,
    last_event: Option<Instant>,
    telemetry_duration: Duration,
    rx: mpsc::Receiver<Message>,
    repo_worker: LatestWorker<TaggedSettings>,
    status_worker: LatestWorker<StatusJob>,
    telemetry_worker: LatestWorker<TelemetryJob>,
    history_worker: LatestWorker<Option<String>>,
    activity_worker: LatestWorker<Vec<Session>>,
    terminal_worker: LatestWorker<(u64, Session, bool)>,
    file_worker: LatestWorker<FileJob>,
    diff_worker: LatestWorker<DiffJob>,
    commit_worker: LatestWorker<DiffJob>,
    operation_worker: LatestWorker<(u64, Settings, String, Option<ProcessInfo>)>,
    record_scroll: UniformListScrollHandle,
    drag: Option<(usize, f32)>,
    increment_width: f32,
}
#[track_caller]
fn action_identity(label: &str) -> SharedString {
    let site = std::panic::Location::caller();
    format!(
        "action:{}:{}:{}:{label}",
        site.file(),
        site.line(),
        site.column()
    )
    .into()
}
#[track_caller]
fn action(
    label: impl Into<SharedString>,
    p: Palette,
    handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let label: SharedString = label.into();
    div()
        .id(action_identity(label.as_ref()))
        .flex()
        .items_center()
        .justify_center()
        .h(px(26.))
        .flex_shrink_0()
        .px(px(8.))
        .rounded(px(3.))
        .gap(px(5.))
        .border_1()
        .border_color(rgba(0x00000000))
        .cursor_pointer()
        .text_size(px(12.))
        .text_color(rgb(p.muted))
        .hover(move |s| s.bg(rgb(p.selected)).text_color(rgb(p.text)))
        .active(move |s| s.bg(rgb(p.selected)).text_color(rgb(p.text)))
        .on_click(handler)
        .child(label)
}
fn small(text: impl AsRef<str>, color: u32) -> Div {
    div()
        .text_size(px(11.))
        .text_color(rgb(color))
        .child(text.as_ref().to_owned())
}
fn line(text: impl AsRef<str>, color: u32) -> Div {
    div()
        .text_size(px(13.))
        .text_color(rgb(color))
        .child(text.as_ref().to_owned())
}
fn heading(title: &str, subtitle: impl AsRef<str>, p: Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .gap(px(5.))
        .py(px(15.))
        .px(px(18.))
        .border_b_1()
        .border_color(rgb(p.border))
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(16.))
                .child(title.to_owned()),
        )
        .child(line(subtitle, p.muted))
}
fn name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.into())
}
fn short_id(id: &str) -> String {
    id.strip_prefix("session-")
        .unwrap_or(id)
        .chars()
        .take(8)
        .collect()
}
fn source_label(source: &str) -> &str {
    match source {
        "codex" => "Codex",
        "deepseek" => "DeepSeek",
        "opencode" => "OpenCode",
        _ => "Agent",
    }
}
fn agent_label(session: &Session) -> String {
    if session.id.starts_with("graph/") && session.agent == "Graph launch" {
        return session.title.clone();
    }

    session
        .agent
        .split(" · ")
        .find(|part| part.starts_with('/'))
        .map(name)
        .or_else(|| {
            session
                .agent
                .split(" · ")
                .find(|s| !s.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| name(&session.project))
}
fn record_matches_outcome(record: &Record, outcome: Option<ActivityKind>) -> bool {
    outcome.is_none_or(|kind| agentwatcher::activity::classify(record) == kind)
}
fn outcome_label(kind: ActivityKind) -> &'static str {
    match kind {
        ActivityKind::Completed => "Completed tools",
        ActivityKind::Error => "Error events",
        ActivityKind::Tool => "Other tool events",
        _ => "Events",
    }
}
fn record_in_time_range(time: &str, range: Option<(u64, u64)>) -> bool {
    range.is_none_or(|(start, end)| {
        agentwatcher::activity::timestamp_seconds(time).is_some_and(|at| at >= start && at < end)
    })
}
fn record_color(record: &Record, p: Palette) -> u32 {
    match record.tool_status.as_str() {
        "Completed" => p.green,
        "Error" => p.red,
        _ => match record.kind.as_str() {
            "user" => p.accent,
            "assistant" => p.syntax_keyword,
            _ => p.faint,
        },
    }
}
fn record_label(record: &Record) -> &str {
    match record.kind.as_str() {
        "user" => "You",
        "assistant" => "Agent",
        "tool" => "Tool",
        other => other,
    }
}
fn trace_preview(record: &Record, p: &Preferences) -> String {
    let text = if !p.trace_previews {
        record_label(record).into()
    } else if record.kind == "tool" {
        visuals::tool_summary_line(record)
    } else if record.is_infrastructure {
        "Recorded context · select to inspect".into()
    } else {
        visuals::display_task(&record.summary)
    };
    text.chars().take(p.trace_preview_chars).collect()
}
fn display_time(value: &str) -> String {
    let seconds = if let Ok(number) = value.parse::<i64>() {
        Some(if number >= 1_000_000_000_000 {
            number / 1000
        } else {
            number
        })
    } else if value.ends_with('Z') {
        std::ffi::CString::new(value).ok().and_then(|input| unsafe {
            let mut parts = std::mem::zeroed::<libc::tm>();
            (!libc::strptime(input.as_ptr(), c"%Y-%m-%dT%H:%M:%S".as_ptr(), &mut parts).is_null())
                .then(|| libc::timegm(&mut parts))
        })
    } else {
        None
    };
    seconds
        .and_then(|seconds| unsafe {
            let mut parts = std::mem::zeroed::<libc::tm>();
            if libc::localtime_r(&seconds, &mut parts).is_null() {
                return None;
            }
            Some(format!(
                "{:02}:{:02}:{:02}",
                parts.tm_hour, parts.tm_min, parts.tm_sec
            ))
        })
        .unwrap_or_else(|| value.chars().take(24).collect())
}
fn lines(s: String) -> Arc<CodeContent> {
    Arc::new(CodeContent::new(s))
}
fn empty(title: &str, body: &str, p: Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .flex_1()
        .gap(px(9.))
        .p(px(32.))
        .child(line(title.to_owned(), p.text))
        .child(line(body.to_owned(), p.muted))
}
impl Workspace {
    fn new(settings: Settings, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let launch_skill_search = cx.new(|cx| TextInput::new("Search installed skills…", cx));
        let launch_task = cx.new(|cx| TextInput::new("What should this agent do?", cx));
        let launch_title = cx.new(|cx| TextInput::new("Name this agent (optional)", cx));
        let launch_workspace = cx.new(|cx| TextInput::new("Workspace folder", cx));
        let search = cx.new(|cx| TextInput::new("Search recorded work…", cx));
        let palette_input = cx.new(|cx| TextInput::new("Search commands…", cx));
        let project_input = cx.new(|cx| TextInput::new("Project folder", cx));
        let worker_input = cx.new(|cx| TextInput::new("Worker script", cx));
        let state_input = cx.new(|cx| TextInput::new("State folder", cx));
        project_input.update(cx, |v, cx| v.set_value(settings.project.clone(), cx));
        worker_input.update(cx, |v, cx| v.set_value(settings.worker.clone(), cx));
        state_input.update(cx, |v, cx| v.set_value(settings.state_dir.clone(), cx));
        let subscriptions = vec![
            cx.subscribe(&launch_skill_search, |_, _, _: &input::Changed, cx| {
                cx.notify()
            }),
            cx.subscribe(&launch_task, |this, _, _: &input::Changed, cx| {
                this.launch_error.clear();
                if this.settings.preferences.launch_title_from_task {
                    let title = this
                        .launch_task
                        .read(cx)
                        .value()
                        .chars()
                        .take(120)
                        .collect::<String>();
                    this.launch_title.update(cx, |v, cx| v.set_value(title, cx));
                }
                cx.notify();
            }),
            cx.subscribe(&launch_workspace, |this, _, _: &input::Changed, cx| {
                this.launch_error.clear();
                cx.notify();
            }),
            cx.subscribe(&launch_title, |this, _, _: &input::Changed, cx| {
                this.launch_error.clear();
                cx.notify();
            }),
            cx.subscribe(&search, |this, input, _: &input::Changed, cx| {
                this.query = input.read(cx).value().to_lowercase();
                this.filter_records();
                cx.notify();
            }),
            cx.subscribe(&palette_input, |this, input, _: &input::Changed, cx| {
                this.palette_query = input.read(cx).value().to_lowercase();
                this.palette_index = 0;
                cx.notify();
            }),
        ];
        let (tx, rx) = mpsc::channel();
        let editor = workspace_editor::EditorState::new(tx.clone());
        let chat = chat_panel::ChatState::new(tx.clone(), window, cx);
        let tools = tools_panel::ToolsState::new(tx.clone(), window, cx);
        let out = tx.clone();
        let repo_worker = LatestWorker::new(move |(epoch, settings): TaggedSettings| {
            let result = backend::repository(&settings.project);
            let _ = out.send(Message::Repository(epoch, result));
        });
        let out = tx.clone();
        let status_worker = LatestWorker::new(move |(epoch, settings, selected): StatusJob| {
            let (mut worker, processes) = backend::status_snapshot(&settings);
            worker.log = telemetry::redact(&worker.log);
            let processes = processes
                .map(|mut rows| {
                    for process in &mut rows {
                        process.command = telemetry::redact(&process.command);
                    }
                    rows
                })
                .map_err(|error| telemetry::redact(&error));
            let agent_process = selected.as_ref().and_then(|session| {
                processes
                    .as_ref()
                    .ok()
                    .and_then(|rows| agent_control::session_process(session, rows))
            });
            let _ = out.send(Message::Status(epoch, worker, processes, agent_process));
        });
        let out = tx.clone();
        let mut last_read = None;
        let telemetry_worker = LatestWorker::new(
            move |(epoch, project, source, id, retained, before, catalog): TelemetryJob| {
                let stamp = retained
                    .as_ref()
                    .and_then(|session| std::fs::metadata(&session.path).ok())
                    .map(|meta| (meta.len(), meta.modified().ok()));
                let read_key = (epoch, source.clone(), id.clone(), before, stamp);
                if !catalog && (retained.is_none() || last_read.as_ref() == Some(&read_key)) {
                    return;
                }
                last_read = Some(read_key);
                let start = Instant::now();
                let mut catalog_error = None;
                let sessions = if !catalog {
                    vec![]
                } else if source == "opencode" {
                    match telemetry::open_code_sessions(&project) {
                        Ok(sessions) => sessions,
                        Err(error) => {
                            catalog_error = Some(telemetry::redact(&error));
                            vec![]
                        }
                    }
                } else {
                    telemetry::discover(&project, &source)
                };
                let selected = select_recording(&source, &id, retained.as_ref(), &sessions);
                let mut snapshot = selected
                    .map(|session| {
                        if session.source.eq_ignore_ascii_case("opencode") {
                            telemetry::open_code_collect(&project, &session.id)
                        } else if let Some(before) = before {
                            telemetry::collect_before(session, before)
                        } else {
                            telemetry::collect(session)
                        }
                    })
                    .unwrap_or_default();
                if let Some(error) = catalog_error {
                    snapshot.error = Some(error);
                }
                let record_search = snapshot
                    .records
                    .iter()
                    .map(|record| {
                        format!("{} {} {}", record.kind, record.summary, record.detail)
                            .to_lowercase()
                    })
                    .collect();
                let _ = out.send(Message::Telemetry(
                    epoch,
                    sessions,
                    snapshot,
                    record_search,
                    start.elapsed(),
                ));
            },
        );
        let out = tx.clone();
        let history_worker = LatestWorker::new(move |cursor: Option<String>| {
            let page = telemetry::discover_history("", "automatic", cursor.as_deref(), 64);
            let _ = out.send(Message::History(page));
        });
        let out = tx.clone();
        let mut collector = ActivityCollector::default();
        let activity_worker = LatestWorker::new(move |sessions: Vec<Session>| {
            let _ = out.send(Message::Activities(collector.poll(&sessions)));
        });
        let out = tx.clone();
        let terminal_worker =
            LatestWorker::new(move |(epoch, session, fresh): (u64, Session, bool)| {
                let result = agent_control::terminal_plan(&session, fresh).and_then(|plan| {
                    let home = std::env::var_os("HOME").ok_or("Home directory unavailable")?;
                    let dir = std::path::PathBuf::from(home)
                        .join("Library/Application Support/AgentWatcher/terminal-plans");
                    agent_control::write_plan_in(&plan, &dir)
                });
                let _ = out.send(Message::Terminal(epoch, result));
            });
        let out = tx.clone();
        let diff_worker =
            LatestWorker::new(move |(epoch, revision, project, key, commit): DiffJob| {
                let result = if commit {
                    backend::commit_diff(&project, &key)
                } else {
                    backend::file_diff(&project, &key)
                }
                .map(|s| telemetry::redact(&s));
                let _ = out.send(Message::Diff(epoch, revision, key, commit, result));
            });
        let out = tx.clone();
        let commit_worker =
            LatestWorker::new(move |(epoch, revision, project, key, commit): DiffJob| {
                let result = backend::commit_diff(&project, &key).map(|s| telemetry::redact(&s));
                let _ = out.send(Message::Diff(epoch, revision, key, commit, result));
            });
        let file_out = tx.clone();
        let file_worker = LatestWorker::new(move |job: FileJob| match job {
            None => {
                let _ = file_out.send(Message::FileTargets(file_actions::available_targets()));
            }
            Some((epoch, project, path, target)) => {
                let result = file_actions::open_file(&project, &path, target);
                let _ = file_out.send(Message::FileOpened(epoch, path, result));
            }
        });
        let catalog_out = tx.clone();
        let catalog_worker = LatestWorker::new(move |(generation, project): (u64, String)| {
            let result = agent_catalog::collect(&project);
            let _ = catalog_out.send(Message::Catalog(generation, result));
        });
        let context_out = tx.clone();
        let context_worker = LatestWorker::new(
            move |(key, session, snapshot): ((String, String, u64), Session, Snapshot)| {
                let result = agent_context::collect(&session, &snapshot);
                let _ = context_out.send(Message::AgentContext(key, result));
            },
        );
        let operation_worker = LatestWorker::new(
            move |(epoch, settings, operation, pid): (
                u64,
                Settings,
                String,
                Option<ProcessInfo>,
            )| {
                let result = if let Some(pid) = pid {
                    backend::stop_process(&settings, &pid)
                } else {
                    backend::worker_action(&settings, &operation)
                };
                let _ = tx.send(Message::Operation(epoch, result));
            },
        );
        let preferences = settings.preferences.clone();
        let mut this = Self {
            settings,
            settings_writer: PersistenceWorker::new(Settings::save),
            settings_progress: PersistenceStatus::default(),
            settings_submission_error: None,
            settings_quit_discard: false,
            applied_preferences: Preferences::default(),
            pending_graph_record: None,
            pane: preferences.startup_view,
            history: vec![preferences.startup_view],
            history_index: 0,
            focus: cx.focus_handle(),
            search,
            palette_input,
            project_input,
            worker_input,
            state_input,
            _subscriptions: subscriptions,
            working_view: cx.new(|cx| CodeView::new(window, cx)),
            commit_view: cx.new(|cx| CodeView::new(window, cx)),
            dock_view: cx.new(|cx| CodeView::new(window, cx)),
            detail_view: cx.new(|cx| CodeView::new(window, cx)),
            visual_detail: cx.new(EventView::new),
            evidence_mode: EvidenceMode::Visual,
            file_targets: Vec::new(),
            file_menu: None,
            log_view: cx.new(|cx| CodeView::new(window, cx)),
            query: String::new(),
            palette_query: String::new(),
            palette_open: false,
            palette_index: 0,
            repository: RepositorySnapshot::default(),
            repository_loading: true,
            worker: WorkerState::default(),
            worker_log: lines(String::new()),
            process_list: vec![],
            agent_process: None,
            process_error: String::new(),
            sessions: vec![],
            known_sessions: vec![],
            agent_activity: vec![],
            activity_changes: std::collections::HashMap::new(),
            history_cursor: None,
            history_loading: true,
            history_complete: false,
            history_project_only: false,
            history_source: 0,
            history_warnings: vec![],
            snapshot: Snapshot::default(),
            record_search: vec![],
            trace_windows: vec![None],
            trace_index: 0,
            trace_time_range: None,
            trace_outcome: None,
            source: "automatic".into(),
            session_id: String::new(),
            record_mode: 0,
            records: vec![],
            selected_record: None,
            selected_commit: String::new(),
            selected_file: String::new(),
            diff: lines(String::new()),
            diff_title: String::new(),
            commit_diff: lines(String::new()),
            commit_title: String::new(),
            commit_revision: 0,
            detail: lines(String::new()),
            detail_title: String::new(),
            sidebar: true,
            tools_expanded: false,
            agent_details: false,
            repository_details: false,
            dashboard_hours: 24,
            dashboard_minute: None,
            dashboard_all_slice_agents: false,
            dashboard_expanded: false,
            dashboard_list: false,
            viewer_font_sizes: [13.; 9],
            graph: agent_graph::GraphState::default(),
            editor,
            chat,
            tools,
            catalog: agent_catalog::Catalog::default(),
            catalog_tab: 0,
            catalog_scope: "All".into(),
            catalog_selected: None,
            catalog_loading: true,
            catalog_detail: cx.new(|cx| CodeView::new(window, cx)),
            catalog_generation: 0,
            catalog_workspace: String::new(),
            catalog_worker,
            launch_skills: Vec::new(),
            launch_skill_picker: false,
            launch_skill_search,
            next_lifecycle_tick: Instant::now(),
            launch_task,
            launch_title,
            launch_workspace,
            launch_write: false,
            launch_acknowledged: false,
            launch_minutes: 10,
            launch_error: String::new(),
            engine: agent_engine::Engine::new(),
            engine_revision: 0,
            quitting: false,
            engine_runs: Vec::new(),
            context: None,
            context_requested: None,
            context_worker,
            context_guidance_open: true,
            context_skills_open: false,
            context_document: None,
            context_view: cx.new(|cx| CodeView::new(window, cx)),
            dock: false,
            dock_tab: 0,
            log_mode: false,
            selected_pid: None,
            selected_process: None,
            confirmation: None,
            worker_confirmation: false,
            epoch: 0,
            diff_revision: 0,
            loading: true,
            operation_busy: false,
            message: String::new(),
            updated: Instant::now(),
            next_refresh: Instant::now() + Duration::from_secs(10),
            next_live_refresh: Instant::now() + Duration::from_secs(1),
            follow_activity: true,
            event_revision: 0,
            last_event: None,
            telemetry_duration: Duration::ZERO,
            rx,
            repo_worker,
            status_worker,
            telemetry_worker,
            history_worker,
            activity_worker,
            terminal_worker,
            file_worker,
            diff_worker,
            commit_worker,
            operation_worker,
            record_scroll: UniformListScrollHandle::new(),
            drag: None,
            increment_width: 260.,
        };
        let owner = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            owner
                .update(cx, |this, cx| this.request_engine_quit(cx))
                .is_err()
        });
        for view in [
            &this.detail_view,
            &this.log_view,
            &this.context_view,
            &this.catalog_detail,
        ] {
            view.update(cx, |v, cx| v.set_diff_highlights(false, cx));
        }
        for view in [
            &this.working_view,
            &this.commit_view,
            &this.dock_view,
            &this.detail_view,
            &this.log_view,
            &this.context_view,
            &this.catalog_detail,
        ] {
            this._subscriptions.push(cx.subscribe(
                view,
                |this, entity, event: &code_view::AskContext, cx| {
                    this.open_pane_chat(
                        format!("viewer:{:?}:{}", entity.entity_id(), event.key),
                        event.title.clone(),
                        event.text.clone(),
                        cx,
                    )
                },
            ));
        }
        this._subscriptions.push(cx.subscribe(
            &this.visual_detail,
            |this, entity, event: &code_view::AskContext, cx| {
                this.open_pane_chat(
                    format!("viewer:{:?}:{}", entity.entity_id(), event.key),
                    event.title.clone(),
                    event.text.clone(),
                    cx,
                )
            },
        ));
        this.request_catalog(cx);
        this.file_worker.request(None);
        this.apply_preferences(cx);
        this.history_worker.request(None);
        this.refresh(cx);
        window.focus(&this.focus, cx);
        cx.spawn_in(window, async move |this, cx| {
            loop {
                smol::Timer::after(Duration::from_millis(100)).await;
                if this
                    .update_in(cx, |this, window, cx| this.poll(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        this
    }
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.next_refresh = Instant::now()
            + Duration::from_secs(self.settings.preferences.repository_refresh_seconds);
        self.repo_worker
            .request((self.epoch, self.settings.clone()));
        self.status_worker.request((
            self.epoch,
            self.settings.clone(),
            self.session_by_id(&self.session_id).cloned(),
        ));
        if self.engine_run(&self.source, &self.session_id).is_none() {
            self.telemetry_worker.request((
                self.epoch,
                self.settings.project.clone(),
                self.source.clone(),
                self.session_id.clone(),
                self.session_by_id(&self.session_id).cloned(),
                self.trace_windows[self.trace_index],
                true,
            ));
        }
        if !self.selected_file.is_empty() && (self.dock || self.pane == 4) {
            self.request_diff(false, cx);
        }
        cx.notify();
    }
    fn manual_refresh(&mut self, cx: &mut Context<Self>) {
        self.context_requested = None;
        self.request_agent_context(cx);
        self.refresh(cx);
        self.activity_worker.request(self.activity_sessions());
        if !self.history_loading {
            self.history_loading = true;
            self.history_worker.request(None);
        }
    }
    fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.poll_graph_engine(cx);
        let settings_changed = self.poll_settings_persistence();
        if self.quitting
            && self.engine.is_idle()
            && self.tools.terminal.is_idle()
            && !self.editor.is_busy()
            && self.settings_quit_ready()
        {
            cx.quit();
            return;
        }
        let messages: Vec<_> = self.rx.try_iter().collect();
        let mut changed = self.tools.poll(window, cx) || !messages.is_empty() || settings_changed;
        for message in messages {
            match message {
                Message::Directory(nonce, root, relative, result) => {
                    self.receive_directory(nonce, root, relative, result, cx)
                }
                Message::Editor(nonce, result) => self.receive_editor(nonce, result, window, cx),
                Message::Chat(nonce, scope, result) => self.receive_chat(nonce, scope, result, cx),
                Message::Catalog(generation, catalog) if generation == self.catalog_generation => {
                    self.catalog = catalog;
                    self.catalog_loading = false;
                    if let Some(id) = self.catalog_selected.clone() {
                        self.catalog_select(id, cx);
                    }
                }
                Message::AgentContext(key, context)
                    if self.context_requested.as_ref() == Some(&key) =>
                {
                    self.context = Some(context);
                }
                Message::FileTargets(targets) => {
                    self.file_targets = targets;
                }
                Message::FileOpened(epoch, path, result) if epoch == self.epoch => {
                    self.message = match result {
                        Ok(()) => format!("Opened {path}"),
                        Err(error) => error,
                    };
                }
                Message::Repository(epoch, repo) if epoch == self.epoch => {
                    self.repository_loading = false;
                    self.repository = repo;
                    if self.selected_file.is_empty() {
                        if let Some(file) = self.repository.changes.first() {
                            self.selected_file = file.path.clone();
                            if self.dock || self.pane == 4 {
                                self.request_diff(false, cx);
                            }
                        }
                    }
                    if self.selected_commit.is_empty() {
                        if let Some(commit) = self.repository.commits.first() {
                            self.selected_commit = commit.hash.clone();
                            if self.pane == 3 {
                                self.request_diff(true, cx);
                            }
                        }
                    }
                }
                Message::Status(epoch, worker, processes, agent_process) if epoch == self.epoch => {
                    self.agent_process = agent_process;
                    self.worker_log = lines(worker.log.clone());
                    self.worker = worker;
                    match processes {
                        Ok(rows) => {
                            self.process_list = rows;
                            self.process_error.clear();
                        }
                        Err(error) => {
                            self.process_list.clear();
                            self.process_error = error;
                        }
                    }
                }
                Message::Telemetry(epoch, sessions, mut snapshot, record_search, duration)
                    if epoch == self.epoch =>
                {
                    if !sessions.is_empty() {
                        self.merge_sessions(&sessions);
                        self.sessions = sessions;
                    }
                    self.telemetry_duration = duration;
                    self.loading = false;
                    for child in self.known_sessions.iter().filter(|s| {
                        s.source == snapshot.session.source && s.parent_id == snapshot.session.id
                    }) {
                        if !snapshot.children.iter().any(|c| c.session_id == child.id) {
                            snapshot.children.push(ChildAgent {
                                id: child.id.clone(),
                                title: child.title.clone(),
                                session_id: child.id.clone(),
                                agent: child.agent.clone(),
                                status: "Recorded".into(),
                            });
                        }
                    }
                    let same_session = self.snapshot.session.id == snapshot.session.id
                        && self.snapshot.session.source == snapshot.session.source;
                    let signature =
                        |s: &Snapshot| agentwatcher::activity::event_signature(&s.records);
                    if same_session
                        && self.trace_index == 0
                        && signature(&self.snapshot) != signature(&snapshot)
                    {
                        self.event_revision += 1;
                        self.last_event = Some(Instant::now());
                    }
                    let selected_id = self
                        .selected_record
                        .and_then(|i| self.snapshot.records.get(i))
                        .map(|r| r.id.clone());
                    self.session_id = snapshot.session.id.clone();
                    self.snapshot = snapshot;
                    self.record_search = record_search;
                    self.selected_record = if same_session {
                        retained_record_index(&self.snapshot.records, selected_id.as_deref())
                    } else {
                        None
                    };
                    if let Some(index) = self.selected_record {
                        let record = &self.snapshot.records[index];
                        self.detail_title = record.kind.clone();
                        self.detail = lines(record.detail.clone());
                        self.visual_detail
                            .update(cx, |view, cx| view.set_record(record, cx));
                    } else if selected_id.is_some() {
                        self.detail_title = "Record outside the recent window".into();
                        self.detail = lines(String::new());
                    }
                    if let Some((source, agent_id, record_id)) = self.pending_graph_record.clone() {
                        if self.snapshot.session.source == source
                            && self.snapshot.session.id == agent_id
                        {
                            self.pending_graph_record = None;
                            if let Some(index) =
                                self.snapshot.records.iter().position(|r| r.id == record_id)
                            {
                                self.inspect_record(index, cx);
                            } else {
                                self.message = "This event is outside the loaded recording window. Open the full trace to browse earlier work.".into();
                            }
                        }
                    }
                    self.request_agent_context(cx);
                    self.filter_records();
                    if self.follow_activity && self.trace_index == 0 && !self.records.is_empty() {
                        self.record_scroll
                            .scroll_to_item(self.records.len() - 1, ScrollStrategy::Bottom);
                    }
                }
                Message::Diff(epoch, revision, key, commit, result) if epoch == self.epoch => {
                    if commit && revision == self.commit_revision {
                        self.commit_title = key;
                        self.commit_diff = lines(
                            result.unwrap_or_else(|e| format!("Unable to read commit diff\n{e}")),
                        );
                    } else if !commit && revision == self.diff_revision {
                        self.diff_title = key;
                        self.diff = lines(
                            result.unwrap_or_else(|e| format!("Unable to read file diff\n{e}")),
                        );
                    }
                }
                Message::Terminal(epoch, result) if epoch == self.epoch => match result {
                    Ok(path) => {
                        cx.open_with_system(&path);
                        self.message =
                            "Agent opened in Terminal. Enter an instruction to continue.".into();
                    }
                    Err(error) => self.message = error,
                },
                Message::History(page) => {
                    self.merge_sessions(&page.sessions);
                    self.history_cursor = page.next_cursor;
                    self.history_complete = page.complete;
                    self.history_warnings = page.warnings;
                    self.history_loading = false;
                }
                Message::Operation(epoch, result) => {
                    self.operation_busy = false;
                    if epoch == self.epoch {
                        self.message = result
                            .err()
                            .unwrap_or_else(|| "Worker command completed".into());
                    }
                    self.refresh(cx);
                }
                Message::Activities(activities) => {
                    for activity in &activities {
                        if activity.changed {
                            self.activity_changes
                                .insert(activity.key.clone(), Instant::now());
                        }
                    }
                    self.activity_changes.retain(|key, at| {
                        at.elapsed() < Duration::from_secs(3)
                            && activities.iter().any(|a| &a.key == key)
                    });
                    self.agent_activity = activities;
                    self.integrate_engine_activity();
                }
                _ => {}
            }
        }
        // Native termination stays deferred while a file operation completes.
        // Resume only after consuming its result; do not reopen a dismissed or
        // already visible confirmation on every poll.
        if should_resume_native_quit(
            cx.native_quit_pending(),
            self.quitting,
            self.editor.is_busy(),
            self.editor.quit_confirm,
        ) {
            self.request_engine_quit(cx);
            changed = true;
        }
        if Instant::now() >= self.next_lifecycle_tick {
            self.next_lifecycle_tick = Instant::now() + Duration::from_secs(1);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            for activity in &mut self.agent_activity {
                let lifecycle = activity.lifecycle_at(now);
                if lifecycle != activity.lifecycle {
                    activity.lifecycle = lifecycle;
                    changed = true;
                }
            }
        }
        let previous_changes = self.activity_changes.len();
        self.activity_changes
            .retain(|_, at| at.elapsed() < Duration::from_secs(3));
        if previous_changes != self.activity_changes.len() {
            changed = true;
        }
        if self
            .last_event
            .is_some_and(|at| at.elapsed() >= Duration::from_secs(3))
        {
            self.last_event = None;
            changed = true;
        }
        if changed {
            self.updated = Instant::now();
            cx.notify();
        }
        if self.settings.preferences.auto_refresh && Instant::now() >= self.next_refresh {
            self.refresh(cx);
        }
        if self.settings.preferences.auto_refresh && Instant::now() >= self.next_live_refresh {
            self.next_live_refresh = Instant::now()
                + Duration::from_secs(self.settings.preferences.activity_refresh_seconds);
            self.activity_worker
                .request_if_no_pending(self.activity_sessions());
            if !self.session_id.is_empty()
                && self.source != "opencode"
                && self.engine_run(&self.source, &self.session_id).is_none()
            {
                self.telemetry_worker.request_if_no_pending((
                    self.epoch,
                    self.settings.project.clone(),
                    self.source.clone(),
                    self.session_id.clone(),
                    self.session_by_id(&self.session_id).cloned(),
                    self.trace_windows[self.trace_index],
                    false,
                ));
            }
            cx.notify();
        }
    }
    fn merge_sessions(&mut self, sessions: &[Session]) {
        for session in sessions {
            if let Some(existing) = self
                .known_sessions
                .iter_mut()
                .find(|s| s.id == session.id && s.source == session.source)
            {
                *existing = session.clone();
            } else {
                self.known_sessions.push(session.clone());
            }
        }
        self.known_sessions.sort_by(|a, b| {
            b.updated
                .parse::<u64>()
                .unwrap_or(0)
                .cmp(&a.updated.parse::<u64>().unwrap_or(0))
                .then(a.id.cmp(&b.id))
        });
    }
    fn load_history(&mut self, cx: &mut Context<Self>) {
        if self.history_loading {
            return;
        }
        self.history_loading = true;
        self.history_worker.request(if self.history_complete {
            None
        } else {
            self.history_cursor.clone()
        });
        cx.notify();
    }
    fn trace_page(&mut self, older: bool, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        if older {
            if !self.snapshot.has_earlier {
                return;
            }
            self.trace_windows.truncate(self.trace_index + 1);
            self.trace_windows.push(Some(self.snapshot.window_start));
            self.trace_index += 1;
        } else {
            if self.trace_index == 0 {
                return;
            }
            self.trace_index -= 1;
        }
        self.clear_details();
        self.loading = true;
        self.epoch += 1;
        self.record_scroll = UniformListScrollHandle::new();
        self.refresh(cx);
    }
    fn filter_records(&mut self) {
        self.records = self
            .snapshot
            .records
            .iter()
            .enumerate()
            .filter(|(index, r)| {
                let mode = match self.record_mode {
                    0 => !r.is_infrastructure,
                    1 => r.kind.to_lowercase().contains("tool"),
                    _ => true,
                };
                let mode = mode
                    && record_in_time_range(&r.time, self.trace_time_range)
                    && record_matches_outcome(r, self.trace_outcome);
                if self.query.is_empty() {
                    mode
                } else {
                    mode && self
                        .record_search
                        .get(*index)
                        .is_some_and(|text| text.contains(&self.query))
                }
            })
            .map(|(i, _)| i)
            .collect();
    }
    fn navigate(&mut self, pane: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.file_menu = None;
        self.editor.open = false;
        if pane != self.pane {
            self.history.truncate(self.history_index + 1);
            self.history.push(pane);
            self.history_index = self.history.len() - 1;
        }
        self.pane = pane;
        if pane == 8 && self.catalog_workspace != self.settings.project {
            self.request_catalog(cx);
        }
        if (2..=5).contains(&pane) {
            self.tools_expanded = true;
        }
        self.query.clear();
        self.search.update(cx, |v, cx| v.set_value("", cx));
        if pane == 3 && !self.selected_commit.is_empty() {
            self.request_diff(true, cx);
        } else if pane == 4 || self.dock {
            if !self.selected_file.is_empty() {
                self.request_diff(false, cx);
            }
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }
    fn history_move(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        self.file_menu = None;
        self.editor.open = false;
        let index = (self.history_index as isize + delta)
            .clamp(0, self.history.len().saturating_sub(1) as isize) as usize;
        self.history_index = index;
        self.pane = self.history[index];
        if self.pane == 8 && self.catalog_workspace != self.settings.project {
            self.request_catalog(cx);
        }
        if (2..=5).contains(&self.pane) {
            self.tools_expanded = true;
        }
        if self.pane == 3 {
            self.request_diff(true, cx);
        } else {
            self.request_diff(false, cx);
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }
    fn request_diff(&mut self, commit: bool, cx: &mut Context<Self>) {
        let key = if commit {
            self.selected_commit.clone()
        } else {
            self.selected_file.clone()
        };
        if key.is_empty() {
            return;
        }
        if commit {
            self.commit_revision += 1;
            if self.commit_title != key {
                self.commit_title = key.clone();
                self.commit_diff = lines("Reading commit…".into());
            }
            self.commit_worker.request((
                self.epoch,
                self.commit_revision,
                self.settings.project.clone(),
                key,
                true,
            ));
        } else {
            self.diff_revision += 1;
            if self.diff_title != key {
                self.diff_title = key.clone();
                self.diff = lines("Reading changes…".into());
            }
            self.diff_worker.request((
                self.epoch,
                self.diff_revision,
                self.settings.project.clone(),
                key,
                false,
            ));
        }
        cx.notify();
    }
    fn toggle_dock(&mut self, cx: &mut Context<Self>) {
        if self.pane == 7 {
            self.graph.inspector_open = (self.graph.creating_agent
                || self.graph.selected.is_some())
                && !self.graph.inspector_open;
            cx.notify();
            return;
        }
        self.dock = !self.dock;
        if self.dock && self.dock_tab == 0 {
            self.request_diff(false, cx);
        }
        cx.notify();
    }
    fn inspect_record(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(record) = self.snapshot.records.get(index) {
            self.detail_title = record.kind.clone();
            self.detail = lines(record.detail.clone());
            self.visual_detail
                .update(cx, |view, cx| view.set_record(record, cx));
            self.selected_record = Some(index);
            if self.pane != 7 {
                self.dock = true;
                self.dock_tab = 1;
            }

            cx.notify();
        }
    }
    fn open_outcome_trace(
        &mut self,
        source: &str,
        id: String,
        kind: ActivityKind,
        range: Option<(u64, u64)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(
            kind,
            ActivityKind::Completed | ActivityKind::Error | ActivityKind::Tool
        ) || !self
            .known_sessions
            .iter()
            .any(|s| s.source == source && s.id == id)
        {
            return;
        }
        if self.source != source || self.session_id != id {
            self.select_session_key_in(source, id, 1, window, cx);
        } else {
            self.navigate(1, window, cx);
        }
        self.log_mode = false;
        self.clear_details();
        self.record_mode = 2;
        self.trace_time_range = range;
        self.trace_outcome = Some(kind);
        self.record_scroll = UniformListScrollHandle::new();
        // Dashboard counts use the current retained recording, so an older
        // paged trace must not satisfy this click. Tag a fresh read generation.
        if self.trace_index != 0 {
            self.snapshot = Snapshot::default();
            self.record_search.clear();
        }
        self.trace_index = 0;
        self.trace_windows = vec![None];
        self.epoch += 1;
        self.loading = true;
        self.filter_records();
        self.refresh(cx);
        cx.notify();
    }
    /// Selecting a graph node keeps the canvas open and binds its right sidebar
    /// and controls to this exact provider/session identity.
    fn open_graph_agent(
        &mut self,
        source: &str,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .known_sessions
            .iter()
            .any(|s| s.source == source && s.id == id)
        {
            return;
        }
        if self.graph.selected.as_ref() != Some(&(source.to_owned(), id.clone())) {
            self.graph.reset_inspector_scroll();
        }
        self.graph.selected = Some((source.to_owned(), id.clone()));
        self.graph.creating_agent = false;
        self.graph.inspector_open = true;
        if self.source != source || self.session_id != id {
            if self.engine_run(source, &id).is_some() {
                self.source = source.into();
                self.session_id = id.clone();
                self.epoch += 1;
                self.clear_details();
                self.sync_selected_engine(cx);
            } else {
                self.select_session_key_in(source, id, 7, window, cx);
            }
        }
        self.request_agent_context(cx);
        cx.notify();
    }
    fn open_graph_event(
        &mut self,
        source: &str,
        id: String,
        record_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_graph_agent(source, id.clone(), window, cx);
        self.graph.reset_inspector_scroll();
        if record_id.is_empty() {
            return;
        }
        let index = (self.snapshot.session.source == source && self.snapshot.session.id == id)
            .then(|| self.snapshot.records.iter().position(|r| r.id == record_id))
            .flatten();
        self.clear_details();
        if let Some(index) = index {
            self.inspect_record(index, cx);
        } else if self.source == source && self.session_id == id {
            // Activity summaries can arrive before the selected trace worker.
            // A clicked recent stripe explicitly requests the current window.
            if self.trace_index != 0 {
                self.trace_index = 0;
                self.trace_windows = vec![None];
                self.trace_time_range = None;
                self.trace_outcome = None;
            }
            // Every missing stripe gets a click-specific generation. A read
            // already in flight may predate this event and must not resolve it.
            self.epoch += 1;
            self.pending_graph_record = Some((source.to_owned(), id, record_id));
            self.loading = true;
            self.refresh(cx);
            cx.notify();
        }
    }

    fn select_session_key_in(
        &mut self,
        source: &str,
        id: String,
        pane: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .known_sessions
            .iter()
            .any(|s| s.source == source && s.id == id)
        {
            return;
        }
        self.source = source.to_owned();
        if pane == 1 {
            self.log_mode = false;
        }
        self.select_session_in(id, pane, window, cx);
    }
    fn select_session_in(
        &mut self,
        id: String,
        pane: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = self.session_by_id(&id).cloned();
        if let Some(session) = &selected {
            if session.project != self.settings.project {
                self.set_project(session.project.clone(), cx);
            }
            self.source = session.source.clone();
        }
        self.clear_details();
        self.agent_process = None;
        self.last_event = None;
        self.session_id = id;
        self.epoch += 1;
        self.loading = true;
        self.snapshot = Snapshot::default();
        self.trace_time_range = None;
        self.trace_outcome = None;
        self.trace_windows = vec![None];
        self.trace_index = 0;
        self.records.clear();
        self.navigate(pane, window, cx);
        self.refresh(cx);
    }
    fn change_source(&mut self, source: &str, cx: &mut Context<Self>) {
        self.clear_details();
        self.agent_process = None;
        self.last_event = None;
        self.source = source.into();
        self.session_id.clear();
        self.epoch += 1;
        self.loading = true;
        self.selected_record = None;
        self.snapshot = Snapshot::default();
        self.trace_time_range = None;
        self.trace_outcome = None;
        self.trace_windows = vec![None];
        self.trace_index = 0;
        self.records.clear();
        self.refresh(cx);
    }
    fn set_project(&mut self, path: String, cx: &mut Context<Self>) {
        if path.is_empty() {
            return;
        }
        self.file_menu = None;
        self.agent_process = None;
        self.last_event = None;
        self.settings.project = path;
        self.settings.normalize();
        if self.tools.files_open {
            self.refresh_file_tree(cx);
        }
        self.epoch += 1;
        self.diff_revision += 1;
        self.selected_commit.clear();
        self.selected_file.clear();
        self.session_id.clear();
        self.selected_record = None;
        self.repository = RepositorySnapshot::default();
        self.repository_loading = true;
        self.snapshot = Snapshot::default();
        self.trace_time_range = None;
        self.trace_outcome = None;
        self.trace_windows = vec![None];
        self.trace_index = 0;
        self.sessions.clear();
        self.records.clear();
        self.diff = lines(String::new());
        self.diff_title.clear();
        self.commit_diff = lines(String::new());
        self.commit_title.clear();
        self.selected_pid = None;
        self.selected_process = None;
        self.confirmation = None;
        self.worker_confirmation = false;
        self.detail = lines(String::new());
        self.loading = true;
        self.request_catalog(cx);
        self.project_input
            .update(cx, |v, cx| v.set_value(self.settings.project.clone(), cx));
        self.persist();
        self.refresh(cx);
    }
    fn apply_preferences(&mut self, cx: &mut Context<Self>) {
        self.settings.preferences.normalize();
        let p = self.settings.preferences.clone();
        let old = &self.applied_preferences;
        if p.show_sidebar != old.show_sidebar {
            self.sidebar = p.show_sidebar;
        }
        if p.open_inspector != old.open_inspector {
            self.dock = p.open_inspector;
        }
        if p.code_font_size != old.code_font_size {
            self.visual_detail.update(cx, |view, cx| {
                view.set_default_font_size(p.code_font_size, cx)
            });
            for view in [
                &self.working_view,
                &self.commit_view,
                &self.dock_view,
                &self.detail_view,
                &self.log_view,
                &self.context_view,
                &self.catalog_detail,
            ] {
                view.update(cx, |view, cx| {
                    view.set_default_font_size(p.code_font_size, cx)
                });
            }
        }
        for view in [
            &self.working_view,
            &self.commit_view,
            &self.dock_view,
            &self.detail_view,
            &self.log_view,
            &self.context_view,
            &self.catalog_detail,
        ] {
            view.update(cx, |v, cx| v.set_zoom_step(p.viewer_zoom_step, cx));
        }
        self.visual_detail
            .update(cx, |v, cx| v.set_zoom_step(p.viewer_zoom_step, cx));
        if p.trace_font_size != old.trace_font_size {
            self.viewer_font_sizes[1] = p.trace_font_size;
        }
        if p.dashboard_hours != old.dashboard_hours {
            self.dashboard_hours = p.dashboard_hours;
            self.dashboard_minute = None;
        }
        if p.follow_latest != old.follow_latest {
            self.follow_activity = p.follow_latest;
        }
        if p.graph_all_agents != old.graph_all_agents {
            self.graph.set_default_scope(p.graph_all_agents);
        }
        if p.auto_refresh != old.auto_refresh
            || p.activity_refresh_seconds != old.activity_refresh_seconds
        {
            self.next_live_refresh = Instant::now();
        }
        if p.auto_refresh != old.auto_refresh
            || p.repository_refresh_seconds != old.repository_refresh_seconds
        {
            self.next_refresh = Instant::now();
        }
        self.editor
            .apply_preferences(&p, p.code_font_size != old.code_font_size, cx);
        self.applied_preferences = p;
        cx.notify();
    }
    fn persist(&mut self) {
        match self.settings_writer.submit(self.settings.clone()) {
            Ok(_) => {
                self.settings_submission_error = None;
                self.message = "Saving settings…".into();
            }
            Err(error) => self.settings_submission_error = Some(error),
        }
        self.settings_progress = self.settings_writer.status();
        if self.message == "Saving settings…" {
            if self.settings_progress.is_saved() {
                self.message = "Settings saved".into();
            } else if self.settings_progress.is_failed() {
                self.message.clear();
            }
        }
    }

    fn poll_settings_persistence(&mut self) -> bool {
        let progress = self.settings_writer.status();
        if progress == self.settings_progress {
            return false;
        }
        if self.message == "Saving settings…" {
            if progress.is_saved() {
                self.message = "Settings saved".into();
            } else if progress.is_failed() {
                self.message.clear();
            }
        }
        self.settings_progress = progress;
        true
    }

    fn settings_save_error(&self) -> Option<&str> {
        self.settings_submission_error.as_deref().or_else(|| {
            self.settings_progress
                .failure
                .as_ref()
                .map(|failure| failure.message.as_str())
        })
    }

    fn retry_settings_save(&mut self, cx: &mut Context<Self>) {
        if self.settings_submission_error.is_some() {
            self.persist();
        } else if self.settings_writer.retry() {
            self.message = "Saving settings…".into();
            self.settings_progress = self.settings_writer.status();
        }
        cx.notify();
    }

    fn begin_settings_quit(&mut self, cx: &mut Context<Self>) {
        if !self.quitting {
            // Capture a divider drag even if its mouse-up has not arrived yet.
            self.drag = None;
            self.settings_quit_discard = false;
            self.persist();
        }
        cx.notify();
    }

    fn settings_quit_ready(&self) -> bool {
        self.settings_quit_discard
            || (self.settings_submission_error.is_none() && self.settings_progress.is_saved())
    }

    fn native_quit_ready(&self, cx: &App) -> bool {
        NativeQuitReadiness {
            requested: self.quitting,
            settings_ready: self.settings_quit_ready(),
            processes_idle: self.engine.is_idle() && self.tools.terminal.is_idle(),
            editor_busy: self.editor.is_busy(),
            has_dirty_documents: self.editor.has_dirty(cx),
            documents_approved: self.editor.quit_accepted,
        }
        .can_terminate()
    }

    fn render_settings_failure(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(12.))
            .py(px(7.))
            .bg(rgb(p.raised))
            .border_t_1()
            .border_color(rgb(p.border))
            .child(div().flex_1().min_w_0().child(small(
                format!(
                    "Settings are not saved. {}",
                    self.settings_save_error().unwrap_or_default()
                ),
                p.red,
            )))
            .child(
                action(
                    if self.settings_progress.is_failed()
                        || self.settings_submission_error.is_some()
                    {
                        "Retry"
                    } else {
                        "Saving…"
                    },
                    p,
                    cx.listener(|this, _, _, cx| this.retry_settings_save(cx)),
                )
                .flex_shrink_0(),
            )
            .into_any_element()
    }

    fn render_quit_status(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let failed = !self.settings_quit_discard
            && (self.settings_progress.is_failed() || self.settings_submission_error.is_some());
        let title = if failed {
            "Settings could not be saved"
        } else if !self.settings_quit_ready() {
            "Saving settings before closing…"
        } else {
            "Stopping the active agent before closing…"
        };
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000088))
            .child(
                div()
                    .w(px(460.))
                    .p(px(20.))
                    .bg(rgb(p.raised))
                    .border_1()
                    .border_color(rgb(p.border))
                    .rounded(px(6.))
                    .child(line(title, p.text))
                    .when(failed, |panel| {
                        panel
                            .child(
                                line(
                                    self.settings_save_error().unwrap_or_default().to_owned(),
                                    p.muted,
                                )
                                .mt(px(10.)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .justify_end()
                                    .gap(px(8.))
                                    .mt(px(18.))
                                    .child(action(
                                        "Quit without saving",
                                        p,
                                        cx.listener(|this, _, _, cx| {
                                            this.settings_progress = this.settings_writer.status();
                                            if this.settings_progress.is_failed()
                                                || this.settings_submission_error.is_some()
                                            {
                                                this.settings_quit_discard = true;
                                            }
                                            cx.notify();
                                        }),
                                    ))
                                    .child(action(
                                        "Retry save",
                                        p,
                                        cx.listener(|this, _, _, cx| this.retry_settings_save(cx)),
                                    )),
                            )
                    }),
            )
            .into_any_element()
    }
    fn clear_details(&mut self) {
        self.pending_graph_record = None;
        self.selected_record = None;
        self.detail_title.clear();
        self.detail = lines(String::new());
    }
    fn choose_path(&mut self, kind: usize, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: kind == 1,
            directories: kind != 1,
            multiple: false,
            prompt: Some(
                if kind == 0 {
                    "Open workspace"
                } else if kind == 1 {
                    "Choose worker script"
                } else {
                    "Choose state folder"
                }
                .into(),
            ),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = prompt.await {
                if let Some(path) = paths.first() {
                    let path = path.to_string_lossy().into_owned();
                    let _ = this.update(cx, |this, cx| {
                        if kind == 0 {
                            this.set_project(path, cx);
                        } else if kind == 1 {
                            this.settings.worker = path.clone();
                            this.worker_input.update(cx, |v, cx| v.set_value(path, cx));
                            this.persist();
                            this.refresh(cx);
                        } else {
                            this.settings.state_dir = path.clone();
                            this.state_input.update(cx, |v, cx| v.set_value(path, cx));
                            this.persist();
                            this.refresh(cx);
                        }
                    });
                }
            }
        })
        .detach();
    }
    fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = true;
        self.palette_query.clear();
        self.palette_index = 0;
        self.palette_input.update(cx, |v, cx| v.set_value("", cx));
        self.palette_input.focus_handle(cx).focus(window, cx);
        cx.notify();
    }
    fn palette_commands(&self) -> Vec<(usize, String, String)> {
        let mut commands: Vec<_> = PANES
            .iter()
            .enumerate()
            .map(|(i, (name, _))| (i, format!("Open {name}"), format!("⌘{}", i + 1)))
            .collect();
        commands.extend([
            (9, "Open workspace…".into(), "⌘O".into()),
            (10, "Refresh workspace".into(), "⌘R".into()),
            (11, "Toggle sidebar".into(), "⌘B".into()),
            (12, "Toggle Changes dock".into(), "⌥⌘I".into()),
            (13, "Appearance: Dark".into(), "".into()),
            (14, "Appearance: Light".into(), "".into()),
            (15, "Appearance: System".into(), "".into()),
            (16, "Reveal project folder".into(), "".into()),
            (17, "Stop configured worker…".into(), "".into()),
            (18, "Toggle workspace files".into(), "⇧⌘E".into()),
            (19, "Toggle local Console".into(), "⌘`".into()),
            (20, "Ask about this pane".into(), "⇧⌘L".into()),
            (21, "Open local text file…".into(), "⇧⌘O".into()),
        ]);
        commands.extend(
            agentwatcher::themes::presets()
                .iter()
                .enumerate()
                .map(|(index, theme)| {
                    (100 + index, format!("Theme: {}", theme.name), String::new())
                }),
        );
        commands
            .into_iter()
            .filter(|(_, name, _)| {
                self.palette_query
                    .split_whitespace()
                    .all(|token| name.to_lowercase().contains(token))
            })
            .collect()
    }
    fn run_command(&mut self, id: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.palette_open = false;
        window.focus(&self.focus, cx);
        match id {
            0..=8 => self.navigate(id, window, cx),
            9 => self.choose_path(0, cx),
            10 => self.manual_refresh(cx),
            11 => self.sidebar = !self.sidebar,
            12 => self.toggle_dock(cx),
            13..=15 => {
                self.choose_appearance(["dark", "light", "system"][id - 13], cx);
            }
            16 => cx.reveal_path(Path::new(&self.settings.project)),
            17 => {
                self.confirmation = None;
                self.worker_confirmation = true;
            }
            18 => self.toggle_files(cx),
            19 => self.toggle_console(cx),
            20 => self.ask_current_pane(cx),
            21 => self.choose_editor_file(cx),
            100.. => {
                if let Some(theme) = agentwatcher::themes::presets().get(id - 100) {
                    self.choose_appearance(theme.id, cx);
                }
            }
            _ => {}
        }
        cx.notify();
    }
    fn operation(&mut self, action: &str, pid: Option<ProcessInfo>, cx: &mut Context<Self>) {
        if self.settings.preferences.monitor_only
            && pid.is_none()
            && matches!(action, "start" | "resume")
        {
            self.message =
                "Monitor only is enabled. Turn it off in Launching settings to run an agent."
                    .into();
            cx.notify();
            return;
        }
        if self.operation_busy {
            return;
        }
        if pid.is_none() && self.settings.worker.is_empty() {
            self.message = "Choose a worker script in Settings first.".into();
            cx.notify();
            return;
        }
        self.operation_busy = true;
        self.confirmation = None;
        self.worker_confirmation = false;
        self.message = "Applying worker command…".into();
        self.operation_worker
            .request((self.epoch, self.settings.clone(), action.into(), pid));
        cx.notify();
    }
    fn session_by_id(&self, id: &str) -> Option<&Session> {
        if id.is_empty() {
            return None;
        }
        let source = if self.source == "automatic" && self.snapshot.session.id == id {
            self.snapshot.session.source.as_str()
        } else {
            self.source.as_str()
        };
        select_recording(source, id, None, &self.known_sessions)
    }
    fn current_agent(&self) -> Option<&Session> {
        self.session_by_id(&self.session_id)
            .or_else(|| (!self.snapshot.session.id.is_empty()).then_some(&self.snapshot.session))
    }
    fn continue_agent(&mut self, fresh: bool, cx: &mut Context<Self>) {
        if self.settings.preferences.monitor_only {
            self.message = "Monitor only is enabled. Agent launches are disabled.".into();
            cx.notify();
            return;
        }

        if let Some(session) = self.current_agent().cloned() {
            self.terminal_worker.request((self.epoch, session, fresh));
            self.message = "Preparing agent in Terminal…".into();
            cx.notify();
        }
    }
    fn zoom_viewer(&mut self, delta: i32, cx: &mut Context<Self>) {
        if self.pane == 7 {
            self.zoom_graph(delta, cx);
            return;
        }
        let code = match self.pane {
            3 => Some(self.commit_view.clone()),
            4 => Some(self.working_view.clone()),
            1 if self.log_mode => Some(self.log_view.clone()),
            _ => None,
        };
        if let Some(view) = code {
            let default_size = self.settings.preferences.code_font_size;
            view.update(cx, |view, cx| {
                let size = if delta == 0 {
                    default_size
                } else {
                    view.font_size() + delta as f32 * self.settings.preferences.viewer_zoom_step
                };
                view.set_font_size(size, cx);
            });
        } else {
            let size = &mut self.viewer_font_sizes[self.pane];
            *size = if delta == 0 {
                self.settings.preferences.trace_font_size
            } else {
                (*size + delta as f32 * self.settings.preferences.viewer_zoom_step).clamp(8., 32.)
            };
        }
        cx.notify();
    }
    fn activity_signal(&self, p: Palette) -> AnyElement {
        let fresh = self
            .last_event
            .is_some_and(|at| at.elapsed() < Duration::from_secs(3));
        let dot = div().size(px(6.)).flex_shrink_0().rounded_full().bg(rgb(
            if fresh || self.agent_process.is_some() {
                p.green
            } else {
                p.faint
            },
        ));
        if fresh
            && self.settings.preferences.pulse_activity
            && !self.settings.preferences.reduced_motion
        {
            dot.with_animation(
                ("activity-pulse", self.event_revision),
                Animation::new(Duration::from_millis(900))
                    .repeat()
                    .with_easing(bounce(ease_in_out)),
                |dot, delta| dot.opacity(0.4 + 0.6 * delta),
            )
            .into_any_element()
        } else {
            dot.into_any_element()
        }
    }
    fn render_agent_controls(&self, p: Palette, cx: &mut Context<Self>) -> Div {
        let mut controls = div().flex().flex_wrap().items_center().gap(px(5.));
        if let Some(session) = self.current_agent() {
            controls=controls.child(self.activity_signal(p)).child(small(if self.agent_process.is_some(){"Running"}else if self.last_event.is_some_and(|at|at.elapsed()<Duration::from_secs(3)){"New activity"}else{"Recorded"},p.faint)).child(action(if session.source == "deepseek"{"Open DeepSeek"}else{"Continue in Terminal"},p,cx.listener(|this,_,_,cx|{
                if this.current_agent().is_some_and(|s|s.source == "deepseek") { cx.open_url("dsh://open"); this.message="DeepSeek opened. Select this recorded session there to continue it.".into();cx.notify(); }
                else {this.continue_agent(false,cx);}
            })).bg(rgb(p.control)).border_color(rgb(p.border)).text_color(rgb(p.text)))
                .child(action("+ Agent",p,cx.listener(|this,_,window,cx| {
                    let key = this.current_agent().map(|s| (s.source.clone(),s.id.clone()));
                    this.navigate(7,window,cx);
                    this.begin_graph_creation(key,cx);
                })))
                .child(action("Stop…",p,cx.listener(|this,_,_,cx|{
                    if let Some(process)=this.agent_process.clone(){this.confirmation=Some(process);this.worker_confirmation=false;}
                    else {this.message="No dedicated live process is linked to this recording. Shared provider agents must be stopped in their provider app.".into();}
                    cx.notify();
                })).text_color(rgb(p.red)).when(self.agent_process.is_none(),|s|s.opacity(0.4)));
        } else {
            controls = controls.child(action(
                "Choose an agent",
                p,
                cx.listener(|this, _, window, cx| this.navigate(5, window, cx)),
            ));
        }
        controls
    }
    fn render_agent_context(&self, p: Palette, cx: &mut Context<Self>) -> Div {
        let mut row = div()
            .flex()
            .flex_wrap()
            .flex_shrink_0()
            .w_full()
            .items_center()
            .gap(px(10.))
            .min_h(px(38.))
            .py(px(5.))
            .px(px(14.))
            .bg(rgb(p.panel))
            .border_b_1()
            .border_color(rgb(p.border));
        if let Some(session) = self.current_agent() {
            let parent = self
                .known_sessions
                .iter()
                .find(|s| s.id == session.parent_id && s.source == session.source);
            let trail = if session.parent_id.is_empty() {
                "Root agent".into()
            } else {
                format!(
                    "{} → {}",
                    parent
                        .map(agent_label)
                        .unwrap_or_else(|| short_id(&session.parent_id)),
                    agent_label(session)
                )
            };
            row = row
                .child(self.activity_signal(p))
                .child(
                    line(
                        format!(
                            "{} · {} · {}",
                            source_label(&session.source),
                            agent_label(session),
                            short_id(&session.id)
                        ),
                        p.text,
                    )
                    .max_w(px(240.))
                    .line_clamp(1)
                    .flex_shrink_0(),
                )
                .child(small(trail, p.faint).flex_1().min_w_0().line_clamp(1))
                .when(self.pane == 3 || self.pane == 4, |s| {
                    s.child(small("Shared workspace changes", p.muted))
                })
                .child(self.render_agent_controls(p, cx));
        } else {
            row = row
                .child(icon("agent", p.faint))
                .child(line("Select an agent to follow its trace", p.muted).flex_1())
                .child(action(
                    "Agent history",
                    p,
                    cx.listener(|this, _, window, cx| this.navigate(5, window, cx)),
                ));
        }
        row
    }
    fn render_titlebar(&self, p: Palette, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .items_center()
            .h(px(40.))
            .flex_shrink_0()
            .pl(px(84.))
            .pr(px(10.))
            .gap(px(4.))
            .bg(rgb(p.panel))
            .border_b_1()
            .border_color(rgb(p.border))
            .child(
                action(
                    "",
                    p,
                    cx.listener(|this, _, _, cx| {
                        this.sidebar = !this.sidebar;
                        cx.notify();
                    }),
                )
                .child(icon("sidebar", p.muted)),
            )
            .child(
                action(
                    " ",
                    p,
                    cx.listener(|this, _, window, cx| this.open_palette(window, cx)),
                )
                .child(icon("search", p.muted)),
            )
            .child(
                action(
                    "‹",
                    p,
                    cx.listener(|this, _, window, cx| this.history_move(-1, window, cx)),
                )
                .opacity(if self.history_index > 0 { 1. } else { 0.35 }),
            )
            .child(
                action(
                    "›",
                    p,
                    cx.listener(|this, _, window, cx| this.history_move(1, window, cx)),
                )
                .opacity(if self.history_index + 1 < self.history.len() {
                    1.
                } else {
                    0.35
                }),
            )
            .child(
                div()
                    .flex_1()
                    .text_size(px(12.))
                    .text_color(rgb(p.muted))
                    .px(px(10.))
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
                    .child(format!(
                        "{}  /  {}",
                        name(&self.settings.project),
                        PANES[self.pane].0
                    )),
            )
            .child(action(
                "✧ Ask",
                p,
                cx.listener(|this, _, _, cx| this.ask_current_pane(cx)),
            ))
            .when(self.editor.count() > 0, |s| {
                s.child(action(
                    format!("Files · {}", self.editor.count()),
                    p,
                    cx.listener(|this, _, _, cx| {
                        this.editor.open = true;
                        cx.notify();
                    }),
                ))
            })
            .child(
                action(
                    "Refresh",
                    p,
                    cx.listener(|this, _, _, cx| this.manual_refresh(cx)),
                )
                .child(icon("refresh", p.muted)),
            )
            .child(
                action(
                    "  ",
                    p,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_dock(cx);
                    }),
                )
                .child(icon("dock", p.muted)),
            )
    }
    fn render_sidebar(&self, width: f32, p: Palette, cx: &mut Context<Self>) -> Div {
        let compact = width < 100.;
        let mut sidebar = div()
            .flex()
            .flex_col()
            .w(px(width))
            .h_full()
            .flex_shrink_0()
            .bg(rgb(p.panel))
            .overflow_hidden();
        sidebar = sidebar.child(
            div()
                .flex()
                .items_center()
                .h(px(48.))
                .px(px(12.))
                .gap(px(9.))
                .child(icon("folder", p.muted))
                .when(!compact, |s| {
                    s.child(
                        action(
                            name(&self.settings.project),
                            p,
                            cx.listener(|this, _, _, cx| this.choose_path(0, cx)),
                        )
                        .flex_1()
                        .justify_start(),
                    )
                }),
        );
        if !compact {
            sidebar = sidebar.child(small("WORKSPACE", p.faint).px(px(16.)).py(px(9.)));
        }
        sidebar = sidebar.children(
            PANES
                .iter()
                .enumerate()
                .filter(|(i, _)| *i < 2 || *i == 7 || *i == 8 || (self.tools_expanded && *i < 6))
                .map(|(i, (title, symbol))| {
                    div()
                        .id(("nav", i))
                        .h(px(32.))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .px(px(16.))
                        .border_l(px(2.))
                        .border_color(rgb(if self.pane == i { p.accent } else { p.panel }))
                        .when(self.pane == i, |s| s.bg(rgb(p.raised)))
                        .cursor_pointer()
                        .hover(move |s| s.bg(rgb(p.raised)))
                        .child(icon(symbol, if self.pane == i { p.text } else { p.muted }))
                        .when(!compact, |s| {
                            s.child(div().flex_1().child((*title).to_owned()))
                                .child(small(format!("⌘{}", i + 1), p.faint))
                        })
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.navigate(i, window, cx)),
                        )
                }),
        );
        sidebar = sidebar.child(
            div().px(px(12.)).pt(px(12.)).child(
                action(
                    if compact {
                        "···"
                    } else if self.tools_expanded {
                        "Less tools ▾"
                    } else {
                        "Explore tools ▸"
                    },
                    p,
                    cx.listener(|this, _, _, cx| {
                        this.tools_expanded = !this.tools_expanded;
                        cx.notify();
                    }),
                )
                .justify_start(),
            ),
        );
        if self.tools_expanded {
            sidebar = sidebar.child(
                div()
                    .px(px(12.))
                    .flex()
                    .flex_wrap()
                    .gap(px(4.))
                    .child(action(
                        "Files",
                        p,
                        cx.listener(|this, _, _, cx| this.toggle_files(cx)),
                    ))
                    .child(action(
                        "Console",
                        p,
                        cx.listener(|this, _, _, cx| this.toggle_console(cx)),
                    )),
            );
        }
        if !compact {
            let mut agents = div()
                .id("sidebar-agents")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(small("AGENTS", p.faint).px(px(16.)).pt(px(20.)).pb(px(8.)));
            for source in ["codex", "deepseek", "opencode"] {
                let mut roots: Vec<_> = self
                    .known_sessions
                    .iter()
                    .filter(|session| session.source == source && session.parent_id.is_empty())
                    .take(self.settings.preferences.sidebar_agent_limit)
                    .collect();
                if let Some(selected) = self.current_agent() {
                    let mut root = selected;
                    for _ in 0..16 {
                        let Some(parent) = self.known_sessions.iter().find(|s| {
                            s.id == root.parent_id && s.id != root.id && s.source == root.source
                        }) else {
                            break;
                        };
                        root = parent;
                    }
                    if root.source == source && !roots.iter().any(|s| s.id == root.id) {
                        roots.push(root);
                    }
                }
                if roots.is_empty() {
                    continue;
                }
                agents = agents.child(small(source_label(source), p.faint).px(px(16.)).py(px(5.)));
                for session in roots {
                    let id = session.id.clone();
                    let source = session.source.clone();
                    agents = agents.child(
                        div()
                            .id(SharedString::from(format!("agent-nav-{}", session.id)))
                            .px(px(16.))
                            .py(px(7.))
                            .cursor_pointer()
                            .hover(move |s| s.bg(rgb(p.raised)))
                            .when(
                                session.id == self.session_id && session.source == self.source,
                                |s| s.bg(rgb(p.selected)),
                            )
                            .child(
                                line(visuals::display_task(&session.title), p.text)
                                    .w_full()
                                    .line_clamp(1),
                            )
                            .child(
                                small(
                                    [
                                        self.settings
                                            .preferences
                                            .sidebar_show_workspace
                                            .then(|| name(&session.project)),
                                        self.settings
                                            .preferences
                                            .sidebar_show_session_ids
                                            .then(|| short_id(&session.id)),
                                    ]
                                    .into_iter()
                                    .flatten()
                                    .collect::<Vec<_>>()
                                    .join(" · "),
                                    p.faint,
                                )
                                .mt(px(3.)),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_session_key_in(&source, id.clone(), 1, window, cx)
                            })),
                    );
                    if session.id == self.session_id && session.source == self.source
                        || (self.snapshot.session.parent_id == session.id
                            && self.snapshot.session.source == session.source)
                    {
                        for child in self
                            .known_sessions
                            .iter()
                            .filter(|s| s.parent_id == session.id && s.source == session.source)
                            .take(self.settings.preferences.sidebar_child_limit)
                        {
                            let id = child.id.clone();
                            let source = child.source.clone();
                            agents = agents.child(
                                div()
                                    .id(SharedString::from(format!("agent-nav-child-{}", child.id)))
                                    .ml(px(24.))
                                    .pl(px(10.))
                                    .pr(px(12.))
                                    .py(px(6.))
                                    .border_l_1()
                                    .border_color(rgb(p.border))
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(rgb(p.raised)))
                                    .when(
                                        child.id == self.session_id && child.source == self.source,
                                        |s| s.bg(rgb(p.selected)),
                                    )
                                    .child(line(agent_label(child), p.muted).line_clamp(1))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.select_session_key_in(
                                            &source,
                                            id.clone(),
                                            1,
                                            window,
                                            cx,
                                        )
                                    })),
                            );
                        }
                    }
                }
            }
            agents = agents.child(
                action(
                    "All agent history →",
                    p,
                    cx.listener(|this, _, window, cx| this.navigate(5, window, cx)),
                )
                .mx(px(8.))
                .mt(px(7.))
                .justify_start(),
            );
            sidebar = sidebar.child(agents);
        }
        sidebar = sidebar.when(compact, |s| s.child(div().flex_1())).child(
            div().px(px(12.)).py(px(8.)).child(
                action(
                    if compact { "⚙" } else { "Settings" },
                    p,
                    cx.listener(|this, _, window, cx| this.navigate(6, window, cx)),
                )
                .justify_start(),
            ),
        );
        sidebar.child(
            div()
                .flex()
                .justify_between()
                .items_center()
                .h(px(36.))
                .px(px(12.))
                .border_t_1()
                .border_color(rgb(p.border))
                .child(small(if compact { "⌘" } else { "AgentWatcher" }, p.faint))
                .when(!compact, |s| {
                    s.child(action(
                        "⌘K",
                        p,
                        cx.listener(|this, _, window, cx| this.open_palette(window, cx)),
                    ))
                }),
        )
    }
    fn render_divider(&self, kind: usize, p: Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id(("divider", kind))
            .w(px(5.))
            .h_full()
            .flex_shrink_0()
            .flex()
            .justify_center()
            .bg(rgb(p.base))
            .child(div().w(px(1.)).h_full().bg(rgb(p.border)))
            .cursor(CursorStyle::ResizeLeftRight)
            .hover(move |s| s.bg(rgb(p.accent)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    if event.click_count == 2 {
                        match kind {
                            0 => this.settings.sidebar_width = 228.,
                            1 => this.settings.dock_width = 400.,
                            _ => this.increment_width = 260.,
                        };
                        this.persist();
                    } else {
                        this.drag = Some((kind, f32::from(event.position.x)));
                    }
                    cx.notify();
                }),
            )
    }
    fn render_sources(&self, p: Palette, cx: &mut Context<Self>) -> Div {
        div().flex().items_center().gap(px(2.)).children(
            [
                ("automatic", "Latest"),
                ("codex", "Codex"),
                ("deepseek", "DeepSeek"),
                ("opencode", "OpenCode"),
            ]
            .into_iter()
            .map(|(id, label)| {
                action(
                    label,
                    p,
                    cx.listener(move |this, _, _, cx| this.change_source(id, cx)),
                )
                .when(self.source == id, |s| {
                    s.bg(rgb(p.selected)).text_color(rgb(p.text))
                })
            }),
        )
    }
    fn render_worker_controls(&self, p: Palette, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .items_center()
            .gap(px(5.))
            .child(small(
                if self.worker.label.is_empty() {
                    "Detecting worker"
                } else {
                    &self.worker.label
                },
                p.muted,
            ))
            .child(
                action(
                    if self.worker.running && !self.worker.pausing {
                        "Pause"
                    } else if self.worker.pausing {
                        "Resume"
                    } else {
                        "Start"
                    },
                    p,
                    cx.listener(|this, _, _, cx| {
                        let action = if this.worker.running && !this.worker.pausing {
                            "pause"
                        } else if this.worker.pausing {
                            "resume"
                        } else {
                            "start"
                        };
                        this.operation(action, None, cx);
                    }),
                )
                .bg(rgb(p.raised))
                .border_color(rgb(p.border))
                .text_color(rgb(p.text))
                .font_weight(FontWeight::MEDIUM)
                .when(
                    self.settings.worker.is_empty() || self.operation_busy,
                    |s| s.opacity(0.35),
                ),
            )
    }
    fn render_agent_list(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let child_view = cx.entity().downgrade();
        let actions = visuals::AgentActions {
            open: std::rc::Rc::new(cx.listener(|this, _, window, cx| this.navigate(1, window, cx))),
            child: std::rc::Rc::new(move |child, window, cx| {
                let _ = child_view.update(cx, |this, cx| this.open_child(child, window, cx));
            }),
        };
        let mut body = div()
            .id("overview-scroll")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(heading(
                "Agents",
                "Your agents and their work, at a glance.",
                p,
            ))
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_between()
                    .min_h(px(44.))
                    .flex_wrap()
                    .py(px(6.))
                    .px(px(18.))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .child(self.render_agent_controls(p, cx))
                    .child(
                        div()
                            .flex()
                            .gap(px(5.))
                            .child(action(
                                "Switch session",
                                p,
                                cx.listener(|this, _, window, cx| this.navigate(5, window, cx)),
                            ))
                            .child(action(
                                if self.agent_details {
                                    "Hide details ▴"
                                } else {
                                    "Agent details ▾"
                                },
                                p,
                                cx.listener(|this, _, _, cx| {
                                    this.agent_details = !this.agent_details;
                                    cx.notify();
                                }),
                            )),
                    ),
            );
        if self.loading && self.snapshot.session.id.is_empty() {
            body = body.child(
                div()
                    .h(px(150.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .px(px(24.))
                    .gap(px(10.))
                    .child(icon("activity", p.faint))
                    .child(line("Reading recorded activity…", p.muted)),
            );
        } else if !self.snapshot.session.id.is_empty() && self.agent_details {
            body = body.child(
                visuals::agent_overview(
                    &self.snapshot,
                    &self.worker,
                    p,
                    self.agent_details,
                    actions,
                )
                .flex_shrink_0(),
            );
        }
        body = body.child(self.render_agent_catalog(p, cx));
        body = body.child(
            div()
                .flex_shrink_0()
                .px(px(18.))
                .py(px(12.))
                .border_t_1()
                .border_color(rgb(p.border))
                .child(
                    action(
                        if self.repository_details {
                            "Repository activity ▴"
                        } else {
                            "Repository activity ▸"
                        },
                        p,
                        cx.listener(|this, _, _, cx| {
                            this.repository_details = !this.repository_details;
                            cx.notify();
                        }),
                    )
                    .justify_start(),
                ),
        );
        if self.repository_details {
            body = body
                .child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .flex_wrap()
                        .gap(px(20.))
                        .px(px(18.))
                        .py(px(10.))
                        .child(small(
                            format!("{} recent increments", self.repository.commits.len()),
                            p.muted,
                        ))
                        .child(small(
                            format!("{} changed files", self.repository.changes.len()),
                            p.muted,
                        ))
                        .when(
                            self.settings.preferences.show_usage && self.snapshot.usage.is_some(),
                            |s| {
                                s.child(small(
                                    format!(
                                        "{} recorded tokens",
                                        self.snapshot.usage.as_ref().unwrap().total
                                    ),
                                    p.faint,
                                ))
                            },
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .justify_between()
                        .px(px(18.))
                        .pt(px(12.))
                        .pb(px(9.))
                        .child(small("RECENT INCREMENTS", p.faint))
                        .child(action(
                            "View all",
                            p,
                            cx.listener(|this, _, window, cx| this.navigate(3, window, cx)),
                        )),
                )
                .children(
                    self.repository
                        .commits
                        .iter()
                        .take(5)
                        .enumerate()
                        .map(|(i, c)| {
                            let hash = c.hash.clone();
                            div()
                                .id(("overview-commit", i))
                                .h(px(34.))
                                .flex_shrink_0()
                                .w_full()
                                .flex()
                                .items_center()
                                .gap(px(12.))
                                .px(px(18.))
                                .cursor_pointer()
                                .hover(move |s| s.bg(rgb(p.raised)))
                                .child(icon("changes", p.faint))
                                .child(
                                    small(c.short_hash.clone(), p.accent)
                                        .font_family(style::CODE_FONT),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .line_clamp(1)
                                        .child(c.subject.clone()),
                                )
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.selected_commit = hash.clone();
                                    this.navigate(3, window, cx);
                                }))
                        }),
                );
            if let Some(error) = &self.repository.error {
                body = body.child(line(error.clone(), p.red).p(px(18.)));
            }
        }
        body.into_any_element()
    }
    fn open_child(&mut self, child: &ChildAgent, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .known_sessions
            .iter()
            .any(|s| s.id == child.session_id && s.source == self.snapshot.session.source)
        {
            self.select_session_key_in(
                &self.snapshot.session.source.clone(),
                child.session_id.clone(),
                1,
                window,
                cx,
            );
        } else {
            self.clear_details();
            self.detail_title = child.title.clone();
            self.detail = lines(format!(
                "Agent: {}\nStatus: {}\nSession: {}",
                child.agent, child.status, child.session_id
            ));
            self.dock = true;
            self.dock_tab = 1;
            cx.notify();
        }
    }
    fn render_activity(&self, p: Palette, width: f32, cx: &mut Context<Self>) -> AnyElement {
        let mut content = div().flex().flex_col().flex_1().min_h_0().child(
            div()
                .flex()
                .flex_wrap()
                .gap_y(px(10.))
                .gap_x(px(14.))
                .min_h(px(38.))
                .py(px(5.))
                .flex_shrink_0()
                .items_center()
                .justify_between()
                .when(width < 680., |s| s.flex_col().items_start())
                .px(px(14.))
                .border_b_1()
                .border_color(rgb(p.border))
                .child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .gap(px(2.))
                        .child(
                            action(
                                "Agent work",
                                p,
                                cx.listener(|this, _, _, cx| {
                                    this.log_mode = false;
                                    cx.notify();
                                }),
                            )
                            .when(!self.log_mode, |s| s.bg(rgb(p.raised))),
                        )
                        .child(
                            action(
                                "Worker log",
                                p,
                                cx.listener(|this, _, _, cx| {
                                    this.log_mode = true;
                                    cx.notify();
                                }),
                            )
                            .when(self.log_mode, |s| s.bg(rgb(p.raised))),
                        ),
                )
                .child(self.render_sources(p, cx)),
        );
        if self.log_mode {
            content = content
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .h(px(42.))
                        .px(px(18.))
                        .child(line("Worker output", p.text))
                        .child(
                            div()
                                .flex()
                                .gap(px(8.))
                                .child(action(
                                    "Copy",
                                    p,
                                    cx.listener(|this, _, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            this.worker.log.clone(),
                                        ))
                                    }),
                                ))
                                .child(self.render_worker_controls(p, cx)),
                        ),
                )
                .child(self.code_view("worker-log", self.worker_log.clone(), p, cx, false));
            return content.into_any_element();
        }
        if let Some(kind) = self.trace_outcome {
            content = content.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .px(px(14.))
                    .py(px(6.))
                    .gap(px(6.))
                    .bg(rgb(p.raised))
                    .child(small(format!("Outcome · {}", outcome_label(kind)), p.text))
                    .child(action(
                        "Show all outcomes",
                        p,
                        cx.listener(|this, _, _, cx| {
                            this.trace_outcome = None;
                            this.filter_records();
                            this.record_scroll = UniformListScrollHandle::new();
                            cx.notify();
                        }),
                    )),
            );
        }
        if let Some((start, end)) = self.trace_time_range {
            content = content.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .px(px(14.))
                    .py(px(6.))
                    .gap(px(6.))
                    .bg(rgb(p.panel))
                    .child(small(
                        format!(
                            "Time range · {} — {} · selected agent",
                            dashboard::outcome_time(start),
                            dashboard::outcome_time(end)
                        ),
                        p.muted,
                    ))
                    .child(action(
                        "Clear time filter",
                        p,
                        cx.listener(|this, _, _, cx| {
                            this.trace_time_range = None;
                            this.filter_records();
                            cx.notify();
                        }),
                    )),
            );
            if !self.loading && self.records.is_empty() && !self.snapshot.session.id.is_empty() {
                content=content.child(small("No matching events in this loaded window. Use Earlier / Newer, or clear the time filter.",p.faint).px(px(14.)).py(px(8.)).w_full().line_clamp(2));
            }
        }
        if self.loading && self.snapshot.records.is_empty() {
            return content
                .child(empty(
                    "Reading recorded activity…",
                    "The workspace stays interactive while local history loads.",
                    p,
                ))
                .into_any_element();
        }
        if self.snapshot.session.id.is_empty() {
            return content
                .child(empty(
                    "No recorded agent work",
                    self.snapshot
                        .error
                        .as_deref()
                        .unwrap_or("Choose a source or open a project with recorded sessions."),
                    p,
                ))
                .into_any_element();
        }
        content = content.child(
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .p(px(18.))
                .border_b_1()
                .border_color(rgb(p.border))
                .child(
                    line(visuals::display_task(&self.snapshot.session.title), p.text)
                        .text_size(px(15.))
                        .line_clamp(3),
                )
                .child(small(
                    format!(
                        "{}   {}   {} records",
                        self.snapshot.session.source,
                        self.snapshot.model,
                        self.snapshot.records.len()
                    ),
                    p.faint,
                )),
        );
        if !self.snapshot.children.is_empty() {
            content = content.child(
                div()
                    .id("agent-tree")
                    .flex()
                    .items_center()
                    .overflow_x_scroll()
                    .gap(px(8.))
                    .h(px(80.))
                    .px(px(18.))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .child(
                        div()
                            .w(px(160.))
                            .flex_shrink_0()
                            .child(small("CURRENT AGENT", p.accent))
                            .child(line(name(&self.settings.project), p.text).mt(px(6.))),
                    )
                    .child(small("→", p.faint))
                    .children(self.snapshot.children.iter().take(128).enumerate().map(
                        |(i, child)| {
                            let detail = child.clone();
                            div()
                                .id(("child", i))
                                .w(px(180.))
                                .flex_shrink_0()
                                .px(px(12.))
                                .py(px(8.))
                                .border_l_1()
                                .border_color(rgb(p.border))
                                .cursor_pointer()
                                .hover(move |s| s.bg(rgb(p.raised)))
                                .child(small(
                                    if child.agent.is_empty() {
                                        "SUBAGENT".into()
                                    } else {
                                        child.agent.clone()
                                    },
                                    p.accent,
                                ))
                                .child(line(child.title.clone(), p.text).line_clamp(1).mt(px(5.)))
                                .child(small(child.status.clone(), p.faint).mt(px(3.)))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_child(&detail, window, cx);
                                }))
                        },
                    )),
            );
        }
        content = content
            .child(
                div()
                    .flex()
                    .h(px(39.))
                    .items_center()
                    .justify_between()
                    .px(px(14.))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .child(
                        div().flex().gap(px(2.)).children(
                            ["Trajectory", "Tools", "All records"]
                                .into_iter()
                                .enumerate()
                                .map(|(i, label)| {
                                    action(
                                        label,
                                        p,
                                        cx.listener(move |this, _, _, cx| {
                                            this.record_mode = i;
                                            this.trace_outcome = None;
                                            this.filter_records();
                                            this.record_scroll = UniformListScrollHandle::new();
                                            cx.notify();
                                        }),
                                    )
                                    .when(self.record_mode == i, |s| s.bg(rgb(p.raised)))
                                }),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                action(
                                    if self.follow_activity {
                                        "● Following latest"
                                    } else {
                                        "Follow latest"
                                    },
                                    p,
                                    cx.listener(|this, _, _, cx| {
                                        this.follow_activity = !this.follow_activity;
                                        if this.follow_activity && !this.records.is_empty() {
                                            this.record_scroll.scroll_to_item(
                                                this.records.len() - 1,
                                                ScrollStrategy::Bottom,
                                            );
                                        }
                                        cx.notify();
                                    }),
                                )
                                .when(self.follow_activity, |s| s.text_color(rgb(p.accent))),
                            )
                            .child(small(
                                format!("{} / {}", self.records.len(), self.snapshot.records.len()),
                                p.faint,
                            )),
                    ),
            )
            .child(div().px(px(14.)).py(px(8.)).child(self.search.clone()));
        if self.snapshot.has_earlier || self.trace_index > 0 {
            content = content.child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_between()
                    .px(px(14.))
                    .py(px(4.))
                    .child(small(
                        if self.trace_index == 0 {
                            "Recent trace"
                        } else {
                            "Earlier trace"
                        },
                        p.faint,
                    ))
                    .child(
                        div()
                            .flex()
                            .gap(px(4.))
                            .child(
                                action(
                                    "← Earlier",
                                    p,
                                    cx.listener(|this, _, _, cx| this.trace_page(true, cx)),
                                )
                                .when(!self.snapshot.has_earlier || self.loading, |s| {
                                    s.opacity(0.35)
                                }),
                            )
                            .child(
                                action(
                                    "Newer →",
                                    p,
                                    cx.listener(|this, _, _, cx| this.trace_page(false, cx)),
                                )
                                .when(self.trace_index == 0 || self.loading, |s| s.opacity(0.35)),
                            ),
                    ),
            );
        }
        if let Some(error) = &self.snapshot.error {
            content = content.child(line(error.clone(), p.red).px(px(18.)).py(px(9.)));
        }
        if let Some(warning) = self.snapshot.warnings.first() {
            content = content.child(
                small(warning.clone(), p.faint)
                    .px(px(18.))
                    .pb(px(8.))
                    .line_clamp(2),
            );
        }
        let rows = uniform_list(
            "trajectory",
            self.records.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .map(|row| {
                        let index = this.records[row];
                        let record = &this.snapshot.records[index];
                        div()
                            .id(("record", index))
                            .w_full()
                            .h(px(
                                if this.record_mode == 1
                                    || this.settings.preferences.trace_compact_rows
                                {
                                    (if this.settings.preferences.trace_compact_rows {
                                        28.
                                    } else {
                                        41.
                                    }) + this.viewer_font_sizes[1]
                                        * this.settings.preferences.trace_line_height
                                } else {
                                    43. + (this.settings.preferences.trace_preview_lines + 1) as f32
                                        * this.viewer_font_sizes[1]
                                        * this.settings.preferences.trace_line_height
                                },
                            ))
                            .flex()
                            .items_start()
                            .gap(px(12.))
                            .px(px(18.))
                            .py(px(if this.settings.preferences.trace_compact_rows {
                                6.
                            } else {
                                10.
                            }))
                            .border_b(px(if this.settings.preferences.trace_show_separators {
                                1.
                            } else {
                                0.
                            }))
                            .border_color(rgb(p.border))
                            .cursor_pointer()
                            .when(this.selected_record == Some(index), |s| s.bg(rgb(p.raised)))
                            .hover(move |s| s.bg(rgb(p.raised)))
                            .child(
                                div()
                                    .w(px(28.))
                                    .h_full()
                                    .flex_shrink_0()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .gap(px(7.))
                                    .when(!this.settings.preferences.trace_show_kind_icons, |s| {
                                        s.hidden()
                                    })
                                    .child(icon(
                                        if record.kind.to_lowercase().contains("tool") {
                                            "terminal"
                                        } else {
                                            "agent"
                                        },
                                        record_color(record, p),
                                    ))
                                    .child(div().w(px(1.)).flex_1().bg(rgb(p.border))),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .w_full()
                                    .min_w_0()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .flex()
                                            .w_full()
                                            .justify_between()
                                            .child(
                                                small(
                                                    record_label(record),
                                                    record_color(record, p),
                                                )
                                                .flex_shrink_0(),
                                            )
                                            .child(
                                                small(
                                                    if this.settings.preferences.show_timestamps {
                                                        display_time(&record.time)
                                                    } else {
                                                        String::new()
                                                    },
                                                    p.faint,
                                                )
                                                .flex_shrink_0(),
                                            ),
                                    )
                                    .child(
                                        line(
                                            trace_preview(record, &this.settings.preferences),
                                            p.text,
                                        )
                                        .text_size(px(this.viewer_font_sizes[1]))
                                        .line_height(px(this.viewer_font_sizes[1]
                                            * 1.45
                                            * this.settings.preferences.trace_line_height))
                                        .line_clamp(
                                            if this.record_mode == 1
                                                || this.settings.preferences.trace_compact_rows
                                            {
                                                1
                                            } else {
                                                this.settings.preferences.trace_preview_lines
                                            },
                                        ),
                                    ),
                            )
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.inspect_record(index, cx)),
                            )
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.record_scroll)
        .flex_1()
        .min_h_0();
        content.child(rows).into_any_element()
    }
    fn render_open_file_button(
        &self,
        id: &'static str,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.selected_file.is_empty() {
            return div().into_any_element();
        }
        let project = self.settings.project.clone();
        let path = self.selected_file.clone();
        let edit_project = project.clone();
        let edit_path = path.clone();
        div()
            .id(id)
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(3.))
            .flex_shrink_0()
            .child(
                action(
                    "Edit in AgentWatcher",
                    p,
                    cx.listener(move |this, _, _, cx| {
                        this.edit_repository_file(edit_project.clone(), edit_path.clone(), cx);
                    }),
                )
                .text_color(rgb(p.accent)),
            )
            .child(action(
                "Other apps ▾",
                p,
                cx.listener(move |this, event: &ClickEvent, window, cx| {
                    if this
                        .file_menu
                        .as_ref()
                        .is_some_and(|(root, file, _)| root == &project && file == &path)
                    {
                        this.file_menu = None;
                    } else {
                        this.file_menu = Some((
                            project.clone(),
                            path.clone(),
                            event.position() + point(px(12.), px(16.)),
                        ));
                    }
                    window.focus(&this.focus, cx);
                    cx.notify();
                }),
            ))
            .into_any_element()
    }
    fn render_file_menu(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let Some((project, path, anchor)) = self.file_menu.clone() else {
            return div().into_any_element();
        };
        let mut menu = div()
            .id("open-with-menu")
            .w(px(248.))
            .p(px(6.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .rounded(px(6.))
            .bg(rgb(p.raised))
            .border_1()
            .border_color(rgb(p.border))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                small(path.clone(), p.faint)
                    .px(px(8.))
                    .py(px(6.))
                    .w_full()
                    .line_clamp(2),
            );
        let edit_project = project.clone();
        let edit_path = path.clone();
        menu = menu.child(
            action(
                "Edit in AgentWatcher",
                p,
                cx.listener(move |this, _, _, cx| {
                    this.edit_repository_file(edit_project.clone(), edit_path.clone(), cx);
                }),
            )
            .w_full()
            .h(px(32.))
            .justify_start()
            .text_color(rgb(p.accent)),
        );
        for target in &self.file_targets {
            let target = *target;
            let file = path.clone();
            let root = project.clone();
            menu = menu.child(
                action(
                    target.label(),
                    p,
                    cx.listener(move |this, _, _, cx| {
                        this.file_menu = None;
                        this.file_worker.request(Some((
                            this.epoch,
                            root.clone(),
                            file.clone(),
                            target,
                        )));
                        this.message = format!("Opening {}…", file);
                        cx.notify();
                    }),
                )
                .w_full()
                .h(px(32.))
                .justify_start()
                .gap(px(8.))
                .text_color(rgb(p.text)),
            );
        }
        if self.file_targets.is_empty() {
            menu = menu.child(small("Finding installed apps…", p.muted).p(px(8.)));
        }
        menu = menu.child(
            small("Terminal apps open the containing folder.", p.faint)
                .px(px(8.))
                .py(px(6.))
                .w_full()
                .line_clamp(2),
        );
        div()
            .id("file-menu-overlay")
            .absolute()
            .inset_0()
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.file_menu = None;
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(
                anchored()
                    .anchor(gpui::Anchor::TopRight)
                    .position(anchor)
                    .snap_to_window()
                    .child(menu),
            )
            .into_any_element()
    }
    fn render_event_evidence(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        if self
            .selected_record
            .and_then(|i| self.snapshot.records.get(i))
            .is_none()
        {
            self.visual_detail.update(cx, |view, cx| view.clear(cx));
            return self.code_view("record-details", self.detail.clone(), p, cx, true);
        }
        self.visual_detail
            .update(cx, |view, cx| view.set_palette(p, cx));
        let mut tabs = div()
            .flex()
            .items_center()
            .w_full()
            .min_w_0()
            .h(px(34.))
            .flex_shrink_0()
            .px(px(10.))
            .gap(px(3.))
            .border_b_1()
            .bg(rgb(p.panel))
            .border_color(rgb(p.border));
        for (mode, label) in [
            (EvidenceMode::Visual, "Visual"),
            (EvidenceMode::Raw, "Raw"),
            (EvidenceMode::Tree, "Tree"),
        ] {
            tabs = tabs.child(
                action(
                    label,
                    p,
                    cx.listener(move |this, _, _, cx| {
                        this.evidence_mode = mode;
                        this.visual_detail.update(cx, |view, cx| {
                            view.set_graphical(mode == EvidenceMode::Tree, cx);
                        });
                        cx.notify();
                    }),
                )
                .when(self.evidence_mode == mode, |s| {
                    s.bg(rgb(p.selected)).text_color(rgb(p.text))
                }),
            );
        }
        let view = if self.evidence_mode != EvidenceMode::Raw {
            self.visual_detail.clone().into_any_element()
        } else {
            self.code_view("record-details", self.detail.clone(), p, cx, true)
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .w_full()
            .min_w_0()
            .child(tabs)
            .child(view)
            .into_any_element()
    }
    fn code_view(
        &self,
        id: &'static str,
        text: Arc<CodeContent>,
        p: Palette,
        cx: &mut Context<Self>,
        detail: bool,
    ) -> AnyElement {
        if text.is_empty() {
            return empty(
                "No content selected",
                "Select an item to inspect its recorded contents.",
                p,
            )
            .into_any_element();
        }
        let (view, key) = if detail {
            (
                &self.detail_view,
                format!(
                    "{}:{}",
                    self.session_id,
                    self.selected_record
                        .and_then(|i| self.snapshot.records.get(i))
                        .map(|r| r.id.as_str())
                        .unwrap_or(&self.detail_title)
                ),
            )
        } else if id == "commit-diff" {
            (&self.commit_view, self.commit_title.clone())
        } else if id == "dock-diff" {
            (&self.dock_view, self.diff_title.clone())
        } else if id == "worker-log" {
            (&self.log_view, self.settings.state_dir.clone())
        } else {
            (&self.working_view, self.diff_title.clone())
        };
        view.update(cx, |view, cx| {
            view.set_content_for(&key, text, cx);
            view.set_palette(p, cx);
            view.set_font(style::CODE_FONT, cx);
        });
        view.clone().into_any_element()
    }
    fn file_list(&self, id: &'static str, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let filtered: Vec<_> = self
            .repository
            .changes
            .iter()
            .enumerate()
            .filter(|(_, file)| {
                self.query.is_empty() || file.path.to_lowercase().contains(&self.query)
            })
            .map(|(i, _)| i)
            .collect();
        uniform_list(
            id,
            filtered.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .map(|row| {
                        let index = filtered[row];
                        let file = &this.repository.changes[index];
                        let path = file.path.clone();
                        div()
                            .id(("change", index))
                            .w_full()
                            .h(px(30.))
                            .w_full()
                            .flex()
                            .items_center()
                            .gap(px(9.))
                            .px(px(12.))
                            .cursor_pointer()
                            .hover(move |s| s.bg(rgb(p.raised)))
                            .when(this.selected_file == file.path, |s| s.bg(rgb(p.selected)))
                            .child(
                                small(
                                    file.status.clone(),
                                    if file.status.contains('?') {
                                        p.accent
                                    } else {
                                        p.muted
                                    },
                                )
                                .w(px(18.)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .line_clamp(1)
                                    .child(file.path.clone()),
                            )
                            .child(small(format!("+{}", file.added), p.green))
                            .child(small(format!("−{}", file.removed), p.red))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.file_menu = None;
                                this.selected_file = path.clone();
                                this.dock_tab = 0;
                                this.request_diff(false, cx);
                            }))
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .w_full()
        .h_full()
        .w_full()
        .into_any_element()
    }
    fn render_increments(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        if self.repository_loading {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(heading("Increments", "Repository history", p))
                .child(empty(
                    "Reading repository history…",
                    "You can keep navigating while the workspace loads.",
                    p,
                ))
                .into_any_element();
        }
        if let Some(error) = &self.repository.error {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(heading("Increments", "Repository history unavailable", p))
                .child(empty("Unable to read repository history", error, p))
                .into_any_element();
        }
        if self.repository.commits.is_empty() {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(heading("Increments", "Commit history", p))
                .child(empty(
                    "No commits yet",
                    "Commits in this workspace will appear here.",
                    p,
                ))
                .into_any_element();
        }
        let list = uniform_list(
            "commits",
            self.repository.commits.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .map(|i| {
                        let c = &this.repository.commits[i];
                        let hash = c.hash.clone();
                        div()
                            .id(("commit", i))
                            .h(px(58.))
                            .w_full()
                            .flex()
                            .flex_col()
                            .justify_center()
                            .gap(px(7.))
                            .px(px(14.))
                            .cursor_pointer()
                            .hover(move |s| s.bg(rgb(p.raised)))
                            .when(c.hash == this.selected_commit, |s| s.bg(rgb(p.selected)))
                            .child(line(c.subject.clone(), p.text).line_clamp(1))
                            .child(small(format!("{}  ·  {}", c.short_hash, c.author), p.faint))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.selected_commit = hash.clone();
                                this.request_diff(true, cx);
                            }))
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .flex_1()
        .min_h_0();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(heading(
                "Increments",
                format!(
                    "{} recent commits · select to inspect changes",
                    self.repository.commits.len()
                ),
                p,
            ))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .w(px(self.increment_width))
                            .flex_shrink_0()
                            .min_h_0()
                            .child(list),
                    )
                    .child(self.render_divider(2, p, cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .h(px(36.))
                                    .items_center()
                                    .justify_between()
                                    .px(px(14.))
                                    .border_b_1()
                                    .border_color(rgb(p.border))
                                    .child(small(
                                        self.selected_commit.chars().take(12).collect::<String>(),
                                        p.muted,
                                    ))
                                    .child(action(
                                        "Copy diff",
                                        p,
                                        cx.listener(|this, _, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                this.commit_diff.source().to_owned(),
                                            ))
                                        }),
                                    )),
                            )
                            .child(self.code_view(
                                "commit-diff",
                                self.commit_diff.clone(),
                                p,
                                cx,
                                false,
                            )),
                    ),
            )
            .into_any_element()
    }
    fn render_changes(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        if self.repository_loading {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(heading("Changes", "Repository changes", p))
                .child(empty(
                    "Reading repository changes…",
                    "You can keep navigating while the workspace loads.",
                    p,
                ))
                .into_any_element();
        }
        if let Some(error) = &self.repository.error {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .child(heading("Changes", "Repository changes unavailable", p))
                .child(empty("Unable to read repository changes", error, p))
                .into_any_element();
        }
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(heading(
                "Changes",
                format!(
                    "{}  ·  {} changed files",
                    self.repository.branch,
                    self.repository.changes.len()
                ),
                p,
            ))
            .child(div().px(px(14.)).py(px(8.)).child(self.search.clone()))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .w(px(250.))
                            .flex_shrink_0()
                            .h_full()
                            .border_r_1()
                            .border_color(rgb(p.border))
                            .child(self.file_list("changes-main", p, cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .w_full()
                                    .min_w_0()
                                    .h(px(36.))
                                    .px(px(14.))
                                    .child(
                                        line(self.selected_file.clone(), p.muted)
                                            .flex_1()
                                            .min_w_0()
                                            .line_clamp(1),
                                    )
                                    .child(self.render_open_file_button("open-with-main", p, cx)),
                            )
                            .child(self.code_view("changes-diff", self.diff.clone(), p, cx, false)),
                    ),
            )
            .into_any_element()
    }
    fn render_processes(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let rows = uniform_list(
            "processes",
            self.process_list.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .map(|i| {
                        let process = &this.process_list[i];
                        let pid = process.pid;
                        let selected_process = process.clone();
                        div()
                            .id(("process", i))
                            .h(px(38.))
                            .w_full()
                            .flex()
                            .items_center()
                            .px(px(18.))
                            .gap(px(12.))
                            .cursor_pointer()
                            .border_b_1()
                            .border_color(rgb(p.border))
                            .when(this.selected_pid == Some(pid), |s| s.bg(rgb(p.selected)))
                            .hover(move |s| s.bg(rgb(p.raised)))
                            .child(small(pid.to_string(), p.muted).w(px(48.)))
                            .child(small(format!("{}%", process.cpu), p.muted).w(px(48.)))
                            .child(small(format!("{}%", process.memory), p.muted).w(px(65.)))
                            .child(small(process.elapsed.clone(), p.faint).w(px(75.)))
                            .child(
                                line(process.command.clone(), p.text)
                                    .flex_1()
                                    .min_w_0()
                                    .line_clamp(1),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.selected_pid = Some(pid);
                                this.selected_process = Some(selected_process.clone());
                                cx.notify();
                            }))
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .flex_1()
        .min_h_0();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(heading(
                "Processes",
                "Agent processes scoped to the selected project and worker",
                p,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(42.))
                    .px(px(18.))
                    .child(line(
                        format!("{} processes", self.process_list.len()),
                        p.muted,
                    ))
                    .child(
                        action(
                            "Stop selected…",
                            p,
                            cx.listener(|this, _, _, cx| {
                                this.confirmation = this.selected_process.clone();
                                cx.notify();
                            }),
                        )
                        .when(self.selected_pid.is_none(), |s| s.opacity(0.3)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(12.))
                    .px(px(18.))
                    .py(px(9.))
                    .bg(rgb(p.panel))
                    .child(small("PID", p.faint).w(px(48.)))
                    .child(small("CPU", p.faint).w(px(48.)))
                    .child(small("MEMORY", p.faint).w(px(65.)))
                    .child(small("ELAPSED", p.faint).w(px(75.)))
                    .child(small("COMMAND", p.faint)),
            )
            .when(self.process_list.is_empty(), |s| {
                s.child(empty(
                    "No matching agent processes",
                    if self.process_error.is_empty() {
                        "Processes appear when a configured worker or project agent is running."
                    } else {
                        &self.process_error
                    },
                    p,
                ))
            })
            .child(rows)
            .into_any_element()
    }
    fn filtered_history(&self) -> Vec<usize> {
        self.known_sessions
            .iter()
            .enumerate()
            .filter(|(_, session)| {
                let source = ["", "codex", "deepseek", "opencode"][self.history_source];
                (source.is_empty() || session.source == source)
                    && (!self.history_project_only || session.project == self.settings.project)
                    && (self.query.is_empty()
                        || format!(
                            "{} {} {} {}",
                            session.title, session.id, session.project, session.agent
                        )
                        .to_lowercase()
                        .contains(&self.query))
            })
            .map(|(index, _)| index)
            .collect()
    }
    fn render_sessions(&self, p: Palette, cx: &mut Context<Self>) -> AnyElement {
        let filtered = self.filtered_history();
        let count = filtered.len();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(heading(
                "Agent history",
                "Every agent keeps its own conversation and trace.",
                p,
            ))
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_between()
                    .h(px(42.))
                    .px(px(14.))
                    .child(
                        div().flex().gap(px(2.)).children(
                            ["All", "Codex", "DeepSeek", "OpenCode"]
                                .iter()
                                .enumerate()
                                .map(|(index, label)| {
                                    action(
                                        *label,
                                        p,
                                        cx.listener(move |this, _, _, cx| {
                                            this.history_source = index;
                                            cx.notify();
                                        }),
                                    )
                                    .when(self.history_source == index, |s| s.bg(rgb(p.selected)))
                                }),
                        ),
                    )
                    .child(action(
                        if self.history_project_only {
                            "This workspace ▾"
                        } else {
                            "All workspaces ▾"
                        },
                        p,
                        cx.listener(|this, _, _, cx| {
                            this.history_project_only = !this.history_project_only;
                            cx.notify();
                        }),
                    )),
            )
            .child(
                div()
                    .px(px(14.))
                    .py(px(8.))
                    .flex_shrink_0()
                    .child(self.search.clone()),
            )
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_between()
                    .px(px(18.))
                    .py(px(8.))
                    .child(small(format!("{count} agents discovered"), p.faint))
                    .child(
                        action(
                            if self.history_loading {
                                "Reading history…"
                            } else if self.history_complete {
                                "Rescan agent history"
                            } else {
                                "Load older agents"
                            },
                            p,
                            cx.listener(|this, _, _, cx| this.load_history(cx)),
                        )
                        .when(self.history_loading, |s| s.opacity(0.45)),
                    ),
            )
            .when(count == 0, |s| {
                s.child(empty(
                    if self.history_loading {
                        "Reading agent history…"
                    } else {
                        "No agents match this view"
                    },
                    "Try All providers or All workspaces. Your other agents remain in the sidebar.",
                    p,
                ))
            })
            .child(
                uniform_list(
                    "agent-history",
                    count,
                    cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                        range
                            .map(|row| {
                                let session = &this.known_sessions[filtered[row]];
                                let id = session.id.clone();
                                let source = session.source.clone();
                                let parent = this.known_sessions.iter().find(|s| {
                                    s.id == session.parent_id && s.source == session.source
                                });
                                let trace = if session.parent_id.is_empty() {
                                    format!("Root agent · {}", short_id(&session.id))
                                } else {
                                    format!(
                                        "{} → {}",
                                        parent
                                            .map(|p| if p.agent.is_empty() {
                                                short_id(&p.id)
                                            } else {
                                                p.agent.clone()
                                            })
                                            .unwrap_or_else(|| short_id(&session.parent_id)),
                                        if session.agent.is_empty() {
                                            short_id(&session.id)
                                        } else {
                                            session.agent.clone()
                                        }
                                    )
                                };
                                div()
                                    .id(("history-agent", row))
                                    .w_full()
                                    .h(px(84.))
                                    .flex()
                                    .items_center()
                                    .gap(px(12.))
                                    .px(px(18.))
                                    .border_b_1()
                                    .border_color(rgb(p.border))
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(rgb(p.raised)))
                                    .when(session.id == this.session_id, |s| s.bg(rgb(p.selected)))
                                    .child(
                                        div()
                                            .size(px(30.))
                                            .flex_shrink_0()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded(px(3.))
                                            .bg(rgb(p.raised))
                                            .text_color(rgb(p.accent))
                                            .child(
                                                source_label(&session.source)
                                                    .chars()
                                                    .next()
                                                    .unwrap_or('A')
                                                    .to_string(),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .flex_1()
                                            .w_full()
                                            .min_w_0()
                                            .gap(px(4.))
                                            .child(
                                                line(visuals::display_task(&session.title), p.text)
                                                    .w_full()
                                                    .line_clamp(1),
                                            )
                                            .child(
                                                small(
                                                    format!(
                                                        "{} · {}",
                                                        source_label(&session.source),
                                                        session.project
                                                    ),
                                                    p.faint,
                                                )
                                                .w_full()
                                                .line_clamp(1),
                                            )
                                            .child(small(trace, p.accent).w_full().line_clamp(1)),
                                    )
                                    .child(icon("right", p.faint))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.select_session_key_in(
                                            &source,
                                            id.clone(),
                                            1,
                                            window,
                                            cx,
                                        )
                                    }))
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .flex_1()
                .min_h_0()
                .w_full(),
            )
            .when(!self.history_warnings.is_empty(), |s| {
                s.child(
                    small(self.history_warnings.join(" · "), p.faint)
                        .px(px(18.))
                        .py(px(8.))
                        .line_clamp(2),
                )
            })
            .into_any_element()
    }
    fn catalog_roots(&self, source: &str) -> Vec<&Session> {
        let mut roots: Vec<_> = self
            .known_sessions
            .iter()
            .filter(|s| {
                s.source == source
                    && (s.parent_id.is_empty()
                        || !self
                            .known_sessions
                            .iter()
                            .any(|p| p.id == s.parent_id && p.source == source))
            })
            .take(4)
            .collect();
        if let Some(selected) = self.current_agent().filter(|s| s.source == source) {
            let mut root = selected;
            for _ in 0..32 {
                if let Some(parent) = self
                    .known_sessions
                    .iter()
                    .find(|s| s.id == root.parent_id && s.source == source)
                {
                    root = parent;
                } else {
                    break;
                }
            }
            if !roots.iter().any(|s| s.id == root.id) {
                roots.push(root);
            }
        }
        roots
    }
    fn activity_sessions(&self) -> Vec<Session> {
        let mut sessions = Vec::new();
        for source in ["deepseek", "codex", "opencode"] {
            for root in self.catalog_roots(source) {
                sessions.push(root.clone());
                sessions.extend(
                    self.known_sessions
                        .iter()
                        .filter(|s| s.source == source && s.parent_id == root.id)
                        .take(4)
                        .cloned(),
                );
            }
        }
        sessions.extend(self.current_agent().cloned());
        let mut keys = std::collections::HashSet::new();
        sessions.retain(|s| keys.insert((s.source.clone(), s.id.clone())));
        for session in &self.known_sessions {
            if sessions.len() >= 64 {
                break;
            }
            if keys.insert((session.source.clone(), session.id.clone())) {
                sessions.push(session.clone());
            }
        }
        sessions.retain(|s| self.engine_run(&s.source, &s.id).is_none());
        sessions.truncate(64);
        sessions
    }
    fn render_activity_lane(
        &self,
        session: &Session,
        child: bool,
        p: Palette,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let id = session.id.clone();
        let source = session.source.clone();
        let activity = self
            .agent_activity
            .iter()
            .find(|a| a.key.0 == session.source && a.key.1 == session.id);
        let fresh = self
            .activity_changes
            .get(&(session.source.clone(), session.id.clone()))
            .is_some_and(|at| at.elapsed() < Duration::from_secs(3));
        let age = activity
            .and_then(|a| a.file_modified)
            .and_then(|at| at.elapsed().ok())
            .map(|d| {
                if d.as_secs() < 60 {
                    format!("{}s ago", d.as_secs())
                } else if d.as_secs() < 3600 {
                    format!("{}m ago", d.as_secs() / 60)
                } else if d.as_secs() < 86400 {
                    format!("{}h ago", d.as_secs() / 3600)
                } else {
                    format!("{}d ago", d.as_secs() / 86400)
                }
            })
            .unwrap_or_default();
        let label = if child {
            agent_label(session)
        } else {
            name(&session.project)
        };
        let summary = activity
            .filter(|a| a.loaded && !a.text.is_empty())
            .map(|a| a.text.clone())
            .unwrap_or_else(|| visuals::display_task(&session.title));
        let mut trail = div()
            .flex()
            .items_end()
            .gap(px(2.))
            .h(px(16.))
            .w(px(142.))
            .flex_shrink_0();
        if let Some(activity) = activity {
            for kind in activity.samples.iter().skip(
                activity
                    .samples
                    .len()
                    .saturating_sub(self.settings.preferences.activity_lane_samples),
            ) {
                let (color, height) = match kind {
                    ActivityKind::User => (p.accent, 10.),
                    ActivityKind::Agent => (p.syntax_keyword, 13.),
                    ActivityKind::Tool => (p.muted, 7.),
                    ActivityKind::Completed => (p.green, 16.),
                    ActivityKind::Error => (p.red, 16.),
                    ActivityKind::Other => (p.faint, 4.),
                };
                trail = trail.child(div().w(px(4.)).h(px(height)).rounded(px(1.)).bg(rgb(color)));
            }
        }
        let dot = div()
            .size(px(6.))
            .rounded_full()
            .flex_shrink_0()
            .bg(rgb(if fresh { p.green } else { p.faint }));
        let signal = if fresh
            && self.settings.preferences.pulse_activity
            && !self.settings.preferences.reduced_motion
        {
            dot.with_animation(
                (
                    SharedString::from(format!("lane-{}", session.id)),
                    activity.map(|a| a.revision as usize).unwrap_or(0),
                ),
                Animation::new(Duration::from_millis(900))
                    .repeat()
                    .with_easing(bounce(ease_in_out)),
                |dot, delta| dot.opacity(0.4 + 0.6 * delta),
            )
            .into_any_element()
        } else {
            dot.into_any_element()
        };
        div()
            .id(SharedString::from(format!(
                "lane-{}-{}",
                session.source, session.id
            )))
            .flex()
            .items_center()
            .w_full()
            .min_w_0()
            .min_h(px(65.))
            .px(px(12.))
            .py(px(9.))
            .gap(px(12.))
            .border_b_1()
            .border_color(rgb(p.border))
            .flex_wrap()
            .when(child, |s| s.pl(px(28.)).border_l_1())
            .bg(rgb(
                if session.id == self.session_id && session.source == self.source {
                    p.selected
                } else {
                    p.base
                },
            ))
            .cursor_pointer()
            .hover(move |s| s.bg(rgb(p.raised)))
            .child(
                div()
                    .w(px(if child { 156. } else { 174. }))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(if child {
                        small("↳", p.faint).into_any_element()
                    } else {
                        icon("agent", p.accent).into_any_element()
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(line(label, p.text).w_full().line_clamp(1))
                            .child(
                                small(
                                    if child {
                                        short_id(&session.id)
                                    } else {
                                        format!(
                                            "{} · {}",
                                            source_label(&session.source),
                                            short_id(&session.id)
                                        )
                                    },
                                    p.faint,
                                )
                                .w_full()
                                .line_clamp(1),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_w(px(140.))
                    .flex_basis(px(140.))
                    .child(line(summary, p.muted).w_full().line_clamp(1))
                    .child(
                        small(
                            if let Some(error) = activity.and_then(|a| a.error.as_ref()) {
                                error.clone()
                            } else if let Some(a) = activity.filter(|a| a.loaded) {
                                format!(
                                    "{} events · {} tool events{}",
                                    a.event_count,
                                    a.tool_count,
                                    if a.error_count > 0 {
                                        format!(
                                            " · {} {}",
                                            a.error_count,
                                            if a.error_count == 1 {
                                                "error"
                                            } else {
                                                "errors"
                                            }
                                        )
                                    } else {
                                        String::new()
                                    }
                                )
                            } else {
                                "Reading activity…".into()
                            },
                            p.faint,
                        )
                        .w_full()
                        .line_clamp(1),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    .flex_shrink_0()
                    .child(trail)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .child(signal)
                            .child(small(
                                if fresh { "New activity".into() } else { age },
                                p.faint,
                            )),
                    ),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.select_session_key_in(&source, id.clone(), 1, window, cx)
            }))
    }
    fn render_agent_catalog(&self, p: Palette, cx: &mut Context<Self>) -> Div {
        let mut catalog = div()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .px(px(18.))
            .pb(px(12.));
        catalog = catalog.child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap(px(10.))
                .py(px(12.))
                .child(small("ACTIVITY BY AGENT", p.faint))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(small("● You", p.accent))
                        .child(small("● Agent", p.syntax_keyword))
                        .child(small("● Tool result", p.green)),
                ),
        );
        for source in ["deepseek", "codex", "opencode"] {
            let agents = self.catalog_roots(source);
            if agents.is_empty() {
                continue;
            }
            catalog = catalog.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .pt(px(16.))
                    .pb(px(8.))
                    .child(line(source_label(source), p.text).font_weight(FontWeight::MEDIUM))
                    .child(small(
                        format!(
                            "{} recorded",
                            self.known_sessions
                                .iter()
                                .filter(|s| s.source == source)
                                .count()
                        ),
                        p.faint,
                    )),
            );
            for session in agents {
                catalog = catalog.child(self.render_activity_lane(session, false, p, cx));
                let children: Vec<_> = self
                    .known_sessions
                    .iter()
                    .filter(|s| s.source == source && s.parent_id == session.id)
                    .collect();
                for child in children.iter().take(4) {
                    catalog = catalog.child(self.render_activity_lane(child, true, p, cx));
                }
                if children.len() > 4 {
                    let id = session.id.clone();
                    let source = session.source.clone();
                    let control_id = SharedString::from(format!("more-child-agents:{source}:{id}"));
                    catalog = catalog.child(
                        action(
                            format!("{} more child agents →", children.len() - 4),
                            p,
                            cx.listener(move |this, _, window, cx| {
                                this.select_session_key_in(&source, id.clone(), 1, window, cx);
                                this.agent_details = true;
                            }),
                        )
                        .id(control_id)
                        .ml(px(18.))
                        .justify_start(),
                    );
                }
            }
        }
        catalog.child(
            action(
                "Browse all agent history →",
                p,
                cx.listener(|this, _, window, cx| this.navigate(5, window, cx)),
            )
            .mt(px(12.))
            .justify_start(),
        )
    }
    fn render_dock(&self, width: f32, p: Palette, cx: &mut Context<Self>) -> Div {
        let mut dock = div()
            .flex()
            .flex_col()
            .w(px(width))
            .h_full()
            .flex_shrink_0()
            .min_h_0()
            .bg(rgb(p.panel))
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(36.))
                    .flex_shrink_0()
                    .gap(px(2.))
                    .px(px(8.))
                    .border_b_1()
                    .bg(rgb(p.raised))
                    .border_color(rgb(p.border))
                    .child(
                        action(
                            "Changes",
                            p,
                            cx.listener(|this, _, _, cx| {
                                this.dock_tab = 0;
                                this.request_diff(false, cx);
                                cx.notify();
                            }),
                        )
                        .when(self.dock_tab == 0, |s| {
                            s.text_color(rgb(p.text)).bg(rgb(p.raised))
                        }),
                    )
                    .child(
                        action(
                            "Details",
                            p,
                            cx.listener(|this, _, _, cx| {
                                this.dock_tab = 1;
                                cx.notify();
                            }),
                        )
                        .when(self.dock_tab == 1, |s| {
                            s.text_color(rgb(p.text)).bg(rgb(p.raised))
                        }),
                    )
                    .child(div().flex_1())
                    .child(action(
                        "×",
                        p,
                        cx.listener(|this, _, _, cx| {
                            this.dock = false;
                            cx.notify();
                        }),
                    )),
            );
        if self.dock_tab == 1 {
            dock = dock
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .h(px(36.))
                        .px(px(12.))
                        .child(small(self.detail_title.clone(), p.muted))
                        .child(action(
                            "Copy",
                            p,
                            cx.listener(|this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    this.detail.source().to_owned(),
                                ))
                            }),
                        )),
                )
                .child(self.render_event_evidence(p, cx));
        } else {
            dock = dock
                .child(div().px(px(12.)).py(px(14.)).child(line(
                    format!(
                        "{}  ·  {} files",
                        self.repository.branch,
                        self.repository.changes.len()
                    ),
                    p.muted,
                )))
                .child(
                    div()
                        .h(px(180.))
                        .flex_shrink_0()
                        .border_b_1()
                        .border_color(rgb(p.border))
                        .child(self.file_list("changes-dock", p, cx)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .h(px(36.))
                        .px(px(12.))
                        .child(
                            small(self.selected_file.clone(), p.muted)
                                .line_clamp(1)
                                .flex_1(),
                        )
                        .child(self.render_open_file_button("open-with-dock", p, cx))
                        .child(action(
                            "Copy",
                            p,
                            cx.listener(|this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    this.diff.source().to_owned(),
                                ))
                            }),
                        )),
                )
                .child(self.code_view("dock-diff", self.diff.clone(), p, cx, false));
        }
        dock
    }
    fn render_palette(&self, p: Palette, cx: &mut Context<Self>) -> Div {
        let commands = self.palette_commands();
        let result_height = commands.len().clamp(1, 11) as f32 * 32.;
        div()
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .pt(px(70.))
            .bg(rgba(0x00000055))
            .child(
                div()
                    .id("command-panel")
                    .h(px(result_height + 100.))
                    .w(px(560.))
                    .max_h(px(500.))
                    .flex()
                    .flex_col()
                    .bg(rgb(p.panel))
                    .border_1()
                    .border_color(rgb(p.border))
                    .rounded(px(7.))
                    .shadow_lg()
                    .child(div().p(px(10.)).child(self.palette_input.clone()))
                    .child(
                        div()
                            .id("command-results")
                            .h(px(result_height))
                            .max_h(px(400.))
                            .overflow_y_scroll()
                            .children(commands.iter().enumerate().map(
                                |(index, (id, title, shortcut))| {
                                    let id = *id;
                                    div()
                                        .id(("command", id))
                                        .flex()
                                        .items_center()
                                        .h(px(32.))
                                        .px(px(14.))
                                        .gap(px(12.))
                                        .cursor_pointer()
                                        .hover(move |s| s.bg(rgb(p.raised)))
                                        .when(index == self.palette_index, |s| {
                                            s.bg(rgb(p.selected))
                                        })
                                        .child(line(title.clone(), p.text).flex_1())
                                        .child(small(shortcut.clone(), p.faint))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.run_command(id, window, cx)
                                        }))
                                },
                            ))
                            .when(commands.is_empty(), |s| {
                                s.child(line("No commands match", p.muted).p(px(18.)))
                            }),
                    )
                    .child(
                        small("↑ ↓  Navigate      ↵  Run command      esc  Close", p.faint)
                            .px(px(14.))
                            .py(px(10.))
                            .border_t_1()
                            .border_color(rgb(p.border)),
                    ),
            )
    }
}
impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let system_light = matches!(
            window.appearance(),
            WindowAppearance::Light | WindowAppearance::VibrantLight
        );
        let p = Palette::for_theme(&self.settings.appearance, system_light);
        zed_editor_runtime::apply_palette(p, cx);
        self.search.update(cx, |input, cx| {
            input.set_placeholder(
                if self.pane == 8 {
                    "Search agents and skills…"
                } else if self.pane == 6 {
                    "Search settings…"
                } else {
                    "Search recorded work…"
                },
                cx,
            );
        });
        for input in [
            &self.search,
            &self.palette_input,
            &self.project_input,
            &self.worker_input,
            &self.state_input,
            &self.launch_task,
            &self.launch_title,
            &self.launch_workspace,
            &self.launch_skill_search,
        ] {
            input.update(cx, |input, cx| {
                input.set_colors(p.raised, p.text, p.faint, p.control_border, cx);
                input.set_interaction_colors(p.focus, p.text_selection, cx)
            });
        }
        let width: f32 = window.viewport_size().width.into();
        let show_dock = self.chat.open
            || if self.pane == 7 || self.editor.open {
                self.graph.inspector_open
                    && (self.graph.creating_agent || self.graph.selected.is_some())
            } else {
                self.pane != 8 && self.dock
            };
        let side_width = if self.sidebar {
            if width < 1180. && show_dock {
                52.
            } else {
                self.settings.sidebar_width.clamp(190., 320.)
            }
        } else {
            0.
        };
        let desired_files = if self.tools.files_open { 230. } else { 0. };
        let dock_width = if show_dock {
            self.settings
                .dock_width
                .clamp(300., (width - side_width - desired_files - 430.).max(300.))
        } else {
            0.
        };
        let file_width = if self.tools.files_open {
            (width - side_width - dock_width - 430.).clamp(160., 230.)
        } else {
            0.
        };
        let center_width = (width - side_width - dock_width - file_width - 4.).max(100.);
        let center_height = (f32::from(window.viewport_size().height)
            - 66.
            - if self.message.is_empty() { 0. } else { 30. })
        .max(100.);
        let console_height = if self.tools.console_open {
            260_f32.min(center_height * 0.45)
        } else {
            0.
        };
        let pane = if self.editor.open {
            self.render_workspace_editor(p, cx)
        } else {
            match self.pane {
                0 => self.render_dashboard(p, center_width, cx),
                1 => self.render_activity(p, center_width, cx),
                2 => self.render_processes(p, cx),
                3 => self.render_increments(p, cx),
                4 => self.render_changes(p, cx),
                5 => self.render_sessions(p, cx),
                8 => self.render_catalog(p, cx),
                7 => self.render_graph(p, center_width, center_height - console_height, cx),
                _ => self.render_preferences(p, cx),
            }
        };
        div().id("workspace").key_context(if self.palette_open{"Workspace Palette"}else{"Workspace"}).track_focus(&self.focus).relative().size_full().flex().flex_col().overflow_hidden()
            .font_family(style::UI_FONT).text_size(px(13.)).text_color(rgb(p.text)).bg(rgb(p.base))
            .on_action(cx.listener(|this,_:&OpenWorkspace,_,cx|this.choose_path(0,cx)))
            .on_action(cx.listener(|this,_:&Refresh,_,cx|this.manual_refresh(cx)))
            .on_action(cx.listener(|this,_:&AskPane,_,cx|this.ask_current_pane(cx)))
            .on_action(cx.listener(|this,_:&SendPaneChat,window,cx|{if this.chat.open && this.chat.question.focus_handle(cx).is_focused(window){this.send_pane_question(cx)}else{cx.propagate()}}))
            .on_action(cx.listener(|this,_:&ToggleFiles,_,cx|this.toggle_files(cx)))
            .on_action(cx.listener(|this,_:&ToggleConsole,_,cx|this.toggle_console(cx)))
            .on_action(cx.listener(|this,_:&OpenFile,_,cx|this.choose_editor_file(cx)))
            .on_action(cx.listener(|this,_:&SaveFile,_,cx|{if this.editor.open {this.save_workspace_file(cx)}else{cx.propagate()}}))
            .on_action(cx.listener(|this,_:&CommandPalette,window,cx|this.open_palette(window,cx)))
            .on_action(cx.listener(|this,_:&ToggleSidebar,_,cx|{this.sidebar=!this.sidebar;cx.notify();}))
            .on_action(cx.listener(|this,_:&ToggleDock,_,cx|this.toggle_dock(cx)))
            .on_action(cx.listener(|this,_:&Back,window,cx|this.history_move(-1,window,cx)))
            .on_action(cx.listener(|this,_:&Forward,window,cx|this.history_move(1,window,cx)))
            .on_action(cx.listener(|this,_:&Overview,window,cx|this.navigate(0,window,cx)))
            .on_action(cx.listener(|this,_:&Activity,window,cx|this.navigate(1,window,cx)))
            .on_action(cx.listener(|this,_:&Processes,window,cx|this.navigate(2,window,cx)))
            .on_action(cx.listener(|this,_:&Increments,window,cx|this.navigate(3,window,cx)))
            .on_action(cx.listener(|this,_:&Changes,window,cx|this.navigate(4,window,cx)))
            .on_action(cx.listener(|this,_:&Sessions,window,cx|this.navigate(5,window,cx)))
            .on_action(cx.listener(|this,_:&SettingsView,window,cx|this.navigate(6,window,cx)))
            .on_action(cx.listener(|this,_:&Graph,window,cx|this.navigate(7,window,cx)))
            .on_action(cx.listener(|this,_:&AgentsSkills,window,cx|this.navigate(8,window,cx)))
            .on_action(cx.listener(|this,_:&Escape,window,cx|{if this.palette_input.read(cx).is_composing(){return;}this.palette_open=false;this.file_menu=None;this.confirmation=None;this.worker_confirmation=false;window.focus(&this.focus, cx);cx.notify();}))
            .on_action(cx.listener(|this,_:&PaletteUp,_,cx|{this.palette_index=this.palette_index.saturating_sub(1);cx.notify();}))
            .on_action(cx.listener(|this,_:&PaletteDown,_,cx|{this.palette_index=(this.palette_index+1).min(this.palette_commands().len().saturating_sub(1));cx.notify();}))
            .on_action(cx.listener(|this,_:&PaletteRun,window,cx|{if !this.palette_input.read(cx).is_composing(){if let Some((id,_,_))=this.palette_commands().get(this.palette_index){this.run_command(*id,window,cx);}}}))
            .on_action(cx.listener(|this,_:&Find,window,cx|this.search.focus_handle(cx).focus(window, cx)))
            .on_action(cx.listener(|this,_:&code_view::ZoomIn,_,cx|this.zoom_viewer(1,cx)))
            .on_action(cx.listener(|this,_:&code_view::ZoomOut,_,cx|this.zoom_viewer(-1,cx)))
            .on_action(cx.listener(|this,_:&code_view::ZoomReset,_,cx|this.zoom_viewer(0,cx)))
            .on_mouse_move(cx.listener(|this,event:&MouseMoveEvent,window,cx|{if let Some((kind,previous))=this.drag{
                if !event.dragging(){return;}let x:f32=event.position.x.into();let delta=x-previous;this.drag=Some((kind,x));let width:f32=window.viewport_size().width.into();
                match kind {0=>this.settings.sidebar_width=(this.settings.sidebar_width+delta).clamp(190.,320.),1=>this.settings.dock_width=(this.settings.dock_width-delta).clamp(300.,(width-520.).max(300.)),_=>this.increment_width=(this.increment_width+delta).clamp(160.,400.)}cx.notify();
            }}))
            .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _, cx| { this.recover_graph_double_click(event, cx); }))
            .capture_any_mouse_up(cx.listener(|this, event: &MouseUpEvent, _, cx| { this.recover_graph_double_click_release(event, cx); }))
            .on_mouse_up(MouseButton::Left,cx.listener(|this,_,_,cx|{if this.drag.take().is_some(){this.persist();cx.notify();}}))
            .child(self.render_titlebar(p,cx))
            .when(self.settings.preferences.show_activity_bar && self.pane != 0 && self.pane != 6 && self.pane != 7 && self.pane!=8,|s|s.child(self.render_agent_context(p,cx)))
            .child(div().flex().flex_1().min_h_0().when(self.sidebar,|s|s.child(self.render_sidebar(side_width,p,cx)).child(self.render_divider(0,p,cx)))
                .when(self.tools.files_open,|s|s.child(self.render_file_tree(file_width,p,cx)))
                .child(div().flex().flex_col().flex_1().min_w_0().min_h_0().overflow_hidden().child(pane).when(self.tools.console_open,|s|s.child(self.render_console(console_height,p,cx))))
                .when(show_dock,|s|s.child(self.render_divider(1,p,cx)).child(if self.chat.open {self.render_pane_chat(dock_width,p,cx)}else if self.pane == 7 || self.editor.open { self.render_graph_sidebar(dock_width,p,cx) } else { self.render_dock(dock_width,p,cx) })))
            .when(!self.quitting && self.settings_save_error().is_some(),|s|s.child(self.render_settings_failure(p,cx)))
            .when(!self.message.is_empty(),|s|s.child(div().flex().items_center().justify_between().px(px(12.)).h(px(30.)).bg(rgb(p.raised)).border_t_1().border_color(rgb(p.border)).child(small(self.message.clone(),p.muted)).child(action("×",p,cx.listener(|this,_,_,cx|{this.message.clear();cx.notify();})))))
            .child(div().flex().items_center().h(px(26.)).flex_shrink_0().px(px(12.)).gap(px(12.)).bg(rgb(p.panel)).border_t_1().border_color(rgb(p.border))
                .child(icon("folder",p.faint)).child(small(name(&self.settings.project),p.muted)).child(small(self.repository.branch.clone(),p.faint))
                .child(small(if self.worker.label.is_empty(){"Read-only watcher".into()}else{format!("Worker: {}",self.worker.label)},p.muted)).child(div().flex_1())
                .child(small(if self.loading{"Reading recorded activity…".into()}else if self.agent_details{format!("History {:.0} ms  ·  Updated {}s ago",self.telemetry_duration.as_secs_f64()*1000.,self.updated.elapsed().as_secs())}else{format!("Updated {}s ago",self.updated.elapsed().as_secs())},p.faint)))
            .when(self.quitting,|s|s.child(self.render_quit_status(p,cx)))
            .when(self.editor_needs_confirmation(),|s|s.child(self.render_editor_confirmation(p,cx)))
            .when(self.file_menu.is_some(),|s|s.child(self.render_file_menu(p,cx)))
            .when(self.palette_open,|s|s.child(self.render_palette(p,cx)))
            .when(self.confirmation.is_some() || self.worker_confirmation, |s| {
                let stopping_worker = self.worker_confirmation;
                let title = if stopping_worker {
                    "Stop configured worker?".to_owned()
                } else {
                    format!("Stop process {}?", self.confirmation.as_ref().unwrap().pid)
                };
                let explanation = if stopping_worker {
                    "Stop the configured worker and its verified child processes. Active work may be interrupted."
                } else {
                    "Send SIGTERM to this agent process. Its identity will be checked again before stopping."
                };
                s.child(div().absolute().inset_0().flex().items_center().justify_center().bg(rgba(0x00000066))
                    .child(div().w(px(420.)).p(px(22.)).bg(rgb(p.panel)).border_1().border_color(rgb(p.border)).rounded(px(6.))
                        .child(line(title,p.text)).child(line(explanation,p.muted).mt(px(12.)))
                        .child(div().flex().justify_end().gap(px(8.)).mt(px(18.))
                            .child(action("Cancel",p,cx.listener(|this,_,_,cx|{this.confirmation=None;this.worker_confirmation=false;cx.notify();})))
                            .child(action(if stopping_worker {"Stop worker"}else{"Stop process"},p,cx.listener(|this,_,_,cx|this.operation("stop",if this.worker_confirmation{None}else{this.confirmation.clone()},cx))).text_color(rgb(p.red))))))
            })
    }
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let mut settings = Settings::load();
    if let Some(i) = args.iter().position(|a| a == "--project") {
        if let Some(path) = args.get(i + 1) {
            settings.project = path.clone();
        }
    }
    settings.normalize();
    if args.iter().any(|a| a == "--diagnose") {
        let start = Instant::now();
        let repo = backend::repository(&settings.project);
        let git_ms = start.elapsed().as_millis();
        let start = Instant::now();
        let sessions = telemetry::discover(&settings.project, "automatic");
        let catalog_ms = start.elapsed().as_millis();
        let start = Instant::now();
        let snapshot = sessions.first().map(telemetry::collect).unwrap_or_default();
        println!(
            "{}",
            serde_json::json!({"framework":"Rust + GPUI","git_ms":git_ms,"catalog_ms":catalog_ms,"session_ms":start.elapsed().as_millis(),"commits":repo.commits.len(),"changed_files":repo.changes.len(),"sessions":sessions.len(),"records":snapshot.records.len(),"children":snapshot.children.len(),"error":snapshot.error})
        );
        return;
    }
    gpui_platform::application()
        .with_assets(Assets)
        .run(move |cx| {
            style::register_fonts(cx).expect("Bundled Zed fonts must load");
            zed_editor_runtime::init(cx);
            input::init(cx);

            cx.bind_keys([
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("cmd-o", OpenWorkspace, Some("Workspace")),
                KeyBinding::new("cmd-r", Refresh, Some("Workspace")),
                KeyBinding::new("cmd-shift-p", CommandPalette, Some("Workspace")),
                KeyBinding::new("cmd-k", CommandPalette, Some("Workspace")),
                KeyBinding::new("cmd-b", ToggleSidebar, Some("Workspace")),
                KeyBinding::new("cmd-alt-i", ToggleDock, Some("Workspace")),
                KeyBinding::new("cmd-[", Back, Some("Workspace")),
                KeyBinding::new("cmd-]", Forward, Some("Workspace")),
                KeyBinding::new("cmd-f", Find, Some("Workspace")),
                KeyBinding::new("cmd-=", code_view::ZoomIn, Some("Workspace")),
                KeyBinding::new("cmd-+", code_view::ZoomIn, Some("Workspace")),
                KeyBinding::new("cmd--", code_view::ZoomOut, Some("Workspace")),
                KeyBinding::new("cmd-0", code_view::ZoomReset, Some("Workspace")),
                KeyBinding::new("ctrl-=", code_view::ZoomIn, Some("Workspace")),
                KeyBinding::new("ctrl-+", code_view::ZoomIn, Some("Workspace")),
                KeyBinding::new("ctrl--", code_view::ZoomOut, Some("Workspace")),
                KeyBinding::new("ctrl-0", code_view::ZoomReset, Some("Workspace")),
                KeyBinding::new("cmd-1", Overview, Some("Workspace")),
                KeyBinding::new("cmd-2", Activity, Some("Workspace")),
                KeyBinding::new("cmd-3", Processes, Some("Workspace")),
                KeyBinding::new("cmd-4", Increments, Some("Workspace")),
                KeyBinding::new("cmd-5", Changes, Some("Workspace")),
                KeyBinding::new("cmd-6", Sessions, Some("Workspace")),
                KeyBinding::new("cmd-7", SettingsView, Some("Workspace")),
                KeyBinding::new("cmd-8", Graph, Some("Workspace")),
                KeyBinding::new("cmd-9", AgentsSkills, Some("Workspace")),
                KeyBinding::new("cmd-s", SaveFile, Some("Workspace")),
                KeyBinding::new("cmd-shift-o", OpenFile, Some("Workspace")),
                KeyBinding::new("cmd-shift-l", AskPane, Some("Workspace")),
                KeyBinding::new("cmd-enter", SendPaneChat, Some("Workspace")),
                KeyBinding::new("cmd-shift-e", ToggleFiles, Some("Workspace")),
                KeyBinding::new("cmd-`", ToggleConsole, Some("Workspace")),
                KeyBinding::new("escape", Escape, Some("Workspace")),
                KeyBinding::new("up", PaletteUp, Some("Palette")),
                KeyBinding::new("down", PaletteDown, Some("Palette")),
                KeyBinding::new("enter", PaletteRun, Some("Palette")),
            ]);
            cx.set_menus(vec![
                Menu {
                    name: "AgentWatcher".into(),
                    disabled: false,
                    items: vec![
                        MenuItem::action("Command Palette", CommandPalette),
                        MenuItem::separator(),
                        MenuItem::os_submenu("Services", SystemMenuType::Services),
                        MenuItem::separator(),
                        MenuItem::action("Quit AgentWatcher", Quit),
                    ],
                },
                Menu {
                    name: "File".into(),
                    disabled: false,
                    items: vec![
                        MenuItem::action("Open Workspace…", OpenWorkspace),
                        MenuItem::action("Refresh", Refresh),
                    ],
                },
                Menu {
                    name: "View".into(),
                    disabled: false,
                    items: vec![
                        MenuItem::action("Toggle Sidebar", ToggleSidebar),
                        MenuItem::action("Toggle Changes Dock", ToggleDock),
                        MenuItem::separator(),
                        MenuItem::action("Increase Viewer Font Size", code_view::ZoomIn),
                        MenuItem::action("Decrease Viewer Font Size", code_view::ZoomOut),
                        MenuItem::action("Reset Viewer Font Size", code_view::ZoomReset),
                    ],
                },
            ]);
            let workspace_window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                            None,
                            size(px(1440.), px(900.)),
                            cx,
                        ))),
                        window_min_size: Some(size(px(900.), px(640.))),
                        titlebar: Some(TitlebarOptions {
                            title: Some("AgentWatcher".into()),
                            appears_transparent: true,
                            traffic_light_position: Some(point(px(12.), px(12.))),
                        }),
                        ..Default::default()
                    },
                    move |window, cx| cx.new(|cx| Workspace::new(settings, window, cx)),
                )
                .expect("Open AgentWatcher workspace");
            cx.on_action(move |_: &Quit, cx| {
                // Action dispatch already has this window on the stack.
                cx.defer(move |cx| {
                    let _ = workspace_window.update(cx, |this, _, cx| this.request_engine_quit(cx));
                });
            });
            cx.on_native_quit_requested(move |cx| {
                workspace_window
                    .update(cx, |this, window, cx| {
                        this.poll_settings_persistence();
                        if this.native_quit_ready(cx) {
                            return true;
                        }
                        window.activate_window();
                        cx.activate(true);
                        this.request_engine_quit(cx);
                        false
                    })
                    .unwrap_or(false)
            });
            let quit_handle = workspace_window
                .update(cx, |this, _, _| this.engine.shutdown_handle())
                .expect("Engine shutdown handle");
            let chat_quit = workspace_window
                .update(cx, |this, _, _| this.chat.client.clone())
                .expect("Chat shutdown handle");
            let console_quit = workspace_window
                .update(cx, |this, _, _| this.tools.terminal.shutdown_handle())
                .expect("Console shutdown handle");
            let settings_quit = workspace_window
                .update(cx, |this, _, _| this.settings_writer.shutdown_handle())
                .expect("Settings shutdown handle");
            cx.on_app_quit(move |cx| {
                let discarded = workspace_window
                    .update(cx, |this, _, _| this.settings_quit_discard)
                    .unwrap_or(false);
                let settings_quit = settings_quit.clone();
                chat_quit.shutdown();
                console_quit.force_shutdown();
                quit_handle.force_shutdown();
                async move {
                    // Cancellable quit requests are gated above, while the window can still
                    // report errors. GPUI's final shutdown budget is 200 ms.
                    let result =
                        smol::unblock(move || settings_quit.finish(Duration::from_millis(150)))
                            .await;
                    if let Err(error) = result {
                        if !discarded {
                            eprintln!("AgentWatcher could not finish saving settings: {error}");
                        }
                    }
                }
            })
            .detach();

            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.activate(true);
        });
}

#[cfg(test)]
mod native_quit_readiness_tests {
    use super::{NativeQuitReadiness, should_resume_native_quit};

    fn ready() -> NativeQuitReadiness {
        NativeQuitReadiness {
            requested: true,
            settings_ready: true,
            processes_idle: true,
            editor_busy: false,
            has_dirty_documents: false,
            documents_approved: false,
        }
    }

    #[test]
    fn native_quit_must_first_enter_the_existing_visible_quit_flow() {
        assert!(
            !NativeQuitReadiness {
                requested: false,
                ..ready()
            }
            .can_terminate()
        );
        assert!(ready().can_terminate());
    }

    #[test]
    fn native_quit_waits_for_preferences_and_all_local_work() {
        assert!(
            !NativeQuitReadiness {
                settings_ready: false,
                ..ready()
            }
            .can_terminate()
        );
        assert!(
            !NativeQuitReadiness {
                processes_idle: false,
                ..ready()
            }
            .can_terminate()
        );
        assert!(
            !NativeQuitReadiness {
                editor_busy: true,
                documents_approved: true,
                ..ready()
            }
            .can_terminate()
        );
    }

    #[test]
    fn dirty_documents_need_the_existing_explicit_close_approval() {
        let dirty = NativeQuitReadiness {
            has_dirty_documents: true,
            ..ready()
        };
        assert!(!dirty.can_terminate());
        assert!(
            NativeQuitReadiness {
                documents_approved: true,
                ..dirty
            }
            .can_terminate()
        );
        assert!(
            !NativeQuitReadiness {
                documents_approved: true,
                settings_ready: false,
                ..dirty
            }
            .can_terminate()
        );
    }

    #[test]
    fn native_request_resumes_after_busy_editor_completion() {
        // The initial request cannot enter the visible quit flow while a local
        // save/open is in flight. Its completion must not require another Quit.
        assert!(!should_resume_native_quit(true, false, true, false));
        assert!(should_resume_native_quit(true, false, false, false));
        // Once the normal quit flow starts, it owns the remaining shutdown.
        assert!(!should_resume_native_quit(true, true, false, false));
    }

    #[test]
    fn confirmation_waits_for_user_and_cancel_does_not_reopen_it() {
        assert!(!should_resume_native_quit(true, false, false, true));
        // Explicit Cancel clears pending before the asynchronous AppKit reply.
        assert!(!should_resume_native_quit(false, false, false, false));
        // A fresh native request can show the confirmation again.
        assert!(should_resume_native_quit(true, false, false, false));
    }
}

#[cfg(test)]
mod time_scope_tests {
    use super::record_in_time_range;
    #[test]
    fn outcome_filter_uses_recorded_status_and_includes_standalone_errors() {
        use super::record_matches_outcome;
        use agentwatcher::{activity::ActivityKind, telemetry::Record};
        let done = Record {
            kind: "tool".into(),
            tool_status: "Completed".into(),
            summary: "text contains Error".into(),
            ..Record::default()
        };
        let pending = Record {
            kind: "tool".into(),
            summary: "text contains Completed".into(),
            ..Record::default()
        };
        let error = Record {
            kind: "error".into(),
            ..Record::default()
        };
        assert!(record_matches_outcome(&done, Some(ActivityKind::Completed)));
        assert!(!record_matches_outcome(&done, Some(ActivityKind::Error)));
        assert!(record_matches_outcome(&pending, Some(ActivityKind::Tool)));
        assert!(!record_matches_outcome(
            &pending,
            Some(ActivityKind::Completed)
        ));
        assert!(record_matches_outcome(&error, Some(ActivityKind::Error)));
        assert!(!record_matches_outcome(&error, Some(ActivityKind::Tool)));
        assert!(record_matches_outcome(&error, None));
    }
    #[test]
    fn selected_slice_uses_same_inclusive_start_exclusive_end_for_both_providers() {
        let start = agentwatcher::activity::timestamp_seconds("2026-10-14T10:00:00Z").unwrap();
        let range = Some((start, start + 900));
        assert!(record_in_time_range("2026-10-14T10:00:00Z", range));
        assert!(record_in_time_range(
            &((start + 899) * 1000).to_string(),
            range
        ));
        assert!(!record_in_time_range("2026-10-14T10:15:00Z", range));
        assert!(!record_in_time_range("unrecorded", range));
        assert!(record_in_time_range("unrecorded", None));
    }
}

#[cfg(test)]
mod provider_selection_tests {
    use super::select_recording;
    use agentwatcher::telemetry::Session;
    #[test]
    fn colliding_ids_keep_provider_during_retained_and_catalog_refresh() {
        let sessions = vec![
            Session {
                source: "codex".into(),
                id: "same".into(),
                ..Session::default()
            },
            Session {
                source: "deepseek".into(),
                id: "same".into(),
                ..Session::default()
            },
        ];
        let selected = select_recording("deepseek", "same", None, &sessions).unwrap();
        assert_eq!(selected.source, "deepseek");
        assert_eq!(
            select_recording("deepseek", "same", Some(selected), &sessions)
                .unwrap()
                .source,
            "deepseek"
        );
        // A mismatched retained session is never accepted, even on a later poll.
        assert_eq!(
            select_recording("deepseek", "same", Some(&sessions[0]), &sessions)
                .unwrap()
                .source,
            "deepseek"
        );
        assert!(select_recording("opencode", "same", Some(selected), &sessions).is_none());
        assert!(select_recording("deepseek", "missing", None, &sessions).is_none());
    }
}

#[cfg(test)]
mod action_identity_tests {
    use super::action_identity;
    #[test]
    fn equal_labels_at_distinct_controls_do_not_share_click_state() {
        let close_inspector = action_identity("×");
        let close_evidence = action_identity("×");
        assert_ne!(close_inspector, close_evidence);
        fn stable(label: &str) -> gpui::SharedString {
            action_identity(label)
        }
        assert_eq!(stable("Copy"), stable("Copy"));
        assert_ne!(stable("Visual"), stable("Tree"));
    }
}
