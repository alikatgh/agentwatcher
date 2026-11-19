//! Bounded, read-only readers for recorded agent sessions. Call from a worker thread.
use regex::Regex;
use serde_json::{Value, json};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File};
#[cfg(test)]
use std::io::Cursor;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const HEADER_BYTES: usize = 32 * 1024;
const CATALOG_HEADERS: usize = 256;
const CATALOG_BYTES: usize = 8 * 1024 * 1024;
const SESSION_BYTES: usize = 16 * 1024 * 1024;
const SESSION_EVENTS: usize = 20_000;
const DECODED_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub project: String,
    pub path: String,
    pub source: String,
    pub updated: String,
    pub parent_id: String,
    pub agent: String,
}

#[derive(Clone, Debug, Default)]
pub struct Record {
    pub id: String,
    pub time: String,
    pub kind: String,
    pub summary: String,
    pub detail: String,
    /// Normalized exact-call status; empty for generic/unpaired-schema records.
    pub tool_status: String,
    pub is_infrastructure: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ChildAgent {
    pub id: String,
    pub title: String,
    pub session_id: String,
    pub agent: String,
    pub status: String,
}

#[derive(Clone, Debug, Default)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub total: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub session: Session,
    pub records: Vec<Record>,
    pub children: Vec<ChildAgent>,
    pub usage: Option<Usage>,
    pub model: String,
    pub warnings: Vec<String>,
    pub error: Option<String>,
    pub window_start: u64,
    pub window_end: u64,
    pub has_earlier: bool,
}

#[derive(Clone, Debug, Default)]
pub struct HistoryPage {
    pub sessions: Vec<Session>,
    pub next_cursor: Option<String>,
    pub complete: bool,
    pub warnings: Vec<String>,
}

struct CatalogBudget {
    start: Instant,
    headers: usize,
    bytes: usize,
    header_limit: usize,
    time_limit: Duration,
}

impl CatalogBudget {
    fn new() -> Self {
        Self {
            start: Instant::now(),
            headers: 0,
            bytes: 0,
            header_limit: CATALOG_HEADERS,
            time_limit: Duration::from_secs(1),
        }
    }

    fn available(&self) -> bool {
        self.headers < self.header_limit
            && self.bytes < CATALOG_BYTES
            && self.start.elapsed() < self.time_limit
    }

    fn prefix(&mut self, path: &Path) -> Option<Vec<u8>> {
        if !self.available() {
            return None;
        }
        self.headers += 1;
        let maximum = HEADER_BYTES.min(CATALOG_BYTES.saturating_sub(self.bytes));
        let mut bytes = Vec::new();
        File::open(path)
            .ok()?
            .take(maximum as u64)
            .read_to_end(&mut bytes)
            .ok()?;
        self.bytes += bytes.len();
        Some(bytes)
    }
}

/// Fast project-scoped discovery. Automatic includes both local providers;
/// global/older history uses discover_history. OpenCode remains explicit.
pub fn discover(project: &str, source: &str) -> Vec<Session> {
    if source == "opencode" {
        return open_code_sessions(project).unwrap_or_default();
    }
    let Some(home) = std::env::var_os("HOME") else {
        return Vec::new();
    };
    discover_in(project, source, Path::new(&home))
}

fn discover_in(project: &str, source: &str, home: &Path) -> Vec<Session> {
    if project.trim().is_empty() {
        return Vec::new();
    }
    let mut budget = CatalogBudget::new();
    let mut sessions = match source {
        "codex" => codex_catalog(project, &home.join(".codex/sessions"), &mut budget),
        "deepseek" => deepseek_catalog(project, &home.join(".dsh"), &mut budget),
        "automatic" => {
            budget.header_limit = CATALOG_HEADERS / 2;
            budget.time_limit = Duration::from_millis(500);
            let mut found = codex_catalog(project, &home.join(".codex/sessions"), &mut budget);
            budget.header_limit = CATALOG_HEADERS;
            budget.time_limit = Duration::from_secs(1);
            found.extend(deepseek_catalog(project, &home.join(".dsh"), &mut budget));
            found
        }
        // Explicit external discovery is handled by open_code_sessions, so this
        // inexpensive local catalog never starts a subprocess unexpectedly.
        _ => Vec::new(),
    };
    sessions.sort_by(|a, b| {
        let left = a.updated.parse::<u64>().unwrap_or(0);
        let right = b.updated.parse::<u64>().unwrap_or(0);
        right.cmp(&left).then_with(|| a.id.cmp(&b.id))
    });
    sessions
}

fn codex_catalog(project: &str, root: &Path, budget: &mut CatalogBudget) -> Vec<Session> {
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    let day = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
        / 86_400;
    // Only date-addressed directories from the last 45 days are enumerated.
    // Neither the Codex home nor complete conversations are recursively read.
    for offset in -1..45 {
        if !budget.available() {
            break;
        }
        let (year, month, day) = civil_date(day - offset);
        let directory = root.join(format!("{year:04}/{month:02}/{day:02}"));
        if !safe_directory(&directory, root) {
            continue;
        }
        let mut candidates = entries(&directory, CATALOG_HEADERS * 2, budget)
            .into_iter()
            .filter(|path| {
                path.file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.starts_with("rollout-") && s.ends_with(".jsonl"))
                    && safe_file(path, root)
            })
            .collect::<Vec<_>>();
        candidates.sort_by_cached_key(|path| std::cmp::Reverse(modified(path)));
        for path in candidates {
            if !budget.available() {
                break;
            }
            if let Some(session) = codex_descriptor(&path, project, budget) {
                if seen.insert(session.id.clone()) {
                    found.push(session);
                }
            }
        }
    }
    found
}

fn codex_descriptor(path: &Path, project: &str, budget: &mut CatalogBudget) -> Option<Session> {
    let Some(bytes) = budget.prefix(&path) else {
        return None;
    };
    let Some(header) = first_json_line(&bytes) else {
        return None;
    };
    let data = &header["payload"];
    let id = text(first(data, &["id", "session_id"]));
    if text(&header["type"]) != "session_meta"
        || !valid_id(&id, "codex")
        || !path.to_string_lossy().ends_with(&format!("{id}.jsonl"))
        || !project_matches(&text(&data["cwd"]), project)
        || internal_codex(data)
    {
        return None;
    }
    let spawn = &data["source"]["subagent"]["thread_spawn"];
    let agent = ["agent_nickname", "agent_role", "agent_path"]
        .iter()
        .map(|key| {
            text(if spawn[*key].is_null() {
                &data[*key]
            } else {
                &spawn[*key]
            })
        })
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    let mut title = text(&data["title"]);
    if title.is_empty() {
        for line in bytes.split(|byte| *byte == b'\n') {
            let Ok(event) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            let message = &event["payload"];
            if event["type"] == "response_item" && message["role"] == "user" {
                let prompt = render(first(message, &["content", "text"]));
                if human_prompt(&prompt) {
                    title = shortened(&prompt, 100);
                }
            }
        }
    }
    if title.is_empty() {
        title = if agent.is_empty() {
            format!("Codex session {}", &id[..8])
        } else {
            agent.clone()
        };
    }
    Some(Session {
        id,
        title: redact(&title),
        project: text(&data["cwd"]),
        path: path.to_string_lossy().into_owned(),
        source: "codex".into(),
        updated: modified(path).to_string(),
        parent_id: text(if spawn["parent_thread_id"].is_null() {
            &data["parent_thread_id"]
        } else {
            &spawn["parent_thread_id"]
        }),
        agent: redact(&agent),
    })
}

fn deepseek_catalog(project: &str, home: &Path, budget: &mut CatalogBudget) -> Vec<Session> {
    let root = home.join("sessions");
    let mut found = Vec::new();
    for project_dir in entries(&root, 128, budget) {
        if !budget.available() {
            break;
        }
        if !safe_directory(&project_dir, &root) {
            continue;
        }
        for entry in entries(&project_dir, CATALOG_HEADERS, budget) {
            if !budget.available() {
                break;
            }
            if let Some(session) = deepseek_descriptor(&entry, home, project, budget) {
                found.push(session);
            }
        }
    }
    found
}

fn deepseek_descriptor(
    entry: &Path,
    home: &Path,
    project: &str,
    budget: &mut CatalogBudget,
) -> Option<Session> {
    let root = home.join("sessions");
    let id = entry.file_name()?.to_str()?.to_owned();
    if !valid_id(&id, "deepseek") || !safe_directory(entry, &root) {
        return None;
    }
    let path = ["session.v4.jsonl", "session.jsonl", "session.v4.jsonl.zstd"]
        .iter()
        .map(|name| entry.join(name))
        .find(|path| safe_file(path, &root))?;
    let cache = home
        .join("storages/session_projcache/sessions")
        .join(format!("{id}.json"));
    let cached = if safe_file(&cache, home) {
        budget.prefix(&cache)
    } else {
        None
    };
    let cached_field = |path: &[&str]| {
        cached
            .as_ref()
            .and_then(|bytes| prefix_json_path(bytes, path))
            .unwrap_or(Value::Null)
    };
    let cached_cwd = text(&cached_field(&["record", "identity", "cwd"]));
    if !project.is_empty() && !cached_cwd.is_empty() && !project_matches(&cached_cwd, project) {
        return None;
    }
    let header = if path.extension().is_some_and(|s| s == "zstd") {
        compressed_header(&path, budget)?
    } else {
        first_json_line(&budget.prefix(&path)?)?
    };
    let cwd = text(&header["cwd"]);
    if header["type"] != "session"
        || header["version"] != 4
        || text(&header["id"]) != id
        || !project_matches(&cwd, project)
    {
        return None;
    }
    let title = text(&cached_field(&["record", "rows", "title", "val"]));
    let relation = cached_field(&["record", "rows", "subagent", "val"]);
    let mut parent_id = text(first(
        &header,
        &[
            "parentSessionId",
            "parentSessionID",
            "parentId",
            "parent_id",
        ],
    ));
    if parent_id.is_empty() {
        parent_id = text(first(
            &relation,
            &[
                "parentSessionId",
                "parentSessionID",
                "parentId",
                "parent_id",
            ],
        ));
    }
    if !valid_id(&parent_id, "deepseek") {
        parent_id.clear();
    }
    let mut agent = text(first(&header, &["agentPreset", "agentType"]));
    if agent.is_empty() {
        agent = text(&cached_field(&["record", "rows", "agentPreset", "val"]));
    }
    Some(Session {
        id,
        title: if title.is_empty() {
            "DeepSeek session".into()
        } else {
            redact(&title)
        },
        project: cwd,
        path: path.to_string_lossy().into_owned(),
        source: "deepseek".into(),
        updated: modified(&path).to_string(),
        parent_id,
        agent: redact(&agent),
    })
}

fn project_matches(recorded: &str, project: &str) -> bool {
    !recorded.is_empty()
        && Path::new(recorded).is_absolute()
        && (project.is_empty() || same_project(recorded, project))
}

fn zstd_executable() -> Option<String> {
    [
        "/usr/bin/zstd",
        "/opt/homebrew/bin/zstd",
        "/usr/local/bin/zstd",
    ]
    .into_iter()
    .find(|path| Path::new(path).is_file())
    .map(str::to_owned)
}

fn compressed_header(path: &Path, budget: &mut CatalogBudget) -> Option<Value> {
    if !budget.available() {
        return None;
    }
    let executable = zstd_executable()?;
    let maximum = HEADER_BYTES.min(CATALOG_BYTES.saturating_sub(budget.bytes));
    let timeout = budget
        .time_limit
        .saturating_sub(budget.start.elapsed())
        .min(Duration::from_millis(150));
    if timeout.is_zero() {
        return None;
    }
    budget.headers += 1;
    // Charge the complete possible prefix, including bounded decoder read-ahead,
    // even if the first row is shorter or decoding fails.
    budget.bytes += maximum;
    let id = path.parent()?.file_name()?.to_str()?;
    if !valid_id(id, "deepseek") {
        return None;
    }
    let expected = Session {
        id: id.into(),
        source: "deepseek".into(),
        ..Session::default()
    };
    // Stop at the complete first row. A structured retained header is independent
    // of the backend's user-facing diagnostic wording and later decoded output.
    let reader = decoded_window(
        &executable,
        path,
        &expected,
        Some(0),
        DecodeLimits {
            parse: ParseLimits {
                bytes: maximum + 2,
                events: 1,
                header_bytes: maximum,
            },
            decoded_bytes: maximum as u64,
            timeout,
            allow_global_header: true,
        },
    )
    .ok()?;
    first_json_line(&reader.header)
}

fn entries(root: &Path, limit: usize, budget: &CatalogBudget) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    for entry in entries.take(limit) {
        if !budget.available() {
            break;
        }
        if let Ok(entry) = entry {
            paths.push(entry.path());
        }
    }
    paths
}

const HISTORY_SORT_BATCH: usize = 4_096;

struct HistoryFrame {
    entries: Option<fs::ReadDir>,
    depth: usize,
    pending: Vec<PathBuf>,
    ready: VecDeque<PathBuf>,
    batch_limit: usize,
}

impl HistoryFrame {
    fn new(entries: fs::ReadDir, depth: usize) -> Self {
        Self {
            entries: Some(entries),
            depth,
            pending: Vec::new(),
            ready: VecDeque::new(),
            batch_limit: HISTORY_SORT_BATCH,
        }
    }

    fn exhausted(&self) -> bool {
        self.entries.is_none() && self.pending.is_empty() && self.ready.is_empty()
    }

    fn next(&mut self, budget: &CatalogBudget, remaining_entries: &mut usize) -> Option<PathBuf> {
        if let Some(path) = self.ready.pop_front() {
            return Some(path);
        }
        // Date directories and rollout filenames sort newest first. Inventory
        // remains resumable when either the page budget or batch cap is hit;
        // an unusually large directory never requires an unbounded sort.
        while budget.available() && *remaining_entries > 0 && self.pending.len() < self.batch_limit
        {
            let Some(entries) = self.entries.as_mut() else {
                break;
            };
            let Some(entry) = entries.next() else {
                self.entries = None;
                break;
            };
            *remaining_entries -= 1;
            if let Ok(entry) = entry {
                self.pending.push(entry.path());
            }
        }
        if self.entries.is_none() || self.pending.len() == self.batch_limit {
            self.pending
                .sort_by(|left, right| right.file_name().cmp(&left.file_name()));
            self.ready = std::mem::take(&mut self.pending).into();
        }
        self.ready.pop_front()
    }
}

struct HistoryWalker {
    source: String,
    root: PathBuf,
    stack: Vec<HistoryFrame>,
}

impl HistoryWalker {
    fn new(source: &str, root: PathBuf) -> Self {
        let stack = if safe_directory(&root, &root) {
            fs::read_dir(&root)
                .ok()
                .map(|entries| vec![HistoryFrame::new(entries, 0)])
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        Self {
            source: source.into(),
            root,
            stack,
        }
    }

    fn next(&mut self, budget: &CatalogBudget, remaining_entries: &mut usize) -> Option<PathBuf> {
        while budget.available() && *remaining_entries > 0 {
            let frame = self.stack.last_mut()?;
            let depth = frame.depth;
            let Some(path) = frame.next(budget, remaining_entries) else {
                if frame.exhausted() {
                    self.stack.pop();
                    continue;
                }
                // Partial directory inventory stays in the cursor for the next
                // page instead of emitting arbitrary filesystem-order rows.
                return None;
            };
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if self.source == "deepseek" {
                if depth == 1 && valid_id(name, "deepseek") && safe_directory(&path, &self.root) {
                    return Some(path);
                }
                if depth == 0 && safe_directory(&path, &self.root) {
                    if let Ok(entries) = fs::read_dir(&path) {
                        self.stack.push(HistoryFrame::new(entries, 1));
                    }
                }
            } else if depth == 3 {
                if name.starts_with("rollout-")
                    && name.ends_with(".jsonl")
                    && safe_file(&path, &self.root)
                {
                    return Some(path);
                }
            } else {
                let width = if depth == 0 { 4 } else { 2 };
                if name.len() == width
                    && name.bytes().all(|byte| byte.is_ascii_digit())
                    && safe_directory(&path, &self.root)
                {
                    if let Ok(entries) = fs::read_dir(&path) {
                        self.stack.push(HistoryFrame::new(entries, depth + 1));
                    }
                }
            }
        }
        None
    }
}

struct HistoryState {
    home: PathBuf,
    project: String,
    source: String,
    walkers: VecDeque<HistoryWalker>,
    pending: Option<(String, PathBuf)>,
    touched: Instant,
}

impl HistoryState {
    fn new(home: &Path, project: &str, source: &str) -> Self {
        let mut walkers = VecDeque::new();
        // DeepSeek gets the first candidate on the initial automatic page, then
        // providers alternate; a large Codex archive cannot hide DSH sessions.
        if source == "automatic" || source == "deepseek" {
            walkers.push_back(HistoryWalker::new("deepseek", home.join(".dsh/sessions")));
        }
        if source == "automatic" || source == "codex" {
            walkers.push_back(HistoryWalker::new("codex", home.join(".codex/sessions")));
        }
        Self {
            home: home.into(),
            project: project.into(),
            source: source.into(),
            walkers,
            pending: None,
            touched: Instant::now(),
        }
    }

    fn page(&mut self, limit: usize) -> HistoryPage {
        let mut budget = CatalogBudget::new();
        let mut remaining_entries = 8_192;
        let mut sessions = Vec::new();
        let mut seen = HashSet::new();
        while budget.available()
            && remaining_entries > 0
            && sessions.len() < limit.clamp(1, CATALOG_HEADERS)
        {
            if budget.headers + 2 > budget.header_limit {
                break;
            }
            let (source, path) = if let Some(pending) = self.pending.take() {
                pending
            } else {
                let Some(mut walker) = self.walkers.pop_front() else {
                    break;
                };
                let path = walker.next(&budget, &mut remaining_entries);
                let source = walker.source.clone();
                if !walker.stack.is_empty() {
                    self.walkers.push_back(walker);
                }
                let Some(path) = path else {
                    continue;
                };
                (source, path)
            };
            let session = if source == "codex" {
                codex_descriptor(&path, &self.project, &mut budget)
            } else {
                deepseek_descriptor(&path, &self.home.join(".dsh"), &self.project, &mut budget)
            };
            if let Some(session) = session {
                if seen.insert((session.source.clone(), session.id.clone())) {
                    sessions.push(session);
                }
            } else if !budget.available() {
                // A candidate fetched just before the deadline must not be
                // silently lost between pages. Retry it with a fresh budget.
                self.pending = Some((source, path));
                break;
            }
        }
        sessions.sort_by_key(|session| {
            std::cmp::Reverse(session.updated.parse::<u64>().unwrap_or_default())
        });
        self.touched = Instant::now();
        let complete = self.walkers.is_empty() && self.pending.is_none();
        HistoryPage {
            sessions,
            complete,
            next_cursor: None,
            warnings: if complete {
                Vec::new()
            } else {
                vec!["More retained agent history is available. Load the next page to continue the bounded catalog scan.".into()]
            },
        }
    }
}

/// Incremental retained history across all dates. Empty project explicitly
/// requests global history; every returned Session keeps its validated cwd.
/// Cursors are in-process resumable traversal handles (up to 8 active scans),
/// not durable bookmarks. Codex date directories and rollout names descend;
/// unusually large directories use resumable bounded sorted batches. Merge
/// pages by (source,id) and sort accumulated rows by updated.
pub fn discover_history(
    project: &str,
    source: &str,
    cursor: Option<&str>,
    limit: usize,
) -> HistoryPage {
    if !["automatic", "codex", "deepseek"].contains(&source) {
        return HistoryPage { complete: true, warnings: vec!["Historical local discovery supports Codex and DeepSeek. OpenCode exports remain explicit.".into()], ..HistoryPage::default() };
    }
    let Some(home) = std::env::var_os("HOME") else {
        return HistoryPage {
            complete: true,
            warnings: vec!["The home directory is unavailable.".into()],
            ..HistoryPage::default()
        };
    };
    static STATES: OnceLock<Mutex<HashMap<String, HistoryState>>> = OnceLock::new();
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let mut states = STATES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let token = if let Some(cursor) = cursor {
        let valid = states.get(cursor).is_some_and(|state| {
            state.project == project && state.source == source && state.home == Path::new(&home)
        });
        if !valid {
            return HistoryPage { complete: true, warnings: vec!["This history cursor expired or belongs to another scope. Start a fresh history scan.".into()], ..HistoryPage::default() };
        }
        cursor.to_owned()
    } else {
        if states.len() >= 8 {
            if let Some(oldest) = states
                .iter()
                .min_by_key(|(_, state)| state.touched)
                .map(|(token, _)| token.clone())
            {
                states.remove(&oldest);
            }
        }
        let token = format!("history-{}", NEXT.fetch_add(1, Ordering::Relaxed));
        states.insert(
            token.clone(),
            HistoryState::new(Path::new(&home), project, source),
        );
        token
    };
    let mut page = states
        .get_mut(&token)
        .expect("validated history state")
        .page(limit);
    if page.complete {
        states.remove(&token);
    } else {
        page.next_cursor = Some(token);
    }
    page
}

pub fn collect(session: &Session) -> Snapshot {
    collect_window(session, None)
}

/// Reads the preceding bounded trace window. Pass the previous snapshot's
/// window_start; the boundary is exclusive, so retained rows do not repeat.
pub fn collect_before(session: &Session, before: u64) -> Snapshot {
    collect_window(session, Some(before))
}

fn collect_window(session: &Session, before: Option<u64>) -> Snapshot {
    if session.source == "opencode" {
        if before.is_some() {
            return Snapshot {
                session: session.clone(),
                error: Some(
                    "OpenCode JSON exports do not support local byte-window paging.".into(),
                ),
                ..Snapshot::default()
            };
        }
        return open_code_collect(&session.project, &session.id);
    }
    let mut empty = Snapshot {
        session: session.clone(),
        ..Snapshot::default()
    };
    let Some(home) = std::env::var_os("HOME") else {
        empty.error = Some("The home directory is unavailable.".into());
        return empty;
    };
    let root = match session.source.as_str() {
        "codex" => PathBuf::from(&home).join(".codex/sessions"),
        "deepseek" => PathBuf::from(&home).join(".dsh/sessions"),
        _ => {
            empty.error = Some("Unsupported recorded session source.".into());
            return empty;
        }
    };
    let path = Path::new(&session.path);
    let filename = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let valid_filename = if session.source == "codex" {
        filename.starts_with("rollout-") && filename.ends_with(&format!("{}.jsonl", session.id))
    } else {
        ["session.v4.jsonl", "session.jsonl", "session.v4.jsonl.zstd"].contains(&filename)
            && path
                .parent()
                .and_then(Path::file_name)
                .and_then(|s| s.to_str())
                == Some(session.id.as_str())
    };
    if !valid_id(&session.id, &session.source) || !valid_filename || !safe_file(path, &root) {
        empty.error = Some("The session path is outside its recording directory, invalid, or contains a symbolic link.".into());
        return empty;
    }
    if filename.ends_with(".zstd") {
        // Decode only into a bounded in-memory window; large retained sessions
        // remain readable without spooling their complete decoded contents.
        let Some(decoder) = zstd_executable() else {
            empty.error = Some("This DeepSeek recording is compressed. An existing system or Homebrew zstd decoder is required; uncompressed sessions remain readable.".into());
            return empty;
        };
        return collect_compressed(&decoder, path, session, before, DecodeLimits::default());
    }
    match File::open(path) {
        Ok(file) => parse_jsonl_window(file, session, false, ParseLimits::default(), before),
        Err(error) => {
            empty.error = Some(format!(
                "The recording could not be read: {}",
                redact(&error.to_string())
            ));
            empty
        }
    }
}

#[derive(Clone, Copy)]
struct DecodeLimits {
    parse: ParseLimits,
    decoded_bytes: u64,
    timeout: Duration,
    allow_global_header: bool,
}

impl Default for DecodeLimits {
    fn default() -> Self {
        Self {
            parse: ParseLimits::default(),
            decoded_bytes: DECODED_BYTES,
            timeout: Duration::from_secs(8),
            allow_global_header: false,
        }
    }
}

struct DecodedWindow {
    header: Vec<u8>,
    header_complete: bool,
    tail: VecDeque<u8>,
    position: u64,
    before: Option<u64>,
    limits: DecodeLimits,
}

impl DecodedWindow {
    fn new(before: Option<u64>, limits: DecodeLimits) -> Result<Self, String> {
        if limits.parse.bytes <= limits.parse.header_bytes + 1 || limits.timeout.is_zero() {
            return Err("Invalid compressed recording window limits.".into());
        }
        if before.is_some_and(|offset| offset > limits.decoded_bytes) {
            return Err(
                "The requested compressed trace offset exceeds the 1 GiB decoded-work limit."
                    .into(),
            );
        }
        Ok(Self {
            header: Vec::new(),
            header_complete: false,
            tail: VecDeque::new(),
            position: 0,
            before,
            limits,
        })
    }

    fn feed(&mut self, mut bytes: &[u8], session: &Session) -> Result<bool, String> {
        if !self.header_complete {
            let count = bytes
                .iter()
                .position(|byte| *byte == b'\n')
                .map(|index| index + 1)
                .unwrap_or(bytes.len());
            if self.header.len().saturating_add(count) > self.limits.parse.header_bytes {
                return Err(
                    "The compressed recording header exceeds its bounded header limit.".into(),
                );
            }
            self.header.extend_from_slice(&bytes[..count]);
            self.position += count as u64;
            if self.position > self.limits.decoded_bytes {
                return Err(
                    "The compressed recording exceeds the 1 GiB decoded-work limit.".into(),
                );
            }
            bytes = &bytes[count..];
            if self.header.last() != Some(&b'\n') {
                if self.header.len() == self.limits.parse.header_bytes {
                    return Err(
                        "The compressed recording header exceeds its bounded header limit.".into(),
                    );
                }
                return Ok(false);
            }
            let header = first_json_line(&self.header)
                .ok_or("The compressed recording has no valid JSON session header.")?;
            if header["type"] != "session"
                || header["version"] != 4
                || text(&header["id"]) != session.id
                || !(same_project(&text(&header["cwd"]), &session.project)
                    || (self.limits.allow_global_header
                        && session.project.is_empty()
                        && project_matches(&text(&header["cwd"]), "")))
            {
                return Err("The compressed recording belongs to another workspace/session or has an unsupported header.".into());
            }
            self.header_complete = true;
        }
        if self.before.is_some_and(|before| self.position >= before) {
            return Ok(true);
        }
        let end = self
            .before
            .unwrap_or(self.limits.decoded_bytes)
            .min(self.limits.decoded_bytes);
        let received = bytes.len();
        let retained = received.min(end.saturating_sub(self.position) as usize);
        let capacity = self.limits.parse.bytes - self.limits.parse.header_bytes;
        let bytes = &bytes[..retained];
        if bytes.len() >= capacity {
            self.tail.clear();
            self.tail.extend(&bytes[bytes.len() - capacity..]);
        } else {
            let discard = self
                .tail
                .len()
                .saturating_add(bytes.len())
                .saturating_sub(capacity);
            self.tail.drain(..discard);
            self.tail.extend(bytes);
        }
        self.position += retained as u64;
        if self.before.is_some_and(|before| self.position >= before) {
            return Ok(true);
        }
        if retained < received || self.position >= self.limits.decoded_bytes {
            return Err("The compressed recording exceeds the 1 GiB decoded-work limit.".into());
        }
        Ok(false)
    }

    fn into_reader(self) -> Result<WindowReader, String> {
        if !self.header_complete {
            return Err("The compressed recording has no complete session header.".into());
        }
        let tail: Vec<u8> = self.tail.into();
        Ok(WindowReader {
            start: self.position - tail.len() as u64,
            length: self.position,
            header: self.header,
            tail,
            position: 0,
        })
    }
}

// A sparse, seekable view of only the original header and retained decoded tail.
// Logical offsets remain identical to the uncompressed recording; the omitted
// gap occupies no memory and cannot accidentally be read as fabricated bytes.
struct WindowReader {
    header: Vec<u8>,
    tail: Vec<u8>,
    start: u64,
    length: u64,
    position: u64,
}

impl Read for WindowReader {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if output.is_empty() || self.position >= self.length {
            return Ok(0);
        }
        let available = if self.position < self.header.len() as u64 {
            &self.header[self.position as usize..]
        } else if self.position >= self.start {
            &self.tail[(self.position - self.start) as usize..]
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Read outside the retained decoded window",
            ));
        };
        let count = available.len().min(output.len());
        output[..count].copy_from_slice(&available[..count]);
        self.position += count as u64;
        Ok(count)
    }
}

impl Seek for WindowReader {
    fn seek(&mut self, offset: SeekFrom) -> std::io::Result<u64> {
        let position = match offset {
            SeekFrom::Start(position) => position as i128,
            SeekFrom::Current(offset) => self.position as i128 + offset as i128,
            SeekFrom::End(offset) => self.length as i128 + offset as i128,
        };
        if !(0..=u64::MAX as i128).contains(&position) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Invalid decoded trace offset",
            ));
        }
        self.position = position as u64;
        Ok(self.position)
    }
}

struct DecoderSlot;
static ACTIVE_DECODERS: AtomicUsize = AtomicUsize::new(0);
impl DecoderSlot {
    fn acquire() -> Result<Self, String> {
        ACTIVE_DECODERS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < 2).then_some(active + 1)
            })
            .map(|_| Self)
            .map_err(|_| {
                "Two compressed recordings are already decoding or awaiting cleanup.".into()
            })
    }
}
impl Drop for DecoderSlot {
    fn drop(&mut self) {
        ACTIVE_DECODERS.fetch_sub(1, Ordering::AcqRel);
    }
}

struct DecoderProcess {
    child: Option<Child>,
    slot: Option<DecoderSlot>,
}

struct DecoderPipes {
    process: DecoderProcess,
    stdout: ChildStdout,
    stderr: ChildStderr,
}

impl Drop for DecoderProcess {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // WNOWAIT preserves PID ownership until after group cleanup.
            if decoder_exited(child.id()).is_ok() {
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
            }
            match child.try_wait() {
                Ok(Some(_)) | Err(_) => {}
                Ok(None) => {
                    let slot = self.slot.take();
                    std::thread::spawn(move || {
                        let _slot = slot;
                        loop {
                            match child.try_wait() {
                                Ok(Some(_)) | Err(_) => break,
                                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                            }
                        }
                    });
                }
            }
        }
    }
}

fn decoder_exited(pid: u32) -> Result<bool, String> {
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result == 0 {
        return Ok(unsafe { info.si_pid() } == pid as i32);
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::Interrupted {
        Ok(false)
    } else {
        Err(format!("Could not observe decoder exit: {error}"))
    }
}

fn decoder_nonblocking(fd: RawFd) -> Result<(), String> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        Err(format!(
            "Could not prepare decoder output: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

fn decoder_drain<R: Read>(
    pipe: &mut R,
    closed: &mut bool,
    mut maximum: usize,
    mut accept: impl FnMut(&[u8]) -> Result<bool, String>,
) -> Result<(), String> {
    let mut buffer = [0; 8192];
    for _ in 0..16 {
        if *closed || maximum == 0 {
            break;
        }
        let capacity = buffer.len().min(maximum);
        match pipe.read(&mut buffer[..capacity]) {
            Ok(0) => {
                *closed = true;
                break;
            }
            Ok(count) => {
                maximum -= count;
                if accept(&buffer[..count])? {
                    break;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(format!("Could not read decoder output: {error}")),
        }
    }
    Ok(())
}

fn decoded_window(
    decoder: &str,
    path: &Path,
    session: &Session,
    before: Option<u64>,
    limits: DecodeLimits,
) -> Result<WindowReader, String> {
    let window = DecodedWindow::new(before, limits)?;
    let deadline = Instant::now()
        .checked_add(limits.timeout)
        .ok_or("Invalid decoder timeout.")?;
    drive_decoder(
        spawn_decoder(decoder, path)?,
        window,
        session,
        deadline,
        Vec::new(),
    )
}

fn spawn_decoder(decoder: &str, path: &Path) -> Result<DecoderPipes, String> {
    let slot = DecoderSlot::acquire()?;
    let child = Command::new(decoder)
        .args(["--decompress", "--stdout", "--quiet", "--memory=16MB", "--"])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("LC_ALL", "C")
        .process_group(0)
        .spawn()
        .map_err(|error| format!("Could not launch the installed decoder: {error}"))?;
    let mut process = DecoderProcess {
        child: Some(child),
        slot: Some(slot),
    };
    let child = process.child.as_mut().expect("owned decoder child");
    let stdout = child.stdout.take().ok_or("Decoder stdout unavailable.")?;
    let stderr = child.stderr.take().ok_or("Decoder stderr unavailable.")?;
    decoder_nonblocking(stdout.as_raw_fd())
        .and_then(|_| decoder_nonblocking(stderr.as_raw_fd()))?;
    Ok(DecoderPipes {
        process,
        stdout,
        stderr,
    })
}

fn drive_decoder(
    pipes: DecoderPipes,
    mut window: DecodedWindow,
    session: &Session,
    deadline: Instant,
    mut diagnostic: Vec<u8>,
) -> Result<WindowReader, String> {
    let DecoderPipes {
        mut process,
        mut stdout,
        mut stderr,
    } = pipes;
    let pid = process.child.as_ref().expect("owned decoder child").id();
    let limits = window.limits;
    let mut failure = None;
    diagnostic.truncate(8192);
    let mut closed_out = false;
    let mut closed_err = false;
    let mut reached_boundary = false;
    let mut term_at = None;
    let mut kill_at = None;
    loop {
        let exited = match decoder_exited(pid) {
            Ok(exited) => exited,
            Err(error) => {
                failure.get_or_insert(error);
                break;
            }
        };
        {
            if failure.is_none() && !reached_boundary {
                let maximum = limits.decoded_bytes.saturating_sub(window.position) as usize;
                if let Err(error) = decoder_drain(&mut stdout, &mut closed_out, maximum, |bytes| {
                    reached_boundary = window.feed(bytes, session)?;
                    Ok(reached_boundary)
                }) {
                    failure.get_or_insert(error);
                }
            }
            // Stop consuming stdout at a work cap or requested boundary, so
            // pipe backpressure bounds extra decoder work during group cleanup.
            if failure.is_some() || reached_boundary {
                closed_out = true;
            }
            if let Err(error) = decoder_drain(&mut stderr, &mut closed_err, usize::MAX, |bytes| {
                diagnostic.extend_from_slice(
                    &bytes[..bytes.len().min(8192usize.saturating_sub(diagnostic.len()))],
                );
                Ok(false)
            }) {
                failure.get_or_insert(error);
            }
        }
        // Observe completed output before comparing the deadline. A parent
        // descheduled after the child finished must still accept its complete,
        // bounded result; header/work-cap failures remain failures.
        if term_at.is_none() && failure.is_none() && closed_out && closed_err {
            let completed = if exited {
                Ok(true)
            } else {
                decoder_exited(pid)
            };
            match completed {
                Ok(true) => break,
                Ok(false) => {}
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
        let now = Instant::now();
        if term_at.is_none() && (failure.is_some() || reached_boundary || now >= deadline) {
            if failure.is_none() && !reached_boundary {
                failure = Some("Compressed recording decoding timed out after 8 seconds.".into());
            }
            unsafe {
                libc::kill(-(pid as i32), libc::SIGTERM);
            }
            term_at = Some(now);
        }
        if term_at.is_some_and(|at| now.duration_since(at) >= Duration::from_millis(150))
            && kill_at.is_none()
        {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
            kill_at = Some(now);
        }
        if kill_at.is_some_and(|at| exited || now.duration_since(at) >= Duration::from_millis(150))
        {
            break;
        }
        let mut descriptors = [
            libc::pollfd {
                fd: if closed_out { -1 } else { stdout.as_raw_fd() },
                events: libc::POLLIN | libc::POLLHUP,
                revents: 0,
            },
            libc::pollfd {
                fd: if closed_err { -1 } else { stderr.as_raw_fd() },
                events: libc::POLLIN | libc::POLLHUP,
                revents: 0,
            },
        ];
        let result = unsafe {
            libc::poll(
                descriptors.as_mut_ptr(),
                descriptors.len() as libc::nfds_t,
                10,
            )
        };
        if result == -1 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
        {
            failure.get_or_insert_with(|| {
                format!(
                    "Could not wait for decoder output: {}",
                    std::io::Error::last_os_error()
                )
            });
        }
    }
    let status = match process
        .child
        .as_mut()
        .expect("owned decoder child")
        .try_wait()
    {
        Ok(Some(status)) => {
            process.child.take();
            Some(status)
        }
        Ok(None) => {
            failure.get_or_insert("Decoder did not exit within its cleanup deadline.".into());
            None
        }
        Err(error) => {
            failure.get_or_insert_with(|| format!("Could not reap decoder: {error}"));
            None
        }
    };
    if failure.is_none() && !reached_boundary && !status.is_some_and(|status| status.success()) {
        failure = Some("The installed decoder could not decode this recording within its 16 MiB codec-memory bound.".into());
    }
    if let Some(error) = failure {
        let diagnostic = redact(&String::from_utf8_lossy(&diagnostic));
        return Err(if diagnostic.is_empty() {
            error
        } else {
            format!("{error}\n{diagnostic}")
        });
    }
    window.into_reader()
}

fn collect_compressed(
    decoder: &str,
    path: &Path,
    session: &Session,
    before: Option<u64>,
    limits: DecodeLimits,
) -> Snapshot {
    match decoded_window(decoder, path, session, before, limits) {
        Ok(reader) => parse_jsonl_window(reader, session, false, limits.parse, before),
        Err(error) => Snapshot {
            session: session.clone(),
            error: Some(redact(&error)),
            ..Snapshot::default()
        },
    }
}

struct Parser {
    snapshot: Snapshot,
    tools: HashMap<String, usize>,
    tool_details: HashMap<String, Value>,
    response_calls: HashMap<String, String>,
    thread_usage: Option<Usage>,
    count_usage: Option<Usage>,
    usages: HashMap<String, Usage>,
    header: bool,
    human_title: bool,
    unclassified_prompt: Option<String>,
    malformed: usize,
}

impl Parser {
    fn new(session: &Session) -> Self {
        Self {
            snapshot: Snapshot {
                session: session.clone(),
                ..Snapshot::default()
            },
            tools: HashMap::new(),
            tool_details: HashMap::new(),
            response_calls: HashMap::new(),
            thread_usage: None,
            count_usage: None,
            usages: HashMap::new(),
            header: false,
            human_title: false,
            unclassified_prompt: None,
            malformed: 0,
        }
    }

    fn record(
        &mut self,
        id: &str,
        time: &str,
        kind: &str,
        summary: &str,
        raw: &Value,
        infrastructure: bool,
    ) {
        self.snapshot.records.push(Record {
            id: redact(id),
            time: redact(time),
            kind: kind.into(),
            summary: shortened(&redact(summary), 240),
            detail: pretty(raw),
            tool_status: String::new(),
            is_infrastructure: infrastructure,
        });
    }

    fn prompt(&mut self, prompt: &str) {
        if human_prompt(prompt) {
            self.snapshot.session.title = shortened(&redact(prompt), 100);
            self.human_title = true;
        }
    }

    fn warning(&mut self, warning: &str) {
        let warning = redact(warning);
        if self.snapshot.warnings.len() < 32 && !self.snapshot.warnings.contains(&warning) {
            self.snapshot.warnings.push(warning);
        }
    }

    fn child(&mut self, id: &str, input: &Value, status: &str) {
        if !valid_id(id, &self.snapshot.session.source) || id == self.snapshot.session.id {
            return;
        }
        let title = text(first(
            input,
            &["task_name", "title", "description", "name", "agent_path"],
        ));
        let agent = text(first(
            input,
            &[
                "agent_type",
                "subagent_type",
                "agentType",
                "agentPreset",
                "agent",
                "model",
            ],
        ));
        if let Some(child) = self
            .snapshot
            .children
            .iter_mut()
            .find(|child| child.session_id == id)
        {
            if !title.is_empty() {
                child.title = redact(&title);
            }
            if !agent.is_empty() {
                child.agent = redact(&agent);
            }
            child.status = redact(status);
        } else {
            self.snapshot.children.push(ChildAgent {
                id: id.into(),
                session_id: id.into(),
                title: if title.is_empty() {
                    "Recorded subagent".into()
                } else {
                    redact(&title)
                },
                agent: redact(&agent),
                status: redact(status),
            });
        }
    }

    fn tool(
        &mut self,
        id: &str,
        name: &str,
        time: &str,
        raw: &Value,
        result: bool,
        input: &Value,
        output: &Value,
    ) {
        if id.is_empty() {
            self.warning("A recorded tool event has no call ID and cannot be paired.");
            return;
        }
        let index = if let Some(index) = self.tools.get(id) {
            *index
        } else {
            let index = self.snapshot.records.len();
            self.tools.insert(id.into(), index);
            self.record(id, time, "tool", "Tool event", &json!({}), false);
            index
        };
        let details = self
            .tool_details
            .entry(id.into())
            .or_insert_with(|| json!({}));
        let key = if result { "result" } else { "call" };
        if !details[key].is_null() {
            self.warning(&format!(
                "A duplicate tool {} was recorded for call ID {id}; the first event was retained.",
                if result { "result" } else { "call" }
            ));
            return;
        }
        details[key] = raw.clone();
        if !name.is_empty() {
            details["name"] = Value::String(name.into());
        }
        if !result {
            details["input"] = decoded(input);
        }
        if result {
            details["output"] = decoded(output);
        }
        let tool_name = text(&details["name"]);
        let failed = failure(&details["output"], 0);
        let status = if details["result"].is_null() {
            "Result not recorded"
        } else if failed {
            "Error"
        } else if details["call"].is_null() {
            "Call not recorded"
        } else {
            "Completed"
        };
        let rendered = if result {
            render(output)
        } else {
            render(input)
        };
        let summary = format!(
            "{} · {status}{}",
            if tool_name.is_empty() {
                "Unknown tool"
            } else {
                &tool_name
            },
            if rendered.is_empty() {
                String::new()
            } else {
                format!(" · {}", shortened(&rendered, 160))
            }
        );
        let detail = pretty(details);
        let child = if subagent_tool(&tool_name) && !details["result"].is_null() {
            let linked = linked_session(&details["output"], &self.snapshot.session.source, 0)
                .or_else(|| {
                    if self.snapshot.session.source == "deepseek" {
                        render(&details["output"])
                            .strip_prefix("started subagent ")
                            .filter(|id| valid_id(id.trim(), "deepseek"))
                            .map(|id| id.trim().to_owned())
                    } else {
                        None
                    }
                });
            linked.map(|id| (id, details["input"].clone(), failed))
        } else {
            None
        };
        let record = &mut self.snapshot.records[index];
        record.summary = shortened(&redact(&summary), 240);
        record.detail = detail;
        record.tool_status = status.into();
        if result || record.time.is_empty() {
            record.time = redact(time);
        }
        if let Some((id, input, failed)) = child {
            self.child(
                &id,
                &input,
                if failed {
                    "Spawn error"
                } else {
                    "Spawn recorded"
                },
            );
        }
    }

    fn codex(&mut self, event: &Value, sequence: usize) {
        let kind = text(&event["type"]);
        let data = &event["payload"];
        let subkind = text(&data["type"]);
        let time = text(&event["timestamp"]);
        let id = format!("codex-{sequence}");
        if sequence == 0 {
            let recorded_id = text(first(data, &["id", "session_id"]));
            if kind != "session_meta"
                || recorded_id != self.snapshot.session.id
                || internal_codex(data)
                || !same_project(&text(&data["cwd"]), &self.snapshot.session.project)
            {
                self.snapshot.error = Some(
                    "The recording has no supported matching Codex workspace/session header."
                        .into(),
                );
                return;
            }
            self.header = true;
            self.snapshot.model = redact(&text(&data["model"]));
            let spawn = &data["source"]["subagent"]["thread_spawn"];
            let parent = text(if spawn["parent_thread_id"].is_null() {
                &data["parent_thread_id"]
            } else {
                &spawn["parent_thread_id"]
            });
            if !parent.is_empty() {
                self.snapshot.session.parent_id = parent;
            }
            self.record(&id, &time, "runtime", "Codex session identity", event, true);
            return;
        }
        match kind.as_str() {
            "turn_context" => {
                if let Some(model) = data["model"].as_str() {
                    self.snapshot.model = redact(model);
                }
                self.record(
                    &id,
                    &time,
                    "context",
                    &format!("Turn context · {}", self.snapshot.model),
                    event,
                    true,
                );
            }
            "response_item" => match subkind.as_str() {
                "reasoning" => {} // Private/encrypted reasoning is never exposed.
                "message" | "agent_message" => {
                    let role = text(&data["role"]);
                    let content = render(first(data, &["content", "text"]));
                    let infrastructure = role == "system"
                        || role == "developer"
                        || (role == "user" && !human_prompt(&content));
                    if role == "user" && !infrastructure {
                        self.prompt(&content);
                    }
                    self.record(
                        &id,
                        &time,
                        if infrastructure {
                            "context"
                        } else if role.is_empty() {
                            "assistant"
                        } else {
                            &role
                        },
                        &content,
                        event,
                        infrastructure,
                    );
                }
                "function_call" | "custom_tool_call" => {
                    let call_id = text(&data["call_id"]);
                    let namespace = text(&data["namespace"]);
                    let name = text(&data["name"]);
                    let name = if namespace.is_empty() {
                        name
                    } else {
                        format!("{namespace}.{name}")
                    };
                    if let Some(response) = data["id"].as_str() {
                        self.response_calls.insert(response.into(), call_id.clone());
                    }
                    self.tool(
                        &call_id,
                        &name,
                        &time,
                        event,
                        false,
                        first(data, &["arguments", "input"]),
                        &Value::Null,
                    );
                }
                "function_call_output" | "custom_tool_call_output" => {
                    self.tool(
                        &text(&data["call_id"]),
                        "",
                        &time,
                        event,
                        true,
                        &Value::Null,
                        &data["output"],
                    );
                }
                _ => self.record(
                    &id,
                    &time,
                    "runtime",
                    &format!("Recorded response item · {subkind}"),
                    event,
                    true,
                ),
            },
            "token_usage_record" => {
                let thread = text(&data["thread_id"]);
                if thread.is_empty() || thread == self.snapshot.session.id {
                    if let Some(usage) = usage(&data["thread_token_usage"]) {
                        self.thread_usage = Some(usage);
                    }
                    if let (Some(response), Some(usage)) =
                        (data["response_id"].as_str(), usage(&data["usage"]))
                    {
                        self.usages.insert(response.into(), usage);
                    }
                }
                self.record(&id, &time, "runtime", "Recorded token usage", event, true);
            }
            "event_msg" => match subkind.as_str() {
                "token_count" => {
                    if let Some(usage) = usage(&data["info"]["total_token_usage"]) {
                        self.count_usage = Some(usage);
                    }
                    self.record(
                        &id,
                        &time,
                        "runtime",
                        "Recorded cumulative token usage",
                        event,
                        true,
                    );
                }
                "user_message" | "agent_message" => {
                    let content = render(first(data, &["message", "text"]));
                    let human = subkind == "user_message" && human_prompt(&content);
                    if human {
                        self.prompt(&content);
                    }
                    self.record(
                        &id,
                        &time,
                        if subkind == "agent_message" {
                            "assistant"
                        } else if human {
                            "user"
                        } else {
                            "context"
                        },
                        &content,
                        event,
                        subkind == "user_message" && !human,
                    );
                }
                "item_completed" => {
                    let item = &data["item"];
                    let item_kind = text(&item["type"]);
                    if item_kind == "SubAgentActivity" {
                        let child = text(&item["agent_thread_id"]);
                        let activity = text(&item["kind"]);
                        if activity == "started"
                            || self.snapshot.children.iter().any(|known| known.id == child)
                        {
                            self.child(&child, item, &format!("{activity} recorded"));
                        }
                        self.record(
                            &id,
                            &time,
                            "runtime",
                            &format!("Subagent {activity}"),
                            event,
                            true,
                        );
                    } else if ["CommandExecution", "McpToolCall", "FileChange"]
                        .contains(&item_kind.as_str())
                    {
                        let item_id = text(&item["id"]);
                        let call_id =
                            self.response_calls
                                .get(&item_id)
                                .cloned()
                                .unwrap_or_else(|| {
                                    if item_id.is_empty() {
                                        id.clone()
                                    } else {
                                        item_id
                                    }
                                });
                        if let Some(index) = self.tools.get(&call_id).copied() {
                            if let Some(details) = self.tool_details.get_mut(&call_id) {
                                details["completed_item"] = event.clone();
                                self.snapshot.records[index].detail = pretty(details);
                            }
                        } else {
                            self.record(
                                &call_id,
                                &time,
                                "tool",
                                &format!(
                                    "{item_kind} · {}",
                                    render(first(item, &["command", "tool", "status"]))
                                ),
                                event,
                                false,
                            );
                        }
                    } else if !["UserMessage", "AgentMessage", "Reasoning"]
                        .contains(&item_kind.as_str())
                    {
                        self.record(
                            &id,
                            &time,
                            "runtime",
                            &format!("Completed item · {item_kind}"),
                            event,
                            true,
                        );
                    }
                }
                _ => self.record(
                    &id,
                    &time,
                    "runtime",
                    &format!("Codex {}", subkind.replace('_', " ")),
                    event,
                    true,
                ),
            },
            "world_state" => {}
            "session_meta" => {}
            _ => self.record(
                &id,
                &time,
                "context",
                &format!("Recorded {}", kind.replace('_', " ")),
                event,
                true,
            ),
        }
    }

    fn deepseek(&mut self, event: &Value, sequence: usize) {
        let kind = text(&event["type"]);
        let data = &event["data"];
        let time = text(first(event, &["time", "createdAt"]));
        let id = format!("deepseek-{sequence}");
        if sequence == 0 {
            if kind != "session"
                || event["version"] != 4
                || text(&event["id"]) != self.snapshot.session.id
                || !same_project(&text(&event["cwd"]), &self.snapshot.session.project)
            {
                self.snapshot.error = Some(
                    "The recording has no supported matching DeepSeek v4 workspace/session header."
                        .into(),
                );
                return;
            }
            self.header = true;
            let parent = text(first(
                event,
                &[
                    "parentSessionId",
                    "parentSessionID",
                    "parentId",
                    "parent_id",
                ],
            ));
            if valid_id(&parent, "deepseek") {
                self.snapshot.session.parent_id = parent;
            }
            self.snapshot.session.agent = redact(&text(&event["agentPreset"]));
            self.record(
                &id,
                &time,
                "runtime",
                "DeepSeek v4 session identity",
                event,
                true,
            );
            return;
        }
        match kind.as_str() {
            "session/title" => {
                if !self.human_title {
                    self.snapshot.session.title = redact(&text(&data["title"]));
                }
                self.record(&id, &time, "runtime", "Recorded session title", event, true);
            }
            "model/selection" | "request/context" | "request/header" => {
                let model = if kind == "request/header" {
                    text(&data["header"]["config"]["model"])
                } else {
                    text(&data["model"])
                };
                if !model.is_empty() {
                    self.snapshot.model = redact(&model);
                }
                self.record(
                    &id,
                    &time,
                    "context",
                    &format!("{kind} · {}", self.snapshot.model),
                    event,
                    true,
                );
            }
            "agent-preset/selected" => {
                self.snapshot.session.agent = redact(&text(&data["agentPreset"]));
                self.record(
                    &id,
                    &time,
                    "context",
                    &self.snapshot.session.agent.clone(),
                    event,
                    true,
                );
            }
            "system/message" | "developer/message" | "user/message" | "assistant/message" => {
                let message = if data["message"].is_object() {
                    &data["message"]
                } else {
                    data
                };
                let role = text(&message["role"]);
                let role = if role.is_empty() {
                    kind.split('/').next().unwrap_or("unknown")
                } else {
                    &role
                };
                let content = render(first(message, &["content", "text"]));
                let source = text(&message["source"]["kind"]);
                let human = role == "user" && source == "user" && human_prompt(&content);
                if human {
                    self.prompt(&content);
                }
                if role == "user"
                    && source.is_empty()
                    && self.unclassified_prompt.is_none()
                    && human_prompt(&content)
                {
                    self.unclassified_prompt = Some(content.clone());
                }
                let infrastructure = ["system", "developer"].contains(&role)
                    || (role == "user" && !human && !source.is_empty());
                self.record(
                    &id,
                    &time,
                    if infrastructure { "context" } else { role },
                    &content,
                    event,
                    infrastructure,
                );
                if role == "assistant" {
                    let message_id = text(&message["id"]);
                    if let Some(usage) = usage(&data["usage"]) {
                        self.usages.insert(
                            if message_id.is_empty() {
                                id.clone()
                            } else {
                                message_id
                            },
                            usage,
                        );
                    }
                    let model = text(&message["source"]["model"]);
                    if !model.is_empty() {
                        self.snapshot.model = redact(&model);
                    }
                }
            }
            "tool/call" => self.tool(
                &text(first(data, &["callId", "id"])),
                &text(&data["name"]),
                &time,
                event,
                false,
                first(data, &["arguments", "input"]),
                &Value::Null,
            ),
            "tool/result" => {
                let message = if data["message"].is_object() {
                    &data["message"]
                } else {
                    data
                };
                let mut call_id = text(first(message, &["toolCallId", "callId"]));
                if call_id.is_empty() {
                    call_id = text(&message["source"]["callId"]);
                }
                if call_id.is_empty() {
                    call_id = text(&data["callId"]);
                }
                // Keep isError and structured child fields in the output value.
                self.tool(
                    &call_id,
                    &text(&data["name"]),
                    &time,
                    event,
                    true,
                    &Value::Null,
                    message,
                );
            }
            _ => {
                if kind.starts_with("subagent/") {
                    if let Some(child) = linked_session(data, "deepseek", 0) {
                        let status = text(first(data, &["status", "stopReason"]));
                        self.child(
                            &child,
                            data,
                            if status.is_empty() {
                                "Recorded child"
                            } else {
                                &status
                            },
                        );
                    }
                }
                self.record(&id, &time, "runtime", &kind, event, true);
            }
        }
    }

    fn finish(mut self) -> Snapshot {
        if !self.header && self.snapshot.error.is_none() {
            self.snapshot.error =
                Some("The recording contains no supported session header.".into());
        }
        if !self.human_title {
            if let Some(prompt) = self.unclassified_prompt.take() {
                self.prompt(&prompt);
            }
        }
        self.snapshot.usage = self
            .thread_usage
            .take()
            .or(self.count_usage.take())
            .or_else(|| sum_usage(self.usages.values()));
        if self.snapshot.usage.is_none() {
            self.warning("No token usage was recorded for this session.");
        }
        if self.snapshot.model.is_empty() {
            self.warning("No model was recorded for this session.");
        }
        if self.malformed > 0 {
            self.warning(&format!(
                "{} malformed or incomplete JSONL rows were omitted.",
                self.malformed
            ));
        }
        if self.tool_details.values().any(|v| v["call"].is_null()) {
            self.warning("Some tool results have no corresponding recorded call.");
        }
        self.warning("Only retained recording events are shown; they do not prove the complete effective provider request was recorded.");
        self.snapshot
    }
}

#[derive(Clone, Copy)]
struct ParseLimits {
    bytes: usize,
    events: usize,
    header_bytes: usize,
}

impl Default for ParseLimits {
    fn default() -> Self {
        Self {
            bytes: SESSION_BYTES,
            events: SESSION_EVENTS,
            header_bytes: HEADER_BYTES,
        }
    }
}

#[cfg(test)]
fn parse_jsonl<R: Read + Seek>(reader: R, session: &Session, truncated: bool) -> Snapshot {
    parse_jsonl_with_limits(reader, session, truncated, ParseLimits::default())
}

#[cfg(test)]
fn parse_jsonl_with_limits<R: Read + Seek>(
    reader: R,
    session: &Session,
    truncated: bool,
    limits: ParseLimits,
) -> Snapshot {
    parse_jsonl_window(reader, session, truncated, limits, None)
}

fn parse_jsonl_window<R: Read + Seek>(
    mut reader: R,
    session: &Session,
    truncated: bool,
    limits: ParseLimits,
    before: Option<u64>,
) -> Snapshot {
    let mut parser = Parser::new(session);
    let total_bytes = match reader
        .seek(SeekFrom::End(0))
        .and_then(|length| reader.seek(SeekFrom::Start(0)).map(|_| length))
    {
        Ok(length) => length,
        Err(error) => {
            parser.snapshot.error = Some(format!(
                "Recording seek failed: {}",
                redact(&error.to_string())
            ));
            return parser.finish();
        }
    };
    let header_limit = limits.header_bytes.min(limits.bytes);
    let mut header = Vec::new();
    let header_read =
        BufReader::new((&mut reader).take(header_limit as u64)).read_until(b'\n', &mut header);
    if let Err(error) = header_read {
        parser.snapshot.error = Some(format!(
            "Session header read failed: {}",
            redact(&error.to_string())
        ));
        return parser.finish();
    }
    if header.len() == header_limit
        && header.last() != Some(&b'\n')
        && total_bytes > header.len() as u64
    {
        parser.snapshot.error =
            Some("The recording header exceeds its bounded header limit.".into());
        return parser.finish();
    }
    match serde_json::from_slice::<Value>(&header) {
        Ok(event) => {
            if session.source == "codex" {
                parser.codex(&event, 0);
            } else {
                parser.deepseek(&event, 0);
            }
        }
        Err(_) => {
            parser.snapshot.error =
                Some("The first recording row is not a valid session header.".into())
        }
    }
    if parser.snapshot.error.is_some() {
        return parser.finish();
    }

    // Reserve the whole bounded header read (including BufReader read-ahead)
    // and one boundary probe. Aggregate disk reads remain within the byte cap.
    // The newest byte window advances on every refresh as the recording grows.
    let body_budget = limits.bytes.saturating_sub(header_limit.saturating_add(1));
    let window_end = before
        .unwrap_or(total_bytes)
        .min(total_bytes)
        .max(header.len() as u64);
    let tail_start = window_end
        .saturating_sub(body_budget as u64)
        .max(header.len() as u64);
    let mut omitted = tail_start > header.len() as u64;
    let boundary = if omitted {
        let mut byte = [0];
        reader
            .seek(SeekFrom::Start(tail_start - 1))
            .and_then(|_| reader.read_exact(&mut byte))
            .map(|_| byte[0] == b'\n')
    } else {
        Ok(true)
    };
    let begins_on_boundary = match boundary {
        Ok(boundary) => boundary,
        Err(error) => {
            parser.snapshot.error = Some(format!(
                "Recording boundary read failed: {}",
                redact(&error.to_string())
            ));
            return parser.finish();
        }
    };
    if let Err(error) = reader.seek(SeekFrom::Start(tail_start)) {
        parser.snapshot.error = Some(format!(
            "Recording tail seek failed: {}",
            redact(&error.to_string())
        ));
        return parser.finish();
    }
    let mut reader = BufReader::new(reader.take(window_end.saturating_sub(tail_start)));
    let mut position = tail_start;
    if !begins_on_boundary {
        let mut partial = Vec::new();
        match reader.read_until(b'\n', &mut partial) {
            Ok(count) => position += count as u64,
            Err(error) => {
                parser.snapshot.error = Some(format!(
                    "Recording boundary read failed: {}",
                    redact(&error.to_string())
                ));
                return parser.finish();
            }
        }
    }
    let capacity = limits.events.saturating_sub(1);
    let mut latest = VecDeque::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        // The underlying Take also bounds a malicious single row with no newline.
        let count = match reader.read_until(b'\n', &mut line) {
            Ok(count) => count,
            Err(error) => {
                parser.snapshot.error = Some(format!(
                    "Recording read failed: {}",
                    redact(&error.to_string())
                ));
                break;
            }
        };
        if count == 0 {
            break;
        }
        let offset = position;
        position += count as u64;
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        if capacity == 0 {
            omitted = true;
            continue;
        }
        if latest.len() == capacity {
            latest.pop_front();
            omitted = true;
        }
        latest.push_back((offset, std::mem::take(&mut line)));
    }
    // Release the compressed backing window before materializing record JSON.
    // The selected raw-row queue is now the only retained input buffer.
    drop(reader);
    drop(line);
    let retained_start = latest
        .front()
        .map(|(offset, _)| *offset)
        .unwrap_or(tail_start);
    parser.snapshot.window_start = retained_start;
    parser.snapshot.window_end = window_end;
    parser.snapshot.has_earlier = retained_start > header.len() as u64;
    // Parse only the newest bounded event queue. Tool results whose calls fell
    // outside the retained window stay explicitly unpaired, never guessed.
    for (offset, line) in latest {
        match serde_json::from_slice::<Value>(&line) {
            Ok(event) => {
                if session.source == "codex" {
                    parser.codex(&event, offset as usize + 1);
                } else {
                    parser.deepseek(&event, offset as usize + 1);
                }
                if parser.snapshot.error.is_some() {
                    break;
                }
            }
            Err(_) => parser.malformed += 1,
        }
    }
    if omitted {
        parser.warning("Earlier history omitted: showing the validated session header and newest complete rows within the 16 MiB / 20,000 event limit. Earlier calls, context and child links may be absent.");
    }
    if truncated {
        parser.warning(
            "The decoded recording was truncated before collection; later events may be absent.",
        );
    }
    parser.finish()
}

pub fn open_code_sessions(project: &str) -> Result<Vec<Session>, String> {
    if project.trim().is_empty() {
        return Err("Select a project to read recorded sessions.".into());
    }
    let executable = opencode_executable()?;
    let output = crate::backend::run(
        &executable,
        &["session", "list", "--format", "json", "--max-count", "256"],
        Some(project),
        Duration::from_secs(8),
        CATALOG_BYTES,
    )
    .map_err(|error| redact(&error))?;
    let value: Value = serde_json::from_str(&output)
        .map_err(|_| "OpenCode returned invalid session-list JSON.".to_owned())?;
    let rows = value
        .as_array()
        .or_else(|| value["sessions"].as_array())
        .or_else(|| value["data"].as_array())
        .ok_or("OpenCode returned an unsupported session-list format.")?;
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    for row in rows.iter().take(CATALOG_HEADERS) {
        let id = text(&row["id"]);
        let cwd = text(first(row, &["directory", "cwd", "path"]));
        let cwd = if cwd.is_empty() {
            text(&row["location"]["directory"])
        } else {
            cwd
        };
        // An unscoped CLI list may include other projects. Require an explicit
        // matching recorded directory rather than guessing from invocation cwd.
        if !valid_id(&id, "opencode") || !same_project(&cwd, project) || !seen.insert(id.clone()) {
            continue;
        }
        let title = text(&row["title"]);
        found.push(Session {
            id,
            title: if title.is_empty() {
                "OpenCode session".into()
            } else {
                redact(&title)
            },
            project: project.into(),
            source: "opencode".into(),
            updated: epoch_updated(&row["time"]["updated"]),
            parent_id: text(first(row, &["parentID", "parentSessionID"])),
            agent: redact(&text(&row["agent"])),
            ..Session::default()
        });
    }
    Ok(found)
}

pub fn open_code_collect(project: &str, id: &str) -> Snapshot {
    let session = Session {
        id: id.into(),
        project: project.into(),
        source: "opencode".into(),
        title: "OpenCode session".into(),
        ..Session::default()
    };
    let failed = |error: String| Snapshot {
        session: session.clone(),
        error: Some(redact(&error)),
        ..Snapshot::default()
    };
    if project.trim().is_empty() || !valid_id(id, "opencode") {
        return failed("A project and valid recorded OpenCode session ID are required.".into());
    }
    let executable = match opencode_executable() {
        Ok(path) => path,
        Err(error) => return failed(error),
    };
    match crate::backend::run(
        &executable,
        &["session", "export", id],
        Some(project),
        Duration::from_secs(8),
        SESSION_BYTES,
    ) {
        Ok(output) => parse_opencode(&output, &session),
        Err(error) => failed(error),
    }
}

fn opencode_executable() -> Result<String, String> {
    let home = std::env::var_os("HOME").ok_or("The home directory is unavailable.")?;
    let path = PathBuf::from(home).join(".opencode/bin/opencode");
    if !path.is_file() {
        return Err(
            "The installed OpenCode executable was not found at ~/.opencode/bin/opencode.".into(),
        );
    }
    Ok(path.to_string_lossy().into_owned())
}

fn parse_opencode(output: &str, session: &Session) -> Snapshot {
    let mut parser = Parser::new(session);
    if output.len() > SESSION_BYTES {
        parser.snapshot.error = Some("The OpenCode export exceeds 16 MiB.".into());
        return parser.snapshot;
    }
    let value: Value = match serde_json::from_str(output) {
        Ok(value) => value,
        Err(_) => {
            parser.snapshot.error = Some("OpenCode returned invalid export JSON.".into());
            return parser.snapshot;
        }
    };
    let root = if value["info"].is_null() && value["data"].is_object() {
        &value["data"]
    } else {
        &value
    };
    let info = &root["info"];
    let directory = text(first(info, &["directory", "cwd"]));
    let directory = if directory.is_empty() {
        text(&info["location"]["directory"])
    } else {
        directory
    };
    if text(&info["id"]) != session.id
        || !same_project(&directory, &session.project)
        || !root["messages"].is_array()
    {
        parser.snapshot.error = Some(
            "OpenCode returned an unsupported export or a different workspace/session.".into(),
        );
        return parser.snapshot;
    }
    parser.header = true;
    let title = text(&info["title"]);
    if !title.is_empty() {
        parser.snapshot.session.title = redact(&title);
    }
    parser.snapshot.session.parent_id = text(first(info, &["parentID", "parentSessionID"]));
    parser.snapshot.session.updated = epoch_updated(&info["time"]["updated"]);
    parser.snapshot.model = model(&info["model"]);
    parser.thread_usage = usage(&info["tokens"]);
    parser.record(
        "opencode-header",
        "",
        "runtime",
        "OpenCode session identity",
        info,
        true,
    );
    let mut events = 0;
    for (index, raw) in root["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        if events >= SESSION_EVENTS {
            break;
        }
        let message = if raw["info"].is_object() {
            &raw["info"]
        } else {
            raw
        };
        let role = text(first(message, &["type", "role"]));
        let id = text(&message["id"]);
        let id = if id.is_empty() {
            format!("message-{index}")
        } else {
            id
        };
        let time = text(&message["time"]["created"]);
        let mut content = text(&message["text"]);
        let parts = first(raw, &["parts", "content"]);
        for (part_index, part) in parts.as_array().into_iter().flatten().enumerate() {
            if events >= SESSION_EVENTS {
                break;
            }
            events += 1;
            let kind = text(&part["type"]);
            match kind.as_str() {
                "text" | "reasoning" => {
                    if !content.is_empty() {
                        content.push_str("\n\n");
                    }
                    content.push_str(&text(&part["text"]));
                }
                "tool" => {
                    let state = &part["state"];
                    let input = first(state, &["input", "arguments"]);
                    let input = if input.is_null() {
                        &part["input"]
                    } else {
                        input
                    };
                    let output = first(state, &["output", "content", "error"]);
                    let name = text(first(part, &["tool", "name"]));
                    let call_id = text(first(part, &["callID", "id"]));
                    let call_id = if call_id.is_empty() {
                        format!("{id}-tool-{part_index}")
                    } else {
                        call_id
                    };
                    parser.tool(
                        &call_id,
                        &name,
                        &time,
                        &json!({"part": part, "input": input}),
                        false,
                        input,
                        &Value::Null,
                    );
                    if !output.is_null()
                        || ["completed", "error"].contains(&text(&state["status"]).as_str())
                    {
                        parser.tool(&call_id, &name, &time, part, true, &Value::Null, &json!({"output": output, "error": state["error"], "status": state["status"], "metadata": state["metadata"]}));
                    }
                }
                _ => parser.record(
                    &format!("{id}-part-{part_index}"),
                    &time,
                    "context",
                    &format!("Recorded part · {kind}"),
                    part,
                    true,
                ),
            }
        }
        if role == "assistant" {
            let current_model = model(&message["model"]);
            if !current_model.is_empty() {
                parser.snapshot.model = current_model;
            } else if let Some(model) = message["modelID"].as_str() {
                parser.snapshot.model = redact(model);
            }
            if let Some(usage) = usage(&message["tokens"]) {
                parser.usages.insert(id.clone(), usage);
            }
        }
        let infrastructure = ["system", "developer", "synthetic", "skill"].contains(&role.as_str())
            || message["synthetic"] == true
            || (role == "user" && !human_prompt(&content));
        if role == "user" && !infrastructure {
            parser.prompt(&content);
        }
        parser.record(
            &id,
            &time,
            if infrastructure { "context" } else { &role },
            &content,
            raw,
            infrastructure,
        );
        events += 1;
    }
    if events >= SESSION_EVENTS {
        parser.warning("OpenCode export truncated at 20,000 events; later events may be absent.");
    }
    parser.finish()
}

fn first<'a>(value: &'a Value, keys: &[&str]) -> &'a Value {
    keys.iter()
        .find_map(|key| value.get(*key).filter(|v| !v.is_null()))
        .unwrap_or(&Value::Null)
}

fn text(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Number(value) => value.to_string(),
        _ => String::new(),
    }
}

fn model(value: &Value) -> String {
    redact(&if value.is_object() {
        text(first(value, &["modelID", "id", "model", "name"]))
    } else {
        text(value)
    })
}

fn epoch_updated(value: &Value) -> String {
    let recorded = text(value);
    recorded
        .parse::<u64>()
        .map(|number| {
            if number > 100_000_000_000 {
                number / 1_000
            } else {
                number
            }
            .to_string()
        })
        .unwrap_or(recorded)
}

fn decoded(value: &Value) -> Value {
    if let Some(text) = value.as_str() {
        serde_json::from_str(text).unwrap_or_else(|_| value.clone())
    } else {
        value.clone()
    }
}

fn render(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(value) => redact(value),
        Value::Array(values) => values
            .iter()
            .map(render)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
        Value::Object(_) => {
            if let Some(text) = value["text"].as_str() {
                return redact(text);
            }
            let content = first(value, &["content", "output"]);
            if !content.is_null() {
                render(content)
            } else {
                pretty(value)
            }
        }
        _ => value.to_string(),
    }
}

fn shortened(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let prefix = chars.by_ref().take(max).collect::<String>();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

fn usage(value: &Value) -> Option<Usage> {
    if !value.is_object() {
        return None;
    }
    let number = |keys: &[&str]| {
        keys.iter().find_map(|key| {
            value[*key]
                .as_u64()
                .or_else(|| value[*key].as_str().and_then(|s| s.parse::<u64>().ok()))
        })
    };
    let input = number(&["input_tokens", "inputTokens", "input"]);
    let output = number(&["output_tokens", "outputTokens", "output"]);
    let total = number(&["total_tokens", "totalTokens", "total"]);
    if input.is_none() && output.is_none() && total.is_none() {
        return None;
    }
    Some(Usage {
        input: input.unwrap_or(0),
        output: output.unwrap_or(0),
        total: total.unwrap_or_else(|| input.unwrap_or(0).saturating_add(output.unwrap_or(0))),
    })
}

fn sum_usage<'a>(values: impl Iterator<Item = &'a Usage>) -> Option<Usage> {
    let mut result = None::<Usage>;
    for usage in values {
        let total = result.get_or_insert_with(Usage::default);
        total.input = total.input.saturating_add(usage.input);
        total.output = total.output.saturating_add(usage.output);
        total.total = total.total.saturating_add(usage.total);
    }
    result
}

fn human_prompt(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty()
        && ![
            "# AGENTS.md instructions",
            "<environment_context>",
            "<permissions instructions>",
            "<INSTRUCTIONS>",
            "<system",
            "<developer",
            "<turn_aborted>",
            "<runtime_context>",
            "<subagent_notification>",
        ]
        .iter()
        .any(|prefix| text.starts_with(prefix))
}

fn internal_codex(info: &Value) -> bool {
    let internal = |value: &str| {
        ["guardian", "auto_review", "auto-review", "approval_review"]
            .contains(&value.to_ascii_lowercase().as_str())
    };
    ["source", "thread_source", "originator", "agent_role"]
        .iter()
        .any(|key| internal(&text(&info[*key])))
        || info["source"]["subagent"]
            .as_object()
            .is_some_and(|object| object.keys().any(|key| internal(key)))
}

fn subagent_tool(name: &str) -> bool {
    [
        "spawn_agent",
        "task",
        "agent",
        "subagent",
        "subagent_fork",
        "delegate_task",
    ]
    .contains(&name.rsplit('.').next().unwrap_or(name))
}

fn linked_session(value: &Value, source: &str, depth: usize) -> Option<String> {
    if depth > 8 {
        return None;
    }
    match value {
        Value::Object(_) => {
            let keys = if source == "codex" {
                &["agent_id", "agentId", "thread_id", "session_id"][..]
            } else {
                &[
                    "sessionId",
                    "sessionID",
                    "session_id",
                    "childId",
                    "childSessionId",
                    "childSessionID",
                    "subagentId",
                    "subagentSessionId",
                    "subagentSessionID",
                    "task_id",
                ][..]
            };
            for key in keys {
                let id = text(&value[*key]);
                if valid_id(&id, source) {
                    return Some(id);
                }
            }
            for key in ["output", "content", "result", "text", "data", "metadata"] {
                if let Some(id) = linked_session(&value[key], source, depth + 1) {
                    return Some(id);
                }
            }
            None
        }
        Value::Array(values) => values
            .iter()
            .find_map(|value| linked_session(value, source, depth + 1)),
        Value::String(text) => serde_json::from_str::<Value>(text)
            .ok()
            .and_then(|value| linked_session(&value, source, depth + 1)),
        _ => None,
    }
}

fn failure(value: &Value, depth: usize) -> bool {
    if depth > 8 {
        return false;
    }
    match value {
        Value::Object(_) => {
            value["isError"] == true
                || value["is_error"] == true
                || value["exit_code"].as_i64().is_some_and(|v| v != 0)
                || ["failed", "error"]
                    .contains(&text(&value["status"]).to_ascii_lowercase().as_str())
                || (!value["error"].is_null() && !render(&value["error"]).is_empty())
                || ["output", "content", "result", "text"]
                    .iter()
                    .any(|key| failure(&value[*key], depth + 1))
        }
        Value::Array(values) => values.iter().any(|value| failure(value, depth + 1)),
        Value::String(text) => {
            if let Ok(value) = serde_json::from_str::<Value>(text) {
                failure(&value, depth + 1)
            } else {
                text.lines().any(|line| {
                    line.strip_prefix("Process exited with code ")
                        .and_then(|code| code.trim().parse::<i64>().ok())
                        .is_some_and(|code| code != 0)
                })
            }
        }
        _ => false,
    }
}

fn valid_id(id: &str, source: &str) -> bool {
    if id.len() > 160 {
        return false;
    }
    match source {
        "codex" => {
            id.len() == 36
                && id.bytes().enumerate().all(|(index, byte)| {
                    if [8, 13, 18, 23].contains(&index) {
                        byte == b'-'
                    } else {
                        byte.is_ascii_hexdigit()
                    }
                })
        }
        "deepseek" => id.strip_prefix("session-").is_some_and(valid_id_tail),
        "opencode" => id.strip_prefix("ses_").is_some_and(valid_id_tail),
        _ => false,
    }
}

fn valid_id_tail(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn first_json_line(bytes: &[u8]) -> Option<Value> {
    serde_json::from_slice(
        bytes
            .split(|byte| *byte == b'\n')
            .find(|line| !line.iter().all(u8::is_ascii_whitespace))?,
    )
    .ok()
}

// The DSH projection cache also contains message rows. Its bounded prefix can
// end inside those rows; only complete values along the requested object path
// are needed for discovery, so the rest of that JSON document need not be read.
fn prefix_json_path(mut bytes: &[u8], path: &[&str]) -> Option<Value> {
    let whitespace = |bytes: &[u8]| {
        bytes
            .iter()
            .position(|byte| !byte.is_ascii_whitespace())
            .unwrap_or(bytes.len())
    };
    bytes = &bytes[whitespace(bytes)..];
    if path.is_empty() {
        return serde_json::Deserializer::from_slice(bytes)
            .into_iter::<Value>()
            .next()?
            .ok();
    }
    if bytes.first() != Some(&b'{') {
        return None;
    }
    bytes = &bytes[1..];
    loop {
        bytes = &bytes[whitespace(bytes)..];
        if bytes.first() != Some(&b'"') {
            return None;
        }
        let mut key_stream = serde_json::Deserializer::from_slice(bytes).into_iter::<String>();
        let key = key_stream.next()?.ok()?;
        bytes = &bytes[key_stream.byte_offset()..];
        bytes = &bytes[whitespace(bytes)..];
        if bytes.first() != Some(&b':') {
            return None;
        }
        bytes = &bytes[1..];
        if key == path[0] {
            return prefix_json_path(bytes, &path[1..]);
        }
        let mut value_stream = serde_json::Deserializer::from_slice(bytes).into_iter::<Value>();
        value_stream.next()?.ok()?;
        bytes = &bytes[value_stream.byte_offset()..];
        bytes = &bytes[whitespace(bytes)..];
        if bytes.first() != Some(&b',') {
            return None;
        }
        bytes = &bytes[1..];
    }
}

fn normalized(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            _ => result.push(component.as_os_str()),
        }
    }
    result
}

fn same_project(left: &str, right: &str) -> bool {
    if left.is_empty() || right.is_empty() {
        return false;
    }
    let canonical =
        |path: &str| fs::canonicalize(path).unwrap_or_else(|_| normalized(Path::new(path)));
    canonical(left) == canonical(right)
}

fn safe_directory(path: &Path, root: &Path) -> bool {
    safe_path(path, root) && fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

fn safe_file(path: &Path, root: &Path) -> bool {
    safe_path(path, root) && fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
}

fn safe_path(path: &Path, root: &Path) -> bool {
    let root = normalized(root);
    let path = normalized(path);
    if !path.is_absolute() || !path.starts_with(&root) {
        return false;
    }
    if fs::canonicalize(&root).map_or(true, |canonical| canonical != root) {
        return false;
    }
    let mut cursor = Some(path.as_path());
    while let Some(path) = cursor {
        if fs::symlink_metadata(path).map_or(true, |m| m.file_type().is_symlink()) {
            return false;
        }
        if path == root {
            return true;
        }
        cursor = path.parent();
    }
    false
}

fn modified(path: &Path) -> u64 {
    fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .unwrap_or_default()
        .as_secs()
}

// Gregorian conversion from UTC epoch days; no time-zone or external command.
fn civil_date(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

/// Redacts credentials in free text, nested JSON strings, environment objects,
/// authorization headers and private URLs. Shared by telemetry and UI logs.
pub fn redact(text: &str) -> String {
    redact_text(text, 0)
}

fn redact_text(text: &str, depth: usize) -> String {
    if depth >= 8 && (text.trim_start().starts_with('{') || text.trim_start().starts_with('[')) {
        return "[nested JSON omitted]".into();
    }
    if depth < 8 && (text.trim_start().starts_with('{') || text.trim_start().starts_with('[')) {
        if let Ok(value) = serde_json::from_str::<Value>(text) {
            return serde_json::to_string(&redact_value(&value, depth + 1))
                .unwrap_or_else(|_| "[unreadable record]".into());
        }
    }
    redact_free_text(text).into_owned()
}

fn redaction_rules() -> &'static [(Regex, &'static str)] {
    static RULES: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    RULES.get_or_init(|| [
        (r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----", "[private key redacted]"),
        (r#"(?i)\b([A-Z_][A-Z0-9_]*(?:API_KEY|SECRET_ACCESS_KEY|ACCESS_KEY_ID|TOKEN|PASSWORD|SECRET|PRIVATE_KEY|CREDENTIALS?)|TOKEN|SECRET|PASSWORD|API_KEY|ACCESS_KEY_ID)["']?\s*[:=]\s*(?:"[^"]*"|'[^']*'|[^\s"'`,;]+)"#, "$1=[redacted]"),
        (r"(?i)\b(Bearer|Basic)\s+[A-Za-z0-9._~+/=-]+", "$1 [redacted]"),
        (r#"(?i)\b(api[_-]?key|access[_-]?token|refresh[_-]?token|password|client[_-]?secret|authorization|cookie)["']?\s*[:=]\s*(?:"[^"]*"|'[^']*'|[^\s"'`,;]+)"#, "$1=[redacted]"),
        (r"\b(?:sk-[A-Za-z0-9_-]{16,}|gh[pousr]_[A-Za-z0-9]{16,}|github_pat_[A-Za-z0-9_]{16,}|xox[baprs]-[A-Za-z0-9-]{12,}|AKIA[A-Z0-9]{16})\b", "[credential redacted]"),
    ].into_iter().map(|(pattern, replacement)| (Regex::new(pattern).expect("static redaction regex"), replacement)).collect())
}

fn redaction_urls() -> &'static Regex {
    static URLS: OnceLock<Regex> = OnceLock::new();
    URLS.get_or_init(|| Regex::new(r#"https?://[^\s<>"']+"#).expect("static URL regex"))
}

fn redact_free_text(text: &str) -> Cow<'_, str> {
    let mut safe = Cow::Borrowed(text);
    for (regex, replacement) in redaction_rules() {
        // Short metadata fields benefit from avoiding capture setup on misses.
        // The extra search costs more on larger payloads, so keep their path.
        if safe.len() <= 64 && !regex.is_match(safe.as_ref()) {
            continue;
        }
        if let Cow::Owned(replaced) = regex.replace_all(safe.as_ref(), *replacement) {
            safe = Cow::Owned(replaced);
        }
    }
    let mut output = None;
    let mut copied = 0;
    for matched in redaction_urls().find_iter(&safe) {
        let url = matched.as_str();
        let (scheme, rest) = url.split_once("://").unwrap_or(("https", ""));
        let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let authority = &rest[..authority_end];
        let tail = &rest[authority_end..];
        let sensitive_query = tail.split_once('?').is_some_and(|(_, query)| {
            query.split('&').any(|pair| {
                let key = pair.split('=').next().unwrap_or("");
                [
                    "token",
                    "key",
                    "secret",
                    "password",
                    "auth",
                    "credential",
                    "code",
                ]
                .iter()
                .any(|part| contains_ascii_case_insensitive(key, part))
            })
        });
        if authority.contains('@')
            || tail.contains('#')
            || contains_ascii_case_insensitive(tail, "/pair")
            || sensitive_query
        {
            let output = output.get_or_insert_with(|| String::with_capacity(safe.len()));
            output.push_str(&safe[copied..matched.start()]);
            output.push_str(scheme);
            output.push_str("://");
            output.push_str(authority.rsplit('@').next().unwrap_or("[redacted]"));
            copied = matched.end();
        }
    }
    if let Some(mut output) = output {
        output.push_str(&safe[copied..]);
        Cow::Owned(output)
    } else {
        safe
    }
}

fn contains_ascii_case_insensitive(text: &str, pattern: &str) -> bool {
    text.as_bytes()
        .windows(pattern.len())
        .any(|window| window.eq_ignore_ascii_case(pattern.as_bytes()))
}

fn redact_value(value: &Value, depth: usize) -> Value {
    if depth > 32 {
        return Value::String("[nested value omitted]".into());
    }
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| {
                    let normalized = key
                        .chars()
                        .filter(|c| c.is_ascii_alphanumeric())
                        .flat_map(char::to_lowercase)
                        .collect::<String>();
                    let sensitive = [
                        "apikey",
                        "accesskey",
                        "accesstoken",
                        "refreshtoken",
                        "authorization",
                        "password",
                        "secret",
                        "credential",
                        "privatekey",
                        "clientsecret",
                        "cookie",
                    ]
                    .iter()
                    .any(|word| normalized.contains(word))
                        || (normalized.ends_with("token") && value.is_string())
                        || ["env", "environment"].contains(&normalized.as_str());
                    (
                        redact_text(key, depth + 1),
                        if sensitive {
                            Value::String("[redacted]".into())
                        } else {
                            redact_value(value, depth + 1)
                        },
                    )
                })
                .collect(),
        ),
        Value::Array(array) => Value::Array(
            array
                .iter()
                .map(|value| redact_value(value, depth + 1))
                .collect(),
        ),
        Value::String(text) => Value::String(redact_text(text, depth + 1)),
        _ => value.clone(),
    }
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(&redact_value(value, 0))
        .unwrap_or_else(|_| "[unreadable record]".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::os::unix::fs::PermissionsExt;
    use std::rc::Rc;
    const ROOT_ID: &str = "11111111-2222-3333-4444-555555555555";
    const CHILD_ID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            Self::with_nonce(nonce)
        }

        fn with_nonce(nonce: u128) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = fs::canonicalize(std::env::temp_dir())
                .unwrap()
                .join(format!(
                    "agentwatcher-telemetry-test-{}-{nonce}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            // This unique directory was created by this test instance only.
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn concurrent_scratch_directories_with_the_same_timestamp_are_independent() {
        let [first, second] = std::thread::scope(|scope| {
            let first = scope.spawn(|| Scratch::with_nonce(0));
            let second = scope.spawn(|| Scratch::with_nonce(0));
            [first.join().unwrap(), second.join().unwrap()]
        });
        assert_ne!(first.0, second.0);
        let retained = first.0.join("retained.txt");
        fs::write(&retained, "first fixture").unwrap();
        let second_path = second.0.clone();
        drop(second);
        assert!(!second_path.exists());
        assert_eq!(fs::read_to_string(retained).unwrap(), "first fixture");
    }

    fn session(source: &str) -> Session {
        Session {
            id: if source == "codex" {
                ROOT_ID
            } else if source == "deepseek" {
                "session-root"
            } else {
                "ses_root"
            }
            .into(),
            project: "/synthetic/agentwatcher".into(),
            source: source.into(),
            ..Session::default()
        }
    }

    fn jsonl(events: &[Value], session: &Session) -> Snapshot {
        let text = events
            .iter()
            .map(|event| serde_json::to_string(event).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        parse_jsonl(Cursor::new(text.as_bytes()), session, false)
    }

    #[test]
    fn codex_pairs_exact_ids_latest_human_and_cumulative_usage() {
        let snapshot = jsonl(
            &[
                json!({"type":"session_meta","payload":{"id":ROOT_ID,"cwd":"/synthetic/agentwatcher"}}),
                json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Original task"}]}}),
                json!({"type":"response_item","payload":{"type":"function_call","call_id":"call-a","name":"spawn_agent","arguments":"{\"task_name\":\"reader\",\"agent_type\":\"worker\"}"}}),
                json!({"type":"response_item","payload":{"type":"function_call","call_id":"call-ab","name":"exec","arguments":{"command":"pwd"}}}),
                json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"call-ab","output":"Process exited with code 2"}}),
                json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"call-a","output":format!("{{\"agent_id\":\"{CHILD_ID}\"}}")}}),
                json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"text":"Latest human task"}]}}),
                json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"text":"<environment_context>injected</environment_context>"}]}}),
                json!({"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":2,"output_tokens":3,"total_tokens":5}}}}),
                json!({"type":"token_usage_record","payload":{"thread_id":ROOT_ID,"thread_token_usage":{"input_tokens":12,"output_tokens":8,"total_tokens":20}}}),
                json!({"type":"token_usage_record","payload":{"thread_id":CHILD_ID,"thread_token_usage":{"total_tokens":999}}}),
            ],
            &session("codex"),
        );
        assert!(snapshot.error.is_none());
        assert_eq!(snapshot.session.title, "Latest human task");
        assert_eq!(snapshot.usage.unwrap().total, 20);
        assert_eq!(snapshot.children[0].session_id, CHILD_ID);
        assert_eq!(snapshot.children[0].title, "reader");
        assert!(
            snapshot
                .records
                .iter()
                .find(|r| r.id == "call-ab")
                .unwrap()
                .summary
                .contains("Error")
        );
        let spawn = snapshot.records.iter().find(|r| r.id == "call-a").unwrap();
        assert!(spawn.summary.contains("Completed"));
        assert_eq!(spawn.tool_status, "Completed");
        assert_eq!(
            snapshot
                .records
                .iter()
                .find(|record| record.id == "call-ab")
                .unwrap()
                .tool_status,
            "Error"
        );
        assert!(!spawn.detail.contains("Process exited with code 2"));
        assert!(
            snapshot
                .records
                .iter()
                .filter(|record| record.kind != "tool")
                .all(|record| record.tool_status.is_empty())
        );
        assert!(
            snapshot
                .records
                .iter()
                .any(|record| record.is_infrastructure && record.summary.contains("injected"))
        );
    }

    #[test]
    fn deepseek_v4_uses_source_discriminant_and_structured_child() {
        let snapshot = jsonl(
            &[
                json!({"type":"session","version":4,"id":"session-root","cwd":"/synthetic/agentwatcher"}),
                json!({"type":"user/message","data":{"message":{"role":"user","source":{"kind":"user"},"content":[{"text":"Keep the real task"}]}}}),
                json!({"type":"user/message","data":{"message":{"role":"user","source":{"kind":"runtime"},"content":[{"text":"Replace the task incorrectly"}]}}}),
                json!({"type":"tool/call","data":{"callId":"dsh-1","name":"subagent","arguments":{"title":"Telemetry"}}}),
                json!({"type":"tool/result","data":{"message":{"toolCallId":"dsh-1","content":[{"text":"{\"sessionId\":\"session-child\"}"}],"isError":false}}}),
                json!({"type":"assistant/message","data":{"message":{"id":"answer-1","role":"assistant","source":{"model":"deepseek"},"text":"Done"},"usage":{"inputTokens":10,"outputTokens":4}}}),
                json!({"type":"assistant/message","data":{"message":{"id":"answer-1","role":"assistant","text":"Done"},"usage":{"inputTokens":10,"outputTokens":4}}}),
            ],
            &session("deepseek"),
        );
        assert!(snapshot.error.is_none());
        assert_eq!(snapshot.session.title, "Keep the real task");
        assert_eq!(snapshot.children[0].session_id, "session-child");
        assert_eq!(snapshot.usage.unwrap().total, 14);
        assert_eq!(snapshot.model, "deepseek");
        assert_eq!(
            snapshot
                .records
                .iter()
                .find(|record| record.id == "dsh-1")
                .unwrap()
                .tool_status,
            "Completed"
        );
    }

    #[test]
    fn opencode_parts_preserve_tool_and_usage() {
        let value = json!({"info":{"id":"ses_root","directory":"/synthetic/agentwatcher","model":{"modelID":"test-model"}},"messages":[
            {"info":{"id":"m1","role":"user"},"parts":[{"type":"text","text":"Inspect this project"}]},
            {"info":{"id":"m2","role":"assistant","tokens":{"input":6,"output":2}},"parts":[{"id":"part1","callID":"exact-call","type":"tool","tool":"task","state":{"status":"completed","input":{"description":"Child"},"output":"Done","metadata":{"sessionId":"ses_child"}}}]}
        ]});
        let snapshot = parse_opencode(&value.to_string(), &session("opencode"));
        assert!(snapshot.error.is_none());
        assert_eq!(
            snapshot
                .records
                .iter()
                .find(|record| record.id == "exact-call")
                .unwrap()
                .tool_status,
            "Completed"
        );
        assert_eq!(snapshot.session.title, "Inspect this project");
        assert_eq!(snapshot.usage.unwrap().total, 8);
        assert_eq!(snapshot.children[0].session_id, "ses_child");
        assert!(
            snapshot
                .records
                .iter()
                .any(|record| record.id == "exact-call" && record.detail.contains("metadata"))
        );
    }

    #[test]
    fn redaction_is_consistent_for_env_nested_json_and_private_urls() {
        let input = json!({"arguments":json!({"env":{"SAFE_NAME":"also-hidden"},"api_key":"nested-secret","input_tokens":44}).to_string(),
            "output":"OPENAI_API_KEY='plain-secret' Bearer bearer-secret https://user:pass@example.test/pair?code=secret#fragment",
            "password":"object-secret"});
        let safe = redact(&input.to_string());
        for secret in [
            "also-hidden",
            "nested-secret",
            "plain-secret",
            "bearer-secret",
            "object-secret",
            "user:pass",
            "code=secret",
            "#fragment",
        ] {
            assert!(!safe.contains(secret), "leaked {secret}");
        }
        assert!(safe.contains("input_tokens"));
        assert!(safe.contains("44"));
        assert_eq!(redact(&safe), safe);
        assert!(
            !redact("-----BEGIN PRIVATE KEY-----\nsecret\n-----END PRIVATE KEY-----")
                .contains("secret")
        );
    }

    #[test]
    fn redaction_keeps_safe_text_borrowed_and_preserves_public_urls() {
        for input in [
            "Agent α read rust/src/main.rs and finished successfully.",
            "View https://example.test/docs?topic=graphs and http://example.test/作業 next.",
            "https://example.test/repaired?topic=public",
            "",
        ] {
            assert!(matches!(redact_free_text(input), Cow::Borrowed(_)));
            assert_eq!(redact(input), input);
        }
        assert_eq!(
            redact(
                "safe https://example.test/docs?topic=graphs private https://user:pass@example.test/path middle http://other.test/PAIR/abc public https://last.test/help end"
            ),
            "safe https://example.test/docs?topic=graphs private https://example.test middle http://other.test public https://last.test/help end"
        );
        for input in [
            "https://example.test/path?CoDe=private",
            "https://example.test/path?apiKEY=private",
            "https://example.test/path#private",
            "https://example.test/pAiRiNg/private",
        ] {
            assert_eq!(redact(input), "https://example.test");
        }
    }

    // Retain the previous allocation path for a deterministic, same-process
    // comparison. It shares compiled rules, so regex initialization is excluded.
    fn legacy_redact_free_text(text: &str) -> String {
        let mut safe = text.to_owned();
        for (regex, replacement) in redaction_rules() {
            safe = regex.replace_all(&safe, *replacement).into_owned();
        }
        redaction_urls()
            .replace_all(&safe, |capture: &regex::Captures<'_>| {
                let url = &capture[0];
                let (scheme, rest) = url.split_once("://").unwrap_or(("https", ""));
                let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
                let authority = &rest[..authority_end];
                let tail = &rest[authority_end..];
                let sensitive_query = tail.split_once('?').is_some_and(|(_, query)| {
                    query.split('&').any(|pair| {
                        let key = pair.split('=').next().unwrap_or("").to_ascii_lowercase();
                        [
                            "token",
                            "key",
                            "secret",
                            "password",
                            "auth",
                            "credential",
                            "code",
                        ]
                        .iter()
                        .any(|part| key.contains(part))
                    })
                });
                if authority.contains('@')
                    || tail.contains('#')
                    || tail.to_ascii_lowercase().contains("/pair")
                    || sensitive_query
                {
                    format!(
                        "{scheme}://{}",
                        authority.rsplit('@').next().unwrap_or("[redacted]")
                    )
                } else {
                    url.to_owned()
                }
            })
            .into_owned()
    }

    #[test]
    fn redaction_matches_previous_rules_across_safe_and_sensitive_text() {
        let cases = [
            "ordinary agent activity without credentials",
            "-----BEGIN RSA PRIVATE KEY-----\nprivate\n-----END RSA PRIVATE KEY-----",
            "OPENAI_API_KEY='private' AWS_SECRET_ACCESS_KEY=private PASSWORD=private",
            "Bearer private Basic dXNlcjpwYXNzd29yZA==",
            "api-key: private access_token=private refresh_token=private cookie=private",
            "sk-abcdefghijklmnop ghp_abcdefghijklmnop github_pat_abcdefghijklmnop xoxb-abcdefghijkl AKIAABCDEFGHIJKLMNOP",
            "https://user:pass@example.test/path?topic=public https://example.test/PAIR/private https://example.test/path?CoDe=private#private",
            "http://example.test/作業?topic=public https://example.test/path?monkey=public",
            "https://one.test/docs https://two.test/path#private https://three.test/help https://four.test/path?auth=private https://five.test/docs",
        ];
        for input in cases {
            assert_eq!(redact(input), legacy_redact_free_text(input), "{input}");
        }
    }

    #[test]
    fn redaction_short_fields_preserve_credentials_at_filter_boundaries() {
        for input in [
            "ordinary agent α activity",
            "OPENAI_API_KEY='private'",
            "API_KEY=private",
            "ſecret=private",
            "Baſic private",
            "Bearer private",
            "sk-abcdefghijklmnop",
            "ghp_abcdefghijklmnop",
            "https://u:p@example.test/pair?code=private",
            "-----BEGIN PRIVATE KEY-----\nprivate\n-----END PRIVATE KEY----- Bearer private",
        ] {
            for length in [0_usize, 63, 64, 65, 128] {
                let padded = format!("{input}{}", " ".repeat(length.saturating_sub(input.len())));
                assert_eq!(
                    redact(&padded),
                    legacy_redact_free_text(&padded),
                    "redaction changed for a {}-byte field",
                    padded.len()
                );
            }
        }
    }

    #[test]
    #[ignore = "manual same-process redaction performance comparison"]
    fn benchmark_redaction_allocation_path() {
        let safe = "Agent α read rust/src/main.rs, inspected the graph, and completed the requested work. View https://example.test/docs?topic=graphs for details.\n".repeat(32);
        let sensitive = format!(
            "{safe}OPENAI_API_KEY='private' Bearer private https://user:pass@example.test/pair?code=private#fragment\n{safe}"
        );
        let cases = [safe.as_str(), sensitive.as_str()];
        for input in cases {
            assert_eq!(redact(input), legacy_redact_free_text(input));
        }
        let measure = |redact: fn(&str) -> String| {
            let start = Instant::now();
            for _ in 0..500 {
                for input in cases {
                    std::hint::black_box(redact(std::hint::black_box(input)));
                }
            }
            start.elapsed()
        };
        let mut legacy = Vec::new();
        let mut current = Vec::new();
        for pass in 0..3 {
            if pass % 2 == 0 {
                legacy.push(measure(legacy_redact_free_text));
                current.push(measure(redact));
            } else {
                current.push(measure(redact));
                legacy.push(measure(legacy_redact_free_text));
            }
        }
        legacy.sort();
        current.sort();
        eprintln!(
            "Redaction (1,000 mixed 4–9 KiB records, median of 3): previous {:.2} ms, current {:.2} ms, {:.2}x",
            legacy[1].as_secs_f64() * 1_000.0,
            current[1].as_secs_f64() * 1_000.0,
            legacy[1].as_secs_f64() / current[1].as_secs_f64()
        );
    }

    #[test]
    fn rejects_workspace_mismatch_and_unproven_child_prose() {
        let snapshot = jsonl(
            &[json!({"type":"session_meta","payload":{"id":ROOT_ID,"cwd":"/different/project"}})],
            &session("codex"),
        );
        assert!(snapshot.error.is_some());
        assert!(linked_session(&json!(format!("I mentioned {CHILD_ID}")), "codex", 0).is_none());
    }

    #[test]
    fn normalized_tool_status_is_empty_for_generic_records() {
        let snapshot = jsonl(
            &[
                json!({"type":"session_meta","payload":{"id":ROOT_ID,"cwd":"/synthetic/agentwatcher"}}),
                json!({"type":"response_item","payload":{"type":"function_call","call_id":"pending-call","name":"exec","arguments":{"command":"pwd"}}}),
                json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"CommandExecution","id":"generic-command","command":"echo generic","status":"completed"}}}),
            ],
            &session("codex"),
        );
        assert!(snapshot.error.is_none());
        assert_eq!(
            snapshot
                .records
                .iter()
                .find(|record| record.id == "pending-call")
                .unwrap()
                .tool_status,
            "Result not recorded"
        );
        let generic = snapshot
            .records
            .iter()
            .find(|record| record.id == "generic-command")
            .unwrap();
        assert_eq!(generic.kind, "tool");
        assert!(generic.tool_status.is_empty());
        assert!(generic.detail.contains("completed"));
        assert!(
            snapshot
                .records
                .iter()
                .filter(|record| record.kind != "tool")
                .all(|record| record.tool_status.is_empty())
        );
    }

    #[test]
    fn bounded_reader_warns_at_event_limit_and_partial_row() {
        let header =
            json!({"type":"session_meta","payload":{"id":ROOT_ID,"cwd":"/synthetic/agentwatcher"}})
                .to_string();
        let input = format!(
            "{header}\n{}",
            "{\"type\":\"world_state\"}\n".repeat(SESSION_EVENTS)
        );
        let snapshot = parse_jsonl(Cursor::new(input.as_bytes()), &session("codex"), false);
        assert!(
            snapshot
                .warnings
                .iter()
                .any(|warning| warning.contains("20,000"))
        );
        let partial = format!("{header}\n{{\"type\":");
        let snapshot = parse_jsonl(Cursor::new(partial.as_bytes()), &session("codex"), false);
        assert!(
            snapshot
                .warnings
                .iter()
                .any(|warning| warning.contains("malformed or incomplete"))
        );
    }

    fn rollover_fixture() -> (Vec<Value>, Vec<Value>) {
        let mut earlier = vec![
            json!({"type":"session_meta","payload":{"id":ROOT_ID,"cwd":"/synthetic/agentwatcher","parent_thread_id":CHILD_ID}}),
        ];
        earlier.push(json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"text":"Old prefix task"}]}}));
        earlier.extend((0..40).map(|index| json!({"type":"event_msg","payload":{"type":"old_event","index":index,"padding":"old context repeated to push the byte window forward"}})));
        let latest = vec![
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"text":"Latest task beyond the limit"}]}}),
            json!({"type":"turn_context","payload":{"model":"newest-model"}}),
            json!({"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":31,"output_tokens":11,"total_tokens":42}}}}),
            json!({"type":"event_msg","payload":{"type":"agent_message","message":"Latest completion sentinel"}}),
        ];
        (earlier, latest)
    }

    fn assert_rollover_snapshot(snapshot: &Snapshot) {
        assert!(snapshot.error.is_none(), "{:?}", snapshot.error);
        assert_eq!(snapshot.session.id, ROOT_ID);
        assert_eq!(snapshot.session.parent_id, CHILD_ID);
        assert_eq!(snapshot.session.title, "Latest task beyond the limit");
        assert_eq!(snapshot.model, "newest-model");
        assert_eq!(snapshot.usage.as_ref().unwrap().total, 42);
        assert!(
            snapshot
                .records
                .iter()
                .any(|record| record.summary == "Latest completion sentinel")
        );
        assert!(
            !snapshot
                .records
                .iter()
                .any(|record| record.summary == "Old prefix task")
        );
        assert!(
            snapshot
                .warnings
                .iter()
                .any(|warning| warning.contains("Earlier history omitted"))
        );
        assert!(
            snapshot
                .records
                .iter()
                .find(|record| record.id == "codex-0")
                .unwrap()
                .detail
                .contains(ROOT_ID)
        );
    }

    #[test]
    fn newest_events_after_event_cap_keep_task_usage_completion_and_header() {
        let (mut events, latest) = rollover_fixture();
        events.extend(latest);
        let recording = events
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        let limits = ParseLimits {
            bytes: 16_384,
            events: 5,
            header_bytes: 512,
        };
        let snapshot = parse_jsonl_with_limits(
            Cursor::new(recording.as_bytes()),
            &session("codex"),
            false,
            limits,
        );
        assert_rollover_snapshot(&snapshot);
        assert_eq!(snapshot.records.len(), limits.events);
    }

    #[test]
    fn latest_window_does_not_pair_an_omitted_call_with_a_similar_id() {
        let (mut events, _) = rollover_fixture();
        events.insert(1, json!({"type":"response_item","payload":{"type":"function_call","call_id":"call-a","name":"omitted_tool","arguments":{}}}));
        events.extend([
            json!({"type":"response_item","payload":{"type":"function_call","call_id":"call-ab","name":"retained_tool","arguments":{}}}),
            json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"call-a","output":"Orphan result sentinel"}}),
            json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"call-ab","output":"Paired result sentinel"}}),
        ]);
        let recording = events
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        let limits = ParseLimits {
            bytes: 16_384,
            events: 4,
            header_bytes: 512,
        };
        let snapshot = parse_jsonl_with_limits(
            Cursor::new(recording.as_bytes()),
            &session("codex"),
            false,
            limits,
        );
        assert!(snapshot.error.is_none());
        let orphan = snapshot
            .records
            .iter()
            .find(|record| record.id == "call-a")
            .unwrap();
        let paired = snapshot
            .records
            .iter()
            .find(|record| record.id == "call-ab")
            .unwrap();
        assert!(orphan.summary.contains("Call not recorded"));
        assert!(!orphan.detail.contains("omitted_tool"));
        assert!(paired.summary.contains("retained_tool · Completed"));
        assert!(paired.detail.contains("Paired result sentinel"));
        assert!(!paired.detail.contains("Orphan result sentinel"));
        assert!(
            snapshot
                .warnings
                .iter()
                .any(|warning| warning.contains("no corresponding recorded call"))
        );
    }

    struct Metered<R> {
        inner: R,
        bytes: Rc<Cell<usize>>,
    }

    impl<R: Read> Read for Metered<R> {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            let count = self.inner.read(output)?;
            self.bytes.set(self.bytes.get() + count);
            Ok(count)
        }
    }

    impl<R: Seek> Seek for Metered<R> {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(position)
        }
    }

    #[test]
    fn newest_bytes_after_byte_cap_keep_latest_records_with_aggregate_read_bound() {
        let (mut events, latest) = rollover_fixture();
        events.extend(latest);
        let recording = events
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        let limits = ParseLimits {
            bytes: 2_048,
            events: 100,
            header_bytes: 512,
        };
        assert!(recording.len() > limits.bytes);
        let bytes = Rc::new(Cell::new(0));
        let reader = Metered {
            inner: Cursor::new(recording.as_bytes()),
            bytes: bytes.clone(),
        };
        let snapshot = parse_jsonl_with_limits(reader, &session("codex"), false, limits);
        assert_rollover_snapshot(&snapshot);
        assert!(
            bytes.get() <= limits.bytes,
            "Read {} bytes with cap {}",
            bytes.get(),
            limits.bytes
        );

        // Appending must advance the same retained window on the next refresh.
        let appended = format!(
            "{recording}\n{}",
            json!({"type":"event_msg","payload":{"type":"agent_message","message":"Newest refresh sentinel"}})
        );
        let refreshed = parse_jsonl_with_limits(
            Cursor::new(appended.as_bytes()),
            &session("codex"),
            false,
            limits,
        );
        assert!(
            refreshed
                .records
                .iter()
                .any(|record| record.summary == "Newest refresh sentinel")
        );
        let previous = snapshot
            .records
            .iter()
            .find(|record| record.summary == "Latest completion sentinel")
            .unwrap();
        let current = refreshed
            .records
            .iter()
            .find(|record| record.summary == "Latest completion sentinel")
            .unwrap();
        assert_eq!(previous.id, current.id);
    }

    #[test]
    fn catalog_budget_cannot_exceed_headers_or_bytes() {
        let scratch = Scratch::new();
        let path = scratch.0.join("synthetic-header.jsonl");
        fs::write(&path, vec![b'x'; HEADER_BYTES * 2]).unwrap();
        let mut budget = CatalogBudget::new();
        for _ in 0..CATALOG_HEADERS {
            // Isolate byte/header counters from scheduler delays in this test.
            budget.start = Instant::now();
            assert_eq!(budget.prefix(&path).unwrap().len(), HEADER_BYTES);
        }
        assert_eq!(budget.bytes, CATALOG_BYTES);
        assert!(budget.prefix(&path).is_none());
        budget.headers = 0;
        assert!(!budget.available());
        budget.bytes = 0;
        budget.start = Instant::now() - Duration::from_secs(2);
        assert!(budget.prefix(&path).is_none());
        assert_eq!(civil_date(0), (1970, 1, 1));
    }

    #[test]
    fn catalog_filters_workspace_and_only_inspects_small_prefix() {
        let scratch = Scratch::new();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            / 86_400;
        let (year, month, day) = civil_date(now);
        let directory = scratch
            .0
            .join(format!(".codex/sessions/{year:04}/{month:02}/{day:02}"));
        fs::create_dir_all(&directory).unwrap();
        let header =
            json!({"type":"session_meta","payload":{"id":ROOT_ID,"cwd":"/synthetic/agentwatcher"}});
        let early = json!({"type":"response_item","payload":{"role":"user","content":[{"text":"Bounded prefix task"}]}});
        let late = json!({"type":"response_item","payload":{"role":"user","content":[{"text":"Beyond catalog byte limit"}]}});
        let fixture = format!("{header}\n{early}\n{}\n{late}\n", " ".repeat(HEADER_BYTES));
        fs::write(
            directory.join(format!("rollout-test-{ROOT_ID}.jsonl")),
            fixture,
        )
        .unwrap();
        fs::write(
            directory.join(format!("rollout-test-{CHILD_ID}.jsonl")),
            json!({"type":"session_meta","payload":{"id":CHILD_ID,"cwd":"/other/project"}})
                .to_string(),
        )
        .unwrap();
        let sessions = discover_in("/synthetic/agentwatcher", "automatic", &scratch.0);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, ROOT_ID);
        assert_eq!(sessions[0].title, "Bounded prefix task");
    }

    #[test]
    fn projection_cache_prefix_can_end_inside_unrelated_message_rows() {
        let prefix = br#"{"record":{"identity":{"cwd":"/synthetic/agentwatcher"},"rows":{"title":{"val":"Bounded cache title"},"messages":[{"text":"incomplete"#;
        assert!(serde_json::from_slice::<Value>(prefix).is_err());
        assert_eq!(
            prefix_json_path(prefix, &["record", "identity", "cwd"]),
            Some(json!("/synthetic/agentwatcher"))
        );
        assert_eq!(
            prefix_json_path(prefix, &["record", "rows", "title", "val"]),
            Some(json!("Bounded cache title"))
        );
        assert!(prefix_json_path(prefix, &["record", "rows", "messages"]).is_none());
    }

    fn dsh_fixture(home: &Path, id: &str, project: &str, parent: &str) {
        let entry = home.join(".dsh/sessions/encoded-project").join(id);
        fs::create_dir_all(&entry).unwrap();
        let header = json!({"type":"session","version":4,"id":id,"createdAt":1,"cwd":project,
            "isSeeded":false,"delegationDepth":if parent.is_empty(){0}else{1},"agentPreset":"standard"});
        fs::write(entry.join("session.v4.jsonl"), format!("{header}\n")).unwrap();
        let cache = home.join(".dsh/storages/session_projcache/sessions");
        fs::create_dir_all(&cache).unwrap();
        // Mirrors the observed installed DSH projection-cache schema; no real
        // prompt, environment, token or user record is used by these fixtures.
        fs::write(cache.join(format!("{id}.json")), json!({"version":1,"record":{
            "identity":{"formatVersion":4,"createdAt":1,"cwd":project,"isSeeded":false,"inheritedEventCount":0},
            "rows":{"title":{"ver":1,"seq":1,"val":"Synthetic DSH task"},
                "agentPreset":{"ver":1,"seq":1,"val":"standard"},
                "subagent":{"ver":1,"seq":1,"val":{"parentSessionId":parent}}}
        }}).to_string()).unwrap();
    }

    #[test]
    fn observed_dsh_cache_schema_preserves_recorded_workspace_and_parent() {
        let scratch = Scratch::new();
        dsh_fixture(&scratch.0, "session-parent", "/synthetic/other-project", "");
        dsh_fixture(
            &scratch.0,
            "session-child",
            "/synthetic/other-project",
            "session-parent",
        );
        let mut budget = CatalogBudget::new();
        let found = deepseek_catalog("", &scratch.0.join(".dsh"), &mut budget);
        assert_eq!(found.len(), 2);
        assert!(
            found
                .iter()
                .all(|session| session.project == "/synthetic/other-project")
        );
        let child = found
            .iter()
            .find(|session| session.id == "session-child")
            .unwrap();
        assert_eq!(child.parent_id, "session-parent");
        assert_eq!(child.agent, "standard");
        assert_eq!(child.title, "Synthetic DSH task");
        let collected = parse_jsonl(File::open(&child.path).unwrap(), child, false);
        assert_eq!(collected.session.parent_id, "session-parent");
        let mut budget = CatalogBudget::new();
        assert!(
            deepseek_catalog(
                "/synthetic/agentwatcher",
                &scratch.0.join(".dsh"),
                &mut budget
            )
            .is_empty()
        );
    }

    #[test]
    fn automatic_recent_discovery_includes_both_local_providers() {
        let scratch = Scratch::new();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            / 86_400;
        let (year, month, day) = civil_date(now);
        let directory = scratch
            .0
            .join(format!(".codex/sessions/{year:04}/{month:02}/{day:02}"));
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join(format!("rollout-test-{ROOT_ID}.jsonl")),
            json!({"type":"session_meta","payload":{"id":ROOT_ID,"cwd":"/synthetic/agentwatcher"}})
                .to_string(),
        )
        .unwrap();
        dsh_fixture(&scratch.0, "session-parent", "/synthetic/agentwatcher", "");
        let sessions = discover_in("/synthetic/agentwatcher", "automatic", &scratch.0);
        assert_eq!(sessions.len(), 2);
        assert!(sessions.iter().any(|session| session.source == "codex"));
        assert!(sessions.iter().any(|session| session.source == "deepseek"));
    }

    #[test]
    fn global_history_visits_newest_dates_and_rollout_names_first() {
        let scratch = Scratch::new();
        let fixtures = [
            ("2020/01/01", "2020-01-01T00-00-00", ROOT_ID),
            ("2026/10/04", "2026-10-04T23-00-00", CHILD_ID),
            (
                "2026/10/14",
                "2026-10-14T01-00-00",
                "33333333-2222-3333-4444-555555555555",
            ),
            (
                "2026/10/14",
                "2026-10-14T22-00-00",
                "44444444-2222-3333-4444-555555555555",
            ),
        ];
        // Create the oldest directories first to reproduce filesystem insertion
        // order hiding the newest agents on the initial global page.
        for (date, timestamp, id) in fixtures {
            let directory = scratch.0.join(".codex/sessions").join(date);
            fs::create_dir_all(&directory).unwrap();
            fs::write(
                directory.join(format!("rollout-{timestamp}-{id}.jsonl")),
                json!({"type":"session_meta","payload":{"id":id,"cwd":"/synthetic/history-project"}}).to_string(),
            ).unwrap();
        }
        let mut state = HistoryState::new(&scratch.0, "", "codex");
        for (_, _, id) in fixtures.into_iter().rev() {
            let page = state.page(1);
            assert_eq!(page.sessions.len(), 1);
            assert_eq!(page.sessions[0].id, id);
            assert_eq!(page.sessions[0].project, "/synthetic/history-project");
        }
        let final_page = state.page(1);
        assert!(final_page.sessions.is_empty());
        assert!(final_page.complete);
    }

    #[test]
    fn history_directory_batches_resume_inventory_without_losing_entries() {
        let scratch = Scratch::new();
        for index in 0..11 {
            fs::write(scratch.0.join(format!("entry-{index:03}")), []).unwrap();
        }
        let mut frame = HistoryFrame::new(fs::read_dir(&scratch.0).unwrap(), 0);
        frame.batch_limit = 4;
        let mut found = HashSet::new();
        let mut batch = Vec::<PathBuf>::new();
        let mut batches = Vec::new();
        let mut consumed = 0;
        for _ in 0..100 {
            let mut remaining = 2;
            let next = frame.next(&CatalogBudget::new(), &mut remaining);
            consumed += 2 - remaining;
            assert!(frame.pending.len() + frame.ready.len() <= frame.batch_limit);
            if let Some(path) = next {
                assert!(found.insert(path.clone()), "duplicate catalog candidate");
                batch.push(path);
                if frame.ready.is_empty() {
                    assert!(
                        batch
                            .windows(2)
                            .all(|pair| pair[0].file_name() >= pair[1].file_name())
                    );
                    batches.push(batch.len());
                    batch.clear();
                }
            }
            if frame.exhausted() {
                break;
            }
        }
        assert!(frame.exhausted());
        assert_eq!(consumed, 11);
        assert_eq!(found.len(), 11);
        assert_eq!(batches, [4, 4, 3]);
    }

    #[test]
    fn global_history_pages_reach_old_sessions_beyond_catalog_header_limit() {
        let scratch = Scratch::new();
        let directory = scratch.0.join(".codex/sessions/2020/01/01");
        fs::create_dir_all(&directory).unwrap();
        for index in 0..270 {
            let id = format!("{index:08x}-2222-3333-4444-555555555555");
            fs::write(
                directory.join(format!("rollout-test-{id}.jsonl")),
                json!({"type":"session_meta","payload":{"id":id,"cwd":"/synthetic/old-project"}})
                    .to_string(),
            )
            .unwrap();
        }
        dsh_fixture(&scratch.0, "session-parent", "/synthetic/other-project", "");
        dsh_fixture(
            &scratch.0,
            "session-child",
            "/synthetic/other-project",
            "session-parent",
        );
        let mut state = HistoryState::new(&scratch.0, "", "automatic");
        let first = state.page(64);
        assert!(
            first
                .sessions
                .iter()
                .any(|session| session.source == "deepseek")
        );
        assert!(
            first
                .sessions
                .iter()
                .any(|session| session.source == "codex")
        );
        assert!(!first.complete);
        let mut found = first.sessions;
        for _ in 0..10 {
            let page = state.page(64);
            assert!(page.sessions.len() <= 64);
            found.extend(page.sessions);
            if page.complete {
                break;
            }
        }
        assert!(state.walkers.is_empty());
        let identities = found
            .iter()
            .map(|session| (&session.source, &session.id))
            .collect::<HashSet<_>>();
        assert_eq!(identities.len(), 272);
        assert_eq!(found.len(), 272);
        assert!(
            found
                .iter()
                .filter(|session| session.source == "codex")
                .all(|session| session.project == "/synthetic/old-project")
        );
        assert!(discover_in("/synthetic/old-project", "codex", &scratch.0).is_empty());
    }

    fn assert_trace_paging(limits: ParseLimits) {
        let header =
            json!({"type":"session_meta","payload":{"id":ROOT_ID,"cwd":"/synthetic/agentwatcher"}});
        let mut events = vec![header];
        events.extend((0..45).map(|index| json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"text":format!("trace-{index:03}")}]}})));
        let recording = events
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        let mut before = None;
        let mut found = HashSet::new();
        let mut complete = false;
        for _ in 0..60 {
            let bytes = Rc::new(Cell::new(0));
            let reader = Metered {
                inner: Cursor::new(recording.as_bytes()),
                bytes: bytes.clone(),
            };
            let snapshot = parse_jsonl_window(reader, &session("codex"), false, limits, before);
            assert!(snapshot.error.is_none(), "{:?}", snapshot.error);
            assert_eq!(snapshot.session.id, ROOT_ID);
            assert!(
                snapshot
                    .records
                    .iter()
                    .find(|record| record.id == "codex-0")
                    .unwrap()
                    .detail
                    .contains(ROOT_ID)
            );
            assert!(bytes.get() <= limits.bytes);
            assert_eq!(
                snapshot.window_end,
                before.unwrap_or(recording.len() as u64)
            );
            assert!(snapshot.window_start < snapshot.window_end);
            for record in snapshot
                .records
                .iter()
                .filter(|record| record.summary.starts_with("trace-"))
            {
                assert!(
                    found.insert(record.summary.clone()),
                    "duplicate trace row {}",
                    record.summary
                );
            }
            if !snapshot.has_earlier {
                complete = true;
                break;
            }
            if let Some(previous) = before {
                assert!(snapshot.window_start < previous);
            }
            before = Some(snapshot.window_start);
        }
        assert!(complete);
        assert_eq!(found.len(), 45);
        assert!(found.contains("trace-000"));
        assert!(found.contains("trace-044"));
    }

    #[test]
    fn older_event_pages_reach_every_retained_event_without_duplicates() {
        assert_trace_paging(ParseLimits {
            bytes: 16_384,
            events: 5,
            header_bytes: 256,
        });
    }

    #[test]
    fn older_byte_pages_reach_every_retained_event_with_bounded_reads() {
        assert_trace_paging(ParseLimits {
            bytes: 1_024,
            events: 100,
            header_bytes: 256,
        });
    }

    #[test]
    fn older_window_still_validates_the_original_session_header() {
        let (mut events, latest) = rollover_fixture();
        events.extend(latest);
        let recording = events
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        let mut requested = session("codex");
        requested.id = CHILD_ID.into();
        let snapshot = parse_jsonl_window(
            Cursor::new(recording.as_bytes()),
            &requested,
            false,
            ParseLimits {
                bytes: 1_024,
                events: 5,
                header_bytes: 512,
            },
            Some(recording.len() as u64 / 2),
        );
        assert!(snapshot.error.is_some());
        assert!(snapshot.records.is_empty());
    }

    #[test]
    fn decoded_stream_retains_only_its_window_and_enforces_header_and_work_caps() {
        let limits = DecodeLimits {
            parse: ParseLimits {
                bytes: 2_048,
                events: 20,
                header_bytes: 512,
            },
            ..DecodeLimits::default()
        };
        let header = json!({"type":"session","version":4,"id":"session-root","cwd":"/synthetic/agentwatcher"});
        let mut window = DecodedWindow::new(None, limits).unwrap();
        window
            .feed(format!("{header}\n").as_bytes(), &session("deepseek"))
            .unwrap();
        let block = [b' '; 8192];
        for _ in 0..(17 * 1024 * 1024 / block.len()) {
            window.feed(&block, &session("deepseek")).unwrap();
        }
        let latest = json!({"type":"assistant/message","data":{"message":{"id":"last","role":"assistant","content":[{"text":"Latest streamed message"}]}}});
        window
            .feed(format!("\n{latest}\n").as_bytes(), &session("deepseek"))
            .unwrap();
        assert!(window.position > SESSION_BYTES as u64);
        assert!(window.header.len() + window.tail.len() <= limits.parse.bytes);
        assert!(window.header.capacity() + window.tail.capacity() <= limits.parse.bytes * 2);
        let snapshot = parse_jsonl_window(
            window.into_reader().unwrap(),
            &session("deepseek"),
            false,
            limits.parse,
            None,
        );
        assert!(snapshot.error.is_none());
        assert!(
            snapshot
                .records
                .iter()
                .any(|record| record.summary == "Latest streamed message")
        );

        let small = DecodeLimits {
            decoded_bytes: 1_024,
            ..limits
        };
        let mut capped = DecodedWindow::new(None, small).unwrap();
        capped
            .feed(format!("{header}\n").as_bytes(), &session("deepseek"))
            .unwrap();
        assert!(
            capped
                .feed(&block, &session("deepseek"))
                .unwrap_err()
                .contains("decoded-work limit")
        );
        assert_eq!(capped.position, small.decoded_bytes);
        let mut wrong = session("deepseek");
        wrong.project = "/wrong/workspace".into();
        let mut window = DecodedWindow::new(None, limits).unwrap();
        assert!(
            window
                .feed(format!("{header}\n").as_bytes(), &wrong)
                .is_err()
        );
        assert!(window.tail.is_empty());
    }

    #[test]
    fn completed_decoder_survives_a_delayed_parent_without_bypassing_work_caps() {
        let scratch = Scratch::new();
        let decoder = scratch.0.join("completed-decoder.sh");
        let header = json!({"type":"session","version":4,"id":"session-root","cwd":"/synthetic/agentwatcher"});
        let event = json!({"type":"assistant/message","data":{"message":{"id":"finished","role":"assistant","content":[{"text":"Completed before delayed parent"}]}}});
        let recording = format!("{header}\n{event}\n");
        fs::write(
            &decoder,
            format!("#!/bin/sh\nprintf '%s\\n' '{header}' '{event}'\n"),
        )
        .unwrap();
        fs::set_permissions(&decoder, fs::Permissions::from_mode(0o700)).unwrap();
        // Hold the child's waitable exit before entering the pump, reproducing
        // a parent that is resumed only after its deadline has elapsed.
        for capped in [false, true] {
            let pipes = spawn_decoder(
                decoder.to_str().unwrap(),
                Path::new("unused-synthetic.zstd"),
            )
            .unwrap();
            let pid = pipes.process.child.as_ref().unwrap().id();
            let startup_deadline = Instant::now() + Duration::from_secs(8);
            while !decoder_exited(pid).unwrap() {
                assert!(Instant::now() < startup_deadline, "fixture failed to exit");
                std::thread::sleep(Duration::from_millis(5));
            }
            let limits = DecodeLimits {
                decoded_bytes: if capped {
                    recording.len() as u64 - 1
                } else {
                    DECODED_BYTES
                },
                ..DecodeLimits::default()
            };
            let result = drive_decoder(
                pipes,
                DecodedWindow::new(None, limits).unwrap(),
                &session("deepseek"),
                Instant::now() - Duration::from_millis(1),
                Vec::new(),
            );
            if capped {
                let error = result
                    .err()
                    .expect("completed output must still enforce its cap");
                assert!(error.contains("decoded-work limit"), "{error}");
            } else {
                let reader =
                    result.unwrap_or_else(|error| panic!("finished decoder was rejected: {error}"));
                let snapshot =
                    parse_jsonl_window(reader, &session("deepseek"), false, limits.parse, None);
                assert!(snapshot.error.is_none(), "{:?}", snapshot.error);
                assert!(
                    snapshot
                        .records
                        .iter()
                        .any(|record| record.summary == "Completed before delayed parent")
                );
            }
        }
    }

    #[test]
    fn decoder_timeout_stops_its_pipe_holding_descendant() {
        let scratch = Scratch::new();
        let decoder = scratch.0.join("synthetic-decoder.sh");
        let header = json!({"type":"session","version":4,"id":"session-root","cwd":"/synthetic/agentwatcher"});
        fs::write(&decoder, format!("#!/bin/sh\nprintf '%s\\n' '{header}'\n/bin/sleep 30 &\nprintf 'fixture_child=%s\\n' \"$!\" >&2\nprintf 'fixture_pump_marker\\n' >&2\nwait\n")).unwrap();
        fs::set_permissions(&decoder, fs::Permissions::from_mode(0o700)).unwrap();
        let mut pipes = spawn_decoder(
            decoder.to_str().unwrap(),
            Path::new("unused-synthetic.zstd"),
        )
        .unwrap();
        let startup_deadline = Instant::now() + Duration::from_secs(8);
        let mut diagnostic = Vec::new();
        let mut closed = false;
        let child = loop {
            // Stop at the child PID's newline, leaving the marker for the pump.
            let maximum = 8192usize.saturating_sub(diagnostic.len()).min(1);
            decoder_drain(&mut pipes.stderr, &mut closed, maximum, |bytes| {
                diagnostic.extend_from_slice(bytes);
                Ok(diagnostic.contains(&b'\n'))
            })
            .unwrap();
            if let Some(pid) = String::from_utf8_lossy(&diagnostic)
                .strip_suffix('\n')
                .and_then(|line| line.strip_prefix("fixture_child="))
                .and_then(|pid| pid.parse::<i32>().ok())
            {
                break pid;
            }
            assert!(
                !closed && diagnostic.len() < 8192 && Instant::now() < startup_deadline,
                "fixture startup failed before the child handshake: {}",
                String::from_utf8_lossy(&diagnostic)
            );
            let mut descriptor = libc::pollfd {
                fd: pipes.stderr.as_raw_fd(),
                events: libc::POLLIN | libc::POLLHUP,
                revents: 0,
            };
            unsafe {
                libc::poll(&mut descriptor, 1, 10);
            }
        };
        assert!(
            fixture_child_running(child).unwrap(),
            "fixture child must be alive before the timeout check"
        );
        // Queue the marker before the short pump timeout without consuming it.
        let mut descriptor = libc::pollfd {
            fd: pipes.stderr.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        loop {
            let ready = unsafe { libc::poll(&mut descriptor, 1, 10) };
            if ready > 0 && descriptor.revents & libc::POLLIN != 0 {
                break;
            }
            assert!(
                (ready == 0
                    || ready == -1
                        && std::io::Error::last_os_error().kind()
                            == std::io::ErrorKind::Interrupted)
                    && Instant::now() < startup_deadline,
                "fixture failed to emit its stderr marker"
            );
        }
        // Establish the fixture first. The production wrapper still starts its
        // unchanged eight-second deadline before spawn; this test isolates the
        // same pump's timeout and group cleanup from cold interpreter startup.
        let limits = DecodeLimits {
            timeout: Duration::from_millis(250),
            ..DecodeLimits::default()
        };
        let start = Instant::now();
        let error = drive_decoder(
            pipes,
            DecodedWindow::new(None, limits).unwrap(),
            &session("deepseek"),
            start + limits.timeout,
            diagnostic,
        )
        .err()
        .expect("the fixture must time out");
        assert!(error.contains("timed out"), "{error}");
        assert!(
            error.contains("fixture_pump_marker"),
            "stderr left for the decoder pump was not drained: {error}"
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(
            error
                .lines()
                .any(|line| line == format!("fixture_child={child}")),
            "bounded fixture diagnostics were lost: {error}"
        );
        assert!(
            !fixture_child_running(child).unwrap(),
            "decoder descendant is still running"
        );
    }

    fn fixture_child_running(pid: i32) -> Result<bool, String> {
        if pid <= 0 {
            return Err("Invalid owned fixture child PID".into());
        }
        if unsafe { libc::kill(pid, 0) } == -1 {
            let error = std::io::Error::last_os_error();
            return if error.raw_os_error() == Some(libc::ESRCH) {
                Ok(false)
            } else {
                Err(format!("Owned child inspection blocked: {error}"))
            };
        }
        #[cfg(target_os = "macos")]
        unsafe {
            let mut info: libc::proc_bsdinfo = std::mem::zeroed();
            let size = std::mem::size_of_val(&info) as i32;
            let count = libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                &mut info as *mut _ as *mut _,
                size,
            );
            if count == 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                return Ok(false);
            }
            if count != size || info.pbi_uid != libc::getuid() {
                return Err("Native inspection of the owned fixture child is unavailable".into());
            }
            Ok(info.pbi_status != libc::SZOMB)
        }
        #[cfg(not(target_os = "macos"))]
        {
            match fs::read_to_string(format!("/proc/{pid}/stat")) {
                Ok(stat) => Ok(stat
                    .rsplit_once(')')
                    .and_then(|(_, rest)| rest.split_whitespace().next())
                    != Some("Z")),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(error) => Err(format!("Owned child status inspection blocked: {error}")),
            }
        }
    }

    // Synthetic 1,079-byte zstd frame: a v4 header, earlier user/tool rows,
    // 17 MiB of ignorable JSONL whitespace, and latest user/tool/assistant rows.
    // No decoded fixture file is created or spooled.
    const LARGE_COMPRESSED_FIXTURE: &str = concat!(
        "28b52ffd0458bc07003691311e70b7cd01ed18a89b24e365adeaa28d4c1c140a69dc40255ab2c8a85e1c1c2900270028",
        "000aa1398f407b8a6547ee406a8f58b6d0c3c749816cbc885948ca06e0e40d6ccc50a95e7dfcc462bd046ae5197cbc07",
        "5f2f03c7cbbe0c914d4b4c1578393e4a05d5c60bd3cbf1ed251b1f39d527475e03c74f8ed978216e61b1f2eee324b0d6",
        "a1b50e1d0b71dba1799da96557e3e8728d74d567224164e40306a1b5765d47709c986950657368c642dc80e5ec52f4d9",
        "9c14894b12445e8e75d91305c9b331fac11d10008ffcf1dcb270356c61d0f197600debf642197119636aeb9880013367",
        "09c6c03bc9322819b042d13e176a5bc40200102002001020020010200200102002001020020010200200102002001020",
        "020010200200102002001020020010200200102002001020020010200200102002001020020010200200102002001020",
        "020010200200102002001020020010200200102002001020020010200200102002001020020010200200102002001020",
        "020010200200102002001020020010200200102002001020020010200200102002001020020010200200102002001020",
        "020010200200102002001020020010200200102002001020020010200200102002001020020010200200102002001020",
        "02001020020010200200102002001020020010200200102002001020440000000100fdffcd0b10020010200200102002",
        "001020020010200200102002001020020010200200102002001020020010200200102002001020020010200200102002",
        "001020020010200200102002001020020010200200102002001020020010200200102002001020020010200200102002",
        "001020020010200200102002001020020010200200102002001020020010200200102002001020020010200200102002",
        "001020020010200200102002001020020010200200102002001020020010200200102002001020020010200200102002",
        "001020020010200200102002001020020010200200102002001020020010200200102002001020020010200200102002",
        "001020440000000100fdffcd0b1002001020020010200200102002001020020010200200102002001020350800161131",
        "1e6069da0684f8dfa299493764f9b31d631a185e2639c200c6800940a0e1782700270029006707f74ed969cd4b3b0949",
        "9cddd3a8b5fc725d7576c7c6f3f2442073a2834a5ebf6792d0670523f97b224c2fd7dd36e8ef8740158d2a9a83ee766b",
        "477b82b18d93cca912b4c668cec682df7b911d099b11664c5644b8ab556a809197a319badb02ec5a17a76a3f8a04115a",
        "7376e74e891642e1cb4d2e0285d61a40af7d09b873cac41af44a127276eba51ded69914e4dd09af26654834a0527fcfd",
        "2067d701170068acb5a6bb6a6440e52f10a141bf45b8860060941c843f218becb1665b1dca0a767b6dc49a611432a6c6",
        "c4a41471bb1de6441e36bc932c06457cac0aae99cf2065",
    );

    #[test]
    fn header_only_stream_stops_before_later_output_and_preserves_identity_checks() {
        let expected = Session {
            id: "session-root".into(),
            source: "deepseek".into(),
            ..Session::default()
        };
        let limits = DecodeLimits {
            parse: ParseLimits {
                bytes: 514,
                events: 1,
                header_bytes: 512,
            },
            decoded_bytes: 512,
            allow_global_header: true,
            ..DecodeLimits::default()
        };
        let header = json!({"type":"session","version":4,"id":"session-root","cwd":"/synthetic/agentwatcher"});
        let bytes = format!("{header}\n{}", "irrelevant later output".repeat(2048));
        let mut window = DecodedWindow::new(Some(0), limits).unwrap();
        assert!(window.feed(bytes.as_bytes(), &expected).unwrap());
        assert!(window.tail.is_empty());
        let reader = window.into_reader().unwrap();
        assert_eq!(first_json_line(&reader.header).unwrap(), header);
        assert!(reader.length <= limits.parse.header_bytes as u64);
        let mut wrong = expected.clone();
        wrong.id = "session-other".into();
        assert!(
            DecodedWindow::new(Some(0), limits)
                .unwrap()
                .feed(bytes.as_bytes(), &wrong)
                .is_err()
        );
        let unknown_workspace = json!({"type":"session","version":4,"id":"session-root","cwd":""});
        assert!(
            DecodedWindow::new(Some(0), limits)
                .unwrap()
                .feed(format!("{unknown_workspace}\n").as_bytes(), &expected)
                .is_err()
        );
    }

    #[test]
    #[ignore = "requires an installed zstd decoder; run explicitly with --ignored"]
    fn compressed_catalog_discovers_recording_with_more_than_32kib_decoded_output() {
        zstd_executable()
            .expect("blocked: no installed zstd decoder for compressed catalog verification");
        let scratch = Scratch::new();
        let home = scratch.0.join(".dsh");
        let entry = home.join("sessions/encoded-project/session-root");
        fs::create_dir_all(&entry).unwrap();
        let bytes = LARGE_COMPRESSED_FIXTURE
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect::<Vec<_>>();
        fs::write(entry.join("session.v4.jsonl.zstd"), bytes).unwrap();
        // This fixture expands to 17.8 MiB. Discovery must stop at its valid
        // header, regardless of how command overflow errors are presented.
        let mut budget = CatalogBudget::new();
        let descriptor = deepseek_descriptor(&entry, &home, "", &mut budget)
            .expect("large decoded recording must remain discoverable");
        assert_eq!(descriptor.id, "session-root");
        assert_eq!(descriptor.project, "/synthetic/agentwatcher");
        assert_eq!(descriptor.source, "deepseek");
        assert_eq!(budget.headers, 1);
        assert!(budget.bytes <= HEADER_BYTES);
        let mut budget = CatalogBudget::new();
        let catalog = deepseek_catalog("/synthetic/agentwatcher", &home, &mut budget);
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog[0].id, descriptor.id);
        let page = HistoryState::new(&scratch.0, "", "automatic").page(64);
        assert_eq!(page.sessions.len(), 1);
        assert_eq!(page.sessions[0].project, "/synthetic/agentwatcher");
    }

    #[test]
    #[ignore = "requires an installed zstd decoder; run explicitly with --ignored"]
    fn compressed_recording_beyond_16mib_supports_latest_and_backward_trace_windows() {
        let decoder = zstd_executable()
            .expect("blocked: no installed zstd decoder for compressed fixture verification");
        let scratch = Scratch::new();
        let path = scratch.0.join("synthetic-large-session.v4.jsonl.zstd");
        let bytes = LARGE_COMPRESSED_FIXTURE
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(bytes.len(), 1079);
        fs::write(&path, bytes).unwrap();
        let selected = session("deepseek");
        let mut snapshot =
            collect_compressed(&decoder, &path, &selected, None, DecodeLimits::default());
        assert!(snapshot.error.is_none(), "{:?}", snapshot.error);
        assert_eq!(snapshot.window_end, 17_826_757);
        assert!(snapshot.has_earlier);
        assert_eq!(snapshot.session.title, "Latest compressed task");
        assert_eq!(snapshot.usage.as_ref().unwrap().total, 8);
        assert_eq!(
            snapshot
                .records
                .iter()
                .find(|record| record.id == "latest-tool")
                .unwrap()
                .tool_status,
            "Completed"
        );
        assert!(
            snapshot
                .records
                .iter()
                .any(|record| record.summary == "Latest compressed completion")
        );
        let mut earlier_message = false;
        let mut earlier_tool = false;
        for _ in 0..6 {
            if !snapshot.has_earlier {
                break;
            }
            let boundary = snapshot.window_start;
            snapshot = collect_compressed(
                &decoder,
                &path,
                &selected,
                Some(boundary),
                DecodeLimits::default(),
            );
            assert!(snapshot.error.is_none(), "{:?}", snapshot.error);
            assert_eq!(snapshot.window_end, boundary);
            assert!(snapshot.window_start < boundary);
            assert_eq!(snapshot.session.id, "session-root");
            assert!(
                snapshot.records.iter().any(
                    |record| record.kind == "runtime" && record.detail.contains("session-root")
                )
            );
            earlier_message |= snapshot
                .records
                .iter()
                .any(|record| record.summary == "Earlier compressed trace sentinel");
            earlier_tool |= snapshot
                .records
                .iter()
                .any(|record| record.id == "early-tool" && record.tool_status == "Completed");
        }
        assert!(!snapshot.has_earlier);
        assert!(earlier_message && earlier_tool);
        let capped = collect_compressed(
            &decoder,
            &path,
            &selected,
            None,
            DecodeLimits {
                decoded_bytes: 1_024,
                ..DecodeLimits::default()
            },
        );
        assert!(
            capped
                .error
                .as_ref()
                .unwrap()
                .contains("decoded-work limit")
        );
        assert!(capped.records.is_empty());
    }
}
