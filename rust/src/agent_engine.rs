//! One explicit, app-owned Codex execution at a time. No history process is ever
//! adopted, resumed, signalled, or used as a provider-native parent.
//!
//! `start` only admits work; all filesystem checks and subprocess work happen on
//! the worker. Call it only from the user's explicit release-work action, after
//! displaying the per-run financial acknowledgement represented by the request.

use crate::telemetry::{self, Record, Usage};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const CODEX: &str = "/opt/homebrew/bin/codex";
const CODEX_VERSION: &str = "codex-cli 0.162.1";
const MAX_TASK_BYTES: usize = 16 * 1024;
const MAX_RUNS: usize = 16;
const MAX_RECORDS: usize = 256;
const MAX_DETAIL_BYTES: usize = 8 * 1024;
const MAX_LINE_BYTES: usize = 64 * 1024;
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_RETAINED_BYTES: usize = 1024 * 1024;
const PROBE_OUTPUT_BYTES: usize = 8 * 1024;
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const TERM_GRACE: Duration = Duration::from_secs(2);
const POLL_MS: i32 = 20;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
// Multiple Engine instances cannot bypass the application's concurrency bound.
static ACTIVE_RUN: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SandboxMode {
    #[default]
    ReadOnly,
    WorkspaceWrite,
}

impl SandboxMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::ReadOnly => "Read only",
            Self::WorkspaceWrite => "Workspace write",
        }
    }

    fn argument(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
        }
    }
}

/// Application launch lineage only; never sent to Codex as a parent/resume ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParentKey {
    pub source: String,
    pub id: String,
}

#[derive(Clone, Debug)]
pub struct LaunchRequest {
    pub task: String,
    pub title: String,
    pub workspace: PathBuf,
    pub sandbox: SandboxMode,
    pub parent: Option<ParentKey>,
    /// Inclusive range: one through ten minutes; no automatic retry or restart.
    pub timeout: Duration,
    /// The user explicitly accepts possible paid-credit use for THIS run despite
    /// the absence of a verified provider billing cap. This is not a budget.
    pub acknowledge_possible_paid_credit_usage: bool,
}

#[derive(Clone, Debug)]
pub struct LaunchInfo {
    pub launch_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunStatus {
    Starting,
    Running,
    Completed,
    Failed,
    Stopped,
    TimedOut,
}

impl RunStatus {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Starting | Self::Running)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Starting => "Starting",
            Self::Running => "Running",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Stopped => "Stopped",
            Self::TimedOut => "Timed out",
        }
    }
}

#[derive(Clone, Debug)]
pub struct RunSnapshot {
    pub launch_id: String,
    pub title: String,
    /// Redacted display value. The worker alone receives the original request.
    pub task: String,
    pub workspace: PathBuf,
    pub sandbox: SandboxMode,
    pub parent: Option<ParentKey>,
    pub thread_id: Option<String>,
    pub status: RunStatus,
    pub records: Vec<Record>,
    pub usage: Option<Usage>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub error: Option<String>,
    pub truncated: bool,
}

struct StoredRun {
    snapshot: RunSnapshot,
    record_bytes: usize,
    sequence: u64,
}

impl StoredRun {
    fn push(&mut self, mut record: Record) {
        self.sequence = self.sequence.saturating_add(1);
        record.id = format!("{}:{}", self.snapshot.launch_id, self.sequence);
        record.time = timestamp();
        record.summary = bounded(&telemetry::redact(&record.summary), 1024);
        let safe_detail = telemetry::redact(&record.detail);
        if safe_detail.len() > MAX_DETAIL_BYTES {
            self.snapshot.truncated = true;
        }
        record.detail = bounded_detail(&safe_detail);
        record.tool_status = bounded(&telemetry::redact(&record.tool_status), 64);
        self.record_bytes += record_size(&record);
        self.snapshot.records.push(record);
        while self.snapshot.records.len() > MAX_RECORDS || self.record_bytes > MAX_RETAINED_BYTES {
            let removed = self.snapshot.records.remove(0);
            self.record_bytes = self.record_bytes.saturating_sub(record_size(&removed));
            self.snapshot.truncated = true;
        }
    }

    fn note(&mut self, kind: &str, summary: &str) {
        self.push(Record {
            kind: kind.into(),
            summary: summary.into(),
            detail: summary.into(),
            is_infrastructure: true,
            ..Default::default()
        });
    }
}

fn record_size(record: &Record) -> usize {
    record.id.len()
        + record.time.len()
        + record.kind.len()
        + record.summary.len()
        + record.detail.len()
        + record.tool_status.len()
}

struct Pending {
    id: String,
    request: LaunchRequest,
    _admission: Admission,
}

struct State {
    runs: VecDeque<StoredRun>,
    pending: Option<Pending>,
    active: Option<String>,
    stop_requested: bool,
    closed: bool,
    unavailable: Option<String>,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    revision: AtomicU64,
    owned_group: Arc<GroupRegistry>,
}

#[derive(Default)]
struct GroupRegistry {
    pid: Mutex<Option<u32>>,
    force_closed: AtomicBool,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn change(&self, id: &str, operation: impl FnOnce(&mut StoredRun)) {
        let mut state = self.lock();
        if let Some(run) = state
            .runs
            .iter_mut()
            .find(|run| run.snapshot.launch_id == id)
        {
            operation(run);
            self.revision.fetch_add(1, Ordering::Release);
        }
    }

    fn cancelled(&self, id: &str) -> bool {
        let state = self.lock();
        state.closed || (state.active.as_deref() == Some(id) && state.stop_requested)
    }
}

struct Admission;
impl Admission {
    fn acquire() -> Result<Self, String> {
        ACTIVE_RUN
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| {
                "An AgentWatcher run is already active. Stop it or wait for it to finish.".into()
            })
    }
}
impl Drop for Admission {
    fn drop(&mut self) {
        ACTIVE_RUN.store(false, Ordering::Release);
    }
}

pub struct Engine {
    shared: Arc<Shared>,
}

/// Emergency OS-quit backstop. This handle cannot start, adopt, or target an
/// arbitrary process; it can only shut down the engine that created it.
#[derive(Clone)]
pub struct EngineShutdownHandle {
    shared: Arc<Shared>,
}

impl EngineShutdownHandle {
    /// Synchronously send SIGKILL to the currently owned group, then let its
    /// worker reap. Use normal `shutdown` + `is_idle` for an orderly UI quit.
    /// The shared registry is cleared before reaping, preventing PID reuse.
    pub fn force_shutdown(&self) {
        self.shared
            .owned_group
            .force_closed
            .store(true, Ordering::Release);
        {
            let mut state = self.shared.lock();
            state.closed = true;
            state.stop_requested = true;
            self.shared.wake.notify_one();
        }
        let pid = self
            .shared
            .owned_group
            .pid
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(pid) = *pid {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        Self::create_worker(PathBuf::from(CODEX))
    }

    // Intentionally private. Tests inject their own inert executable without a
    // production environment variable, alternate provider, or hidden backdoor.
    #[cfg(test)]
    fn with_executable(executable: PathBuf) -> Self {
        Self::create_worker(executable)
    }

    fn create_worker(executable: PathBuf) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                runs: VecDeque::new(),
                pending: None,
                active: None,
                stop_requested: false,
                closed: false,
                unavailable: None,
            }),
            wake: Condvar::new(),
            revision: AtomicU64::new(0),
            owned_group: Arc::new(GroupRegistry::default()),
        });
        let worker = Arc::clone(&shared);
        if let Err(error) = std::thread::Builder::new()
            .name("agentwatcher-owned-run".into())
            .spawn(move || worker_loop(worker, executable))
        {
            shared.lock().unavailable = Some(format!("Could not start the run worker: {error}"));
        }
        Self { shared }
    }

    /// Nonblocking admission; filesystem/auth failures appear as a failed run.
    /// This does not resume or send a message to the selected parent recording.
    pub fn start(&self, request: LaunchRequest) -> Result<LaunchInfo, String> {
        validate_request(&request)?;
        let mut state = self.shared.lock();
        if state.closed {
            return Err("The run engine has closed.".into());
        }
        if let Some(error) = &state.unavailable {
            return Err(error.clone());
        }
        if state.active.is_some() {
            return Err("An AgentWatcher run is already active.".into());
        }
        let admission = Admission::acquire()?;
        let id = format!(
            "aw-{}-{}",
            timestamp(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        );
        let mut run = StoredRun {
            snapshot: RunSnapshot {
                launch_id: id.clone(),
                title: bounded(&telemetry::redact(&launch_title(&request)), 512),
                task: bounded(&telemetry::redact(&request.task), MAX_TASK_BYTES),
                workspace: request.workspace.clone(),
                sandbox: request.sandbox,
                parent: request.parent.clone(),
                thread_id: None,
                status: RunStatus::Starting,
                records: Vec::new(),
                usage: None,
                started_at: timestamp(),
                finished_at: None,
                error: None,
                truncated: false,
            },
            record_bytes: 0,
            sequence: 0,
        };
        run.note("runtime", "Checking workspace and ChatGPT sign-in");
        state.runs.push_front(run);
        state.runs.truncate(MAX_RUNS);
        state.active = Some(id.clone());
        state.stop_requested = false;
        state.pending = Some(Pending {
            id: id.clone(),
            request,
            _admission: admission,
        });
        self.shared.revision.fetch_add(1, Ordering::Release);
        self.shared.wake.notify_one();
        Ok(LaunchInfo { launch_id: id })
    }

    pub fn revision(&self) -> u64 {
        self.shared.revision.load(Ordering::Acquire)
    }

    /// Begin orderly application shutdown without blocking GPUI. The host must
    /// keep polling `is_idle` before exiting so TERM/KILL/reap can finish.
    pub fn shutdown(&self) {
        let mut state = self.shared.lock();
        state.closed = true;
        state.stop_requested = true;
        self.shared.wake.notify_one();
    }

    pub fn shutdown_handle(&self) -> EngineShutdownHandle {
        EngineShutdownHandle {
            shared: Arc::clone(&self.shared),
        }
    }

    /// True only after owned subprocess cleanup and global admission release.
    pub fn is_idle(&self) -> bool {
        let state = self.shared.lock();
        state.active.is_none() && state.pending.is_none()
    }

    /// Newest launches first; all display text and evidence are redacted.
    pub fn snapshot(&self) -> Vec<RunSnapshot> {
        self.shared
            .lock()
            .runs
            .iter()
            .map(|run| run.snapshot.clone())
            .collect()
    }

    /// Signals only the process group created by this engine's worker. Pending
    /// starts are cancelled too; no external or recorded PID is ever accepted.
    pub fn stop(&self, launch_id: &str) -> Result<(), String> {
        let mut state = self.shared.lock();
        let Some(run) = state
            .runs
            .iter()
            .find(|run| run.snapshot.launch_id == launch_id)
        else {
            return Err("That launch is not owned by this AgentWatcher engine.".into());
        };
        if !run.snapshot.status.is_active() {
            return Err("That launch has already finished.".into());
        }
        if state.active.as_deref() != Some(launch_id) {
            return Err("That launch is not the active owned run.".into());
        }
        state.stop_requested = true;
        self.shared.wake.notify_one();
        Ok(())
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown();
        // Do not join on the GPUI thread. The worker owns the child RAII guard
        // and completes TERM/KILL/reap before releasing global run admission.
    }
}

fn validate_request(request: &LaunchRequest) -> Result<(), String> {
    if request.task.trim().is_empty()
        || request.task.len() > MAX_TASK_BYTES
        || request.task.contains('\0')
    {
        return Err("Enter a task between 1 byte and 16 KiB, without null characters.".into());
    }
    if request.title.chars().count() > 120 || request.title.contains('\0') {
        return Err("Use a title of at most 120 characters, without null characters.".into());
    }
    if !request.workspace.is_absolute()
        || request.workspace.as_os_str().as_encoded_bytes().len() > 4096
        || request
            .workspace
            .as_os_str()
            .as_encoded_bytes()
            .contains(&0)
    {
        return Err("Choose an absolute workspace folder.".into());
    }
    if !(Duration::from_secs(60)..=Duration::from_secs(600)).contains(&request.timeout) {
        return Err("The run time limit must be between 1 and 10 minutes.".into());
    }
    if let Some(parent) = &request.parent {
        if parent.source.trim().is_empty()
            || parent.source.len() > 64
            || parent.source.chars().any(char::is_control)
            || parent.id.trim().is_empty()
            || parent.id.len() > 512
            || parent.id.chars().any(char::is_control)
        {
            return Err("The selected graph parent has an invalid identity.".into());
        }
    }
    if !request.acknowledge_possible_paid_credit_usage {
        return Err("This run requires explicit acknowledgement of possible paid-credit use without a verified provider billing cap.".into());
    }
    Ok(())
}

fn launch_title(request: &LaunchRequest) -> String {
    if !request.title.trim().is_empty() {
        return request.title.trim().into();
    }
    request
        .task
        .trim()
        .lines()
        .next()
        .unwrap_or("Codex task")
        .chars()
        .take(120)
        .collect()
}

fn validate_workspace(workspace: &Path) -> Result<PathBuf, String> {
    let canonical = workspace
        .canonicalize()
        .map_err(|_| "The workspace folder is unavailable.".to_string())?;
    if !canonical.is_dir() {
        return Err("The workspace must be an existing folder.".into());
    }
    // Inspect both lexical and resolved ancestry so symlinks cannot hide a
    // project configuration that might activate an MCP server or endpoint.
    for root in [workspace, canonical.as_path()] {
        for (depth, ancestor) in root.ancestors().enumerate() {
            if depth > 256 {
                return Err("The workspace ancestry exceeds the supported limit.".into());
            }
            match fs::symlink_metadata(ancestor.join(".codex/config.toml")) {
                Ok(_) => return Err("In-app runs currently require a workspace with no .codex/config.toml in its folder or ancestors. This prevents unverified project MCP servers and configuration from running.".into()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err("Could not verify that the workspace has no project Codex configuration.".into()),
            }
        }
    }
    Ok(canonical)
}

fn worker_loop(shared: Arc<Shared>, executable: PathBuf) {
    loop {
        let pending = {
            let mut state = shared.lock();
            while state.pending.is_none() && !state.closed {
                state = shared
                    .wake
                    .wait(state)
                    .unwrap_or_else(|error| error.into_inner());
            }
            if state.closed && state.pending.is_none() {
                return;
            }
            state.pending.take().expect("one admitted pending run")
        };
        let id = pending.id.clone();
        let result = execute_run(&shared, &executable, &id, &pending.request);
        // Publish terminal state and release admission under the same lock, so
        // observing a finished snapshot means the next explicit run can start.
        let mut state = shared.lock();
        if let Some(run) = state
            .runs
            .iter_mut()
            .find(|run| run.snapshot.launch_id == id)
        {
            run.snapshot.status = result.status;
            run.snapshot.finished_at = Some(timestamp());
            run.snapshot.error = result.error.as_deref().map(telemetry::redact);
            run.note(
                if result.error.is_some() {
                    "error"
                } else {
                    "runtime"
                },
                result.error.as_deref().unwrap_or(result.status.label()),
            );
        }
        // Drop admission before clearing active while holding the state lock;
        // another click can never observe a free slot before child cleanup.
        drop(pending);
        state.active = None;
        state.stop_requested = false;
        shared.revision.fetch_add(1, Ordering::Release);
        if state.closed {
            return;
        }
    }
}

struct RunResult {
    status: RunStatus,
    error: Option<String>,
}

impl RunResult {
    fn failed(error: String) -> Self {
        Self {
            status: RunStatus::Failed,
            error: Some(error),
        }
    }
    fn stopped() -> Self {
        Self {
            status: RunStatus::Stopped,
            error: None,
        }
    }
}

fn execute_run(shared: &Shared, executable: &Path, id: &str, request: &LaunchRequest) -> RunResult {
    if shared.cancelled(id) {
        return RunResult::stopped();
    }
    let workspace = match validate_workspace(&request.workspace) {
        Ok(path) => path,
        Err(error) => return RunResult::failed(error),
    };
    let cancelled = || shared.cancelled(id);
    let version = probe(
        executable,
        &[OsString::from("--version")],
        &workspace,
        &cancelled,
        &shared.owned_group,
    );
    match version {
        Ok(version) if version.trim() == CODEX_VERSION => {}
        Ok(_) => {
            return RunResult::failed(
                "This in-app engine requires the verified Codex CLI 0.162.1 installation.".into(),
            );
        }
        Err(_) if cancelled() => return RunResult::stopped(),
        Err(error) => return RunResult::failed(error),
    }
    let login = probe(
        executable,
        &[OsString::from("login"), OsString::from("status")],
        &workspace,
        &cancelled,
        &shared.owned_group,
    );
    match login {
        Ok(login) if login.trim() == "Logged in using ChatGPT" => {}
        Ok(_) => return RunResult::failed("Sign in to the installed Codex CLI with ChatGPT. API-key authentication is not supported by this in-app engine.".into()),
        Err(_) if cancelled() => return RunResult::stopped(),
        Err(error) => return RunResult::failed(error),
    }
    // Recheck after auth, immediately before spawning model work. A workspace
    // modified during preflight must not bypass the configuration exclusion.
    if let Err(error) = validate_workspace(&workspace) {
        return RunResult::failed(error);
    }
    if cancelled() {
        return RunResult::stopped();
    }
    let command = codex_command(executable, &workspace, request.sandbox);
    let process = match OwnedPipes::spawn_owned(command, true, &shared.owned_group) {
        Ok(process) => process,
        Err(error) => return RunResult::failed(error),
    };
    shared.change(id, |run| {
        run.snapshot.status = RunStatus::Running;
        run.note(
            "runtime",
            "Codex started · ChatGPT sign-in · network-disabled sandbox",
        );
        run.push(Record {
            kind: "user".into(),
            summary: request.task.clone(),
            detail: request.task.clone(),
            ..Default::default()
        });
    });
    let mut out = LineBuffer::default();
    let mut err = LineBuffer::default();
    let mut parser = EventParser::default();
    let outcome = pump(
        process,
        request.task.as_bytes(),
        request.timeout,
        MAX_OUTPUT_BYTES,
        &cancelled,
        |stream, bytes| {
            let lines = match stream {
                Stream::Stdout => &mut out,
                Stream::Stderr => &mut err,
            };
            lines.feed(bytes, |line| {
                accept_line(shared, id, &mut parser, stream, line)
            })?;
            match &parser.failure {
                Some(error) => Err(error.clone()),
                None => Ok(()),
            }
        },
    );
    // Flush bounded unterminated tails after ownership cleanup.
    out.finish(|line| accept_line(shared, id, &mut parser, Stream::Stdout, line));
    err.finish(|line| accept_line(shared, id, &mut parser, Stream::Stderr, line));
    match outcome {
        Ok(PumpOutcome { stopped: true, .. }) => RunResult::stopped(),
        Ok(PumpOutcome {
            timed_out: true, ..
        }) => RunResult {
            status: RunStatus::TimedOut,
            error: Some(
                "The run reached its time limit and its owned process group was stopped.".into(),
            ),
        },
        Ok(outcome) if !outcome.status.success() => {
            RunResult::failed(format!("Codex exited unsuccessfully ({})", outcome.status))
        }
        Ok(_) if parser.failure.is_some() => RunResult::failed(parser.failure.unwrap()),
        Ok(_) if !parser.completed || !parser.bound => RunResult::failed(
            "Codex exited without a complete turn and verified thread identity.".into(),
        ),
        Ok(_) => RunResult {
            status: RunStatus::Completed,
            error: None,
        },
        Err(_) if cancelled() => RunResult::stopped(),
        Err(error) => RunResult::failed(error),
    }
}

fn sanitized_command(executable: &Path, workspace: &Path) -> Command {
    let mut command = Command::new(executable);
    command.env_clear();
    // An allowlist also removes proxy, CODEX_HOME, key, base URL, endpoint,
    // provider, alternate-auth, Node-loader, and inherited-agent overrides.
    for name in ["HOME", "USER", "LOGNAME", "TMPDIR"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command.env("PATH", "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin");
    command
        .env("LANG", "en_US.UTF-8")
        .env("LC_ALL", "en_US.UTF-8");
    command.current_dir(workspace).process_group(0);
    command
}

fn codex_command(executable: &Path, workspace: &Path, sandbox: SandboxMode) -> Command {
    let mut command = sanitized_command(executable, workspace);
    command.args([
        "-a",
        "never",
        "exec",
        "--ignore-user-config",
        "--json",
        "--color",
        "never",
        "--sandbox",
        sandbox.argument(),
        "-C",
    ]);
    command.arg(workspace);
    for feature in [
        "multi_agent",
        "multi_agent_v2",
        "goals",
        "hooks",
        "apps",
        "plugins",
        "remote_plugin",
        "browser_use",
        "browser_use_external",
        "computer_use",
        "in_app_browser",
        "image_generation",
        "memories",
        "chronicle",
        "skill_search",
        "skill_mcp_dependency_install",
        "unbounded_connection_retries",
        "shell_snapshot",
    ] {
        command.args(["--disable", feature]);
    }
    for setting in [
        "sandbox_workspace_write.network_access=false",
        "forced_login_method=\"chatgpt\"",
        "model_provider=\"openai\"",
        "web_search=\"disabled\"",
    ] {
        command.args(["-c", setting]);
    }
    command.arg("-");
    command
}

fn probe(
    executable: &Path,
    arguments: &[OsString],
    workspace: &Path,
    cancelled: &impl Fn() -> bool,
    registry: &Arc<GroupRegistry>,
) -> Result<String, String> {
    if cancelled() {
        return Err("Run cancelled before authentication.".into());
    }
    let mut command = sanitized_command(executable, workspace);
    command.args(arguments);
    let process = OwnedPipes::spawn_owned(command, false, registry)?;
    let mut output = Vec::new();
    let outcome = pump(
        process,
        &[],
        PROBE_TIMEOUT,
        PROBE_OUTPUT_BYTES,
        cancelled,
        |_, bytes| {
            output.extend_from_slice(bytes);
            Ok(())
        },
    )?;
    if outcome.stopped || outcome.timed_out || !outcome.status.success() {
        return Err("The installed Codex authentication/version check failed or timed out. No task was submitted.".into());
    }
    Ok(String::from_utf8_lossy(&output).into_owned())
}

#[derive(Clone, Copy)]
enum Stream {
    Stdout,
    Stderr,
}

#[derive(Default)]
struct LineBuffer {
    pending: Vec<u8>,
}
impl LineBuffer {
    fn feed(&mut self, bytes: &[u8], mut accept: impl FnMut(&[u8])) -> Result<(), String> {
        for part in bytes.split_inclusive(|byte| *byte == b'\n') {
            if self.pending.len().saturating_add(part.len()) > MAX_LINE_BYTES {
                self.pending.clear();
                return Err("Codex output exceeded the 64 KiB line limit; its owned process group was stopped.".into());
            }
            self.pending.extend_from_slice(part);
            if self.pending.last() == Some(&b'\n') {
                accept(&self.pending);
                self.pending.clear();
            }
        }
        Ok(())
    }
    fn finish(&mut self, mut accept: impl FnMut(&[u8])) {
        if !self.pending.is_empty() {
            accept(&self.pending);
            self.pending.clear();
        }
    }
}

#[derive(Default)]
struct EventParser {
    bound: bool,
    completed: bool,
    failure: Option<String>,
}

fn accept_line(shared: &Shared, id: &str, parser: &mut EventParser, stream: Stream, line: &[u8]) {
    let text = String::from_utf8_lossy(line);
    if text.trim().is_empty() {
        return;
    }
    shared.change(id, |run| {
        if matches!(stream, Stream::Stderr) {
            run.push(Record {
                kind: "runtime".into(),
                summary: text.trim().into(),
                detail: text.into_owned(),
                is_infrastructure: true,
                ..Default::default()
            });
        } else {
            parser.parse(run, &text);
        }
    });
}

impl EventParser {
    fn parse(&mut self, run: &mut StoredRun, raw: &str) {
        let value = match serde_json::from_str::<Value>(raw) {
            Ok(value) => value,
            Err(_) => {
                run.push(Record {
                    kind: "runtime".into(),
                    summary: "Codex emitted non-JSON output".into(),
                    detail: raw.into(),
                    is_infrastructure: true,
                    ..Default::default()
                });
                return;
            }
        };
        let event = string(&value["type"]);
        let mut kind = "runtime";
        let mut status = String::new();
        let mut detail = value.clone();
        let summary = match event {
            "thread.started" => {
                let thread = string(&value["thread_id"]);
                if valid_thread_id(thread) && !self.bound {
                    self.bound = true;
                    run.snapshot.thread_id = Some(thread.into());
                    "Codex thread started".into()
                } else if run.snapshot.thread_id.as_deref() == Some(thread) {
                    "Codex thread confirmed".into()
                } else {
                    self.failure =
                        Some("Codex emitted an invalid or conflicting thread identity.".into());
                    kind = "error";
                    "Invalid Codex thread identity".into()
                }
            }
            "turn.started" => "Codex is working".into(),
            "turn.completed" => {
                self.completed = true;
                let usage = &value["usage"];
                let input = usage["input_tokens"].as_u64().unwrap_or(0);
                let output = usage["output_tokens"].as_u64().unwrap_or(0);
                run.snapshot.usage = Some(Usage {
                    input,
                    output,
                    total: input.saturating_add(output),
                });
                format!("Turn completed · {input} input / {output} output tokens")
            }
            "turn.failed" | "error" => {
                kind = "error";
                let message = [
                    string(&value["error"]["message"]),
                    string(&value["message"]),
                ]
                .into_iter()
                .find(|text| !text.is_empty())
                .unwrap_or("Codex reported an error");
                if event == "turn.failed" {
                    self.failure = Some(bounded(&telemetry::redact(message), 2048));
                }
                message.into()
            }
            "item.started" | "item.updated" | "item.completed" => {
                let item = &value["item"];
                let completed = event == "item.completed";
                let failed = item["status"] == "failed"
                    || item["exit_code"].as_i64().is_some_and(|code| code != 0)
                    || !item["error"].is_null();
                status = if failed {
                    "Error"
                } else if completed {
                    "Completed"
                } else {
                    "Result not recorded"
                }
                .into();
                match string(&item["type"]) {
                    "agent_message" => {
                        kind = "assistant";
                        detail = json!({"text":item["text"],"raw_event":value});
                        string(&item["text"]).into()
                    }
                    "reasoning" => {
                        kind = "reasoning";
                        string(&item["text"]).into()
                    }
                    "command_execution" => {
                        kind = "tool";
                        if completed && item["exit_code"].as_i64().is_none() && !failed {
                            status = "Result not recorded".into();
                        }
                        detail = json!({
                            "name":"exec_command",
                            "input":{"cmd":item["command"],"workdir":run.snapshot.workspace},
                            "output":if completed { json!({"output":item["aggregated_output"],"exit_code":item["exit_code"],"status":item["status"]}) } else { Value::Null },
                            "call":{"id":item["id"],"type":"codex_exec_command"},
                            "result":if completed { json!({"id":item["id"],"type":"codex_exec_command","exit_code":item["exit_code"]}) } else { Value::Null },
                            "raw_event":value,
                        });
                        format!("Command · {}", string(&item["command"]))
                    }
                    "file_change" => {
                        kind = "tool";
                        detail = json!({"name":"file_change","input":{"changes":item["changes"]},"output":if completed {json!({"status":item["status"]})} else {Value::Null},"call":{"id":item["id"]},"result":if completed {json!({"id":item["id"]})} else {Value::Null},"raw_event":value});
                        let paths = item["changes"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .take(4)
                            .map(|change| string(&change["path"]))
                            .collect::<Vec<_>>();
                        format!("File changes · {}", paths.join(", "))
                    }
                    "mcp_tool_call" => {
                        kind = "tool";
                        detail = json!({"name":format!("{}.{}",string(&item["server"]),string(&item["tool"])),"input":item["arguments"],"output":if completed {json!({"result":item["result"],"error":item["error"]})} else {Value::Null},"call":{"id":item["id"]},"result":if completed {json!({"id":item["id"]})} else {Value::Null},"raw_event":value});
                        format!(
                            "Tool · {} / {}",
                            string(&item["server"]),
                            string(&item["tool"])
                        )
                    }
                    "web_search" => {
                        kind = "tool";
                        format!("Web search · {}", string(&item["query"]))
                    }
                    "todo_list" => "Task plan updated".into(),
                    other => format!("Codex activity · {}", bounded(other, 120)),
                }
            }
            other => format!("Codex event · {}", bounded(other, 120)),
        };
        run.push(Record {
            kind: kind.into(),
            summary,
            detail: detail.to_string(),
            tool_status: status,
            is_infrastructure: kind == "runtime",
            ..Default::default()
        });
    }
}

fn valid_thread_id(thread: &str) -> bool {
    thread.len() == 36
        && thread.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn string(value: &Value) -> &str {
    value.as_str().unwrap_or("")
}
fn bounded(value: &str, maximum: usize) -> String {
    if value.len() <= maximum {
        return value.into();
    }
    let mut end = maximum.saturating_sub("…".len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}

/// Keep inspector JSON valid when details exceed their cap. Large exact inputs
/// or outputs become absent evidence rather than misleading clipped commands or
/// apparently complete historical file contents.
fn bounded_detail(safe: &str) -> String {
    if safe.len() <= MAX_DETAIL_BYTES {
        return safe.into();
    }
    if let Ok(Value::Object(mut object)) = serde_json::from_str::<Value>(safe) {
        let raw = object
            .remove("raw_event")
            .map(|raw| raw.to_string())
            .unwrap_or_else(|| safe.into());
        object.insert("detail_truncated".into(), Value::Bool(true));
        object.insert(
            "raw_event_prefix".into(),
            Value::String(bounded(&raw, 1024)),
        );
        for key in ["output", "input", "text", "content"] {
            let encoded = Value::Object(object.clone()).to_string();
            if encoded.len() <= MAX_DETAIL_BYTES {
                return encoded;
            }
            if object.contains_key(key) {
                object.insert(key.into(), Value::Null);
            }
        }
        let encoded = Value::Object(object).to_string();
        if encoded.len() <= MAX_DETAIL_BYTES {
            return encoded;
        }
    }
    json!({"detail_truncated":true,"raw_event_prefix":bounded(safe, MAX_DETAIL_BYTES / 8)})
        .to_string()
}
fn timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string()
}

struct OwnedChild {
    child: Child,
    reaped: bool,
    registry: Option<Arc<GroupRegistry>>,
}
impl OwnedChild {
    fn clear_owner(&self) {
        if let Some(registry) = &self.registry {
            let mut owner = registry
                .pid
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if *owner == Some(self.child.id()) {
                *owner = None;
            }
        }
    }
    fn exited(&self) -> Result<bool, String> {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // WNOWAIT leaves the leader PID owned until group cleanup, preventing a
        // reused PID/process group from being signalled after an early reap.
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                self.child.id() as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result == 0 {
            return Ok(unsafe { info.si_pid() } == self.child.id() as i32);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::Interrupted {
            Ok(false)
        } else {
            Err(format!("Could not observe owned Codex process: {error}"))
        }
    }
    fn signal(&self, signal: i32) {
        unsafe {
            libc::kill(-(self.child.id() as i32), signal);
        }
    }
    fn reap(&mut self) -> Result<ExitStatus, String> {
        self.clear_owner();
        let status = self
            .child
            .wait()
            .map_err(|error| format!("Could not reap owned Codex process: {error}"))?;
        self.reaped = true;
        Ok(status)
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        // Do not try_wait here: reaping before signalling would permit PID reuse
        // and could leave an owned descendant running after its leader exited.
        if self.reaped {
            return;
        }
        self.signal(libc::SIGKILL);
        self.clear_owner();
        let _ = self.child.wait();
    }
}

struct OwnedPipes {
    process: OwnedChild,
    stdout: ChildStdout,
    stderr: ChildStderr,
    stdin: Option<ChildStdin>,
}
impl OwnedPipes {
    fn spawn_owned(
        command: Command,
        input: bool,
        registry: &Arc<GroupRegistry>,
    ) -> Result<Self, String> {
        Self::spawn_registered(registry, || Self::spawn(command, input))
    }

    /// The registry lock spans fork/exec and publication. Emergency shutdown
    /// cannot return while an unregistered child exists, including pipe-setup
    /// failures (whose local guard kills/reaps before this lock is released).
    fn spawn_registered(
        registry: &Arc<GroupRegistry>,
        spawn: impl FnOnce() -> Result<Self, String>,
    ) -> Result<Self, String> {
        let mut owner = registry
            .pid
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if registry.force_closed.load(Ordering::Acquire) {
            return Err("The run engine is shutting down. No child was started.".into());
        }
        if owner.is_some() {
            return Err("The owned process slot is not free.".into());
        }
        let mut pipes = spawn()?;
        *owner = Some(pipes.process.child.id());
        pipes.process.registry = Some(Arc::clone(registry));
        // Shutdown can set the flag while waiting for this same lock. Kill
        // before handing a newly spawned task to the stdin pump in that case.
        if registry.force_closed.load(Ordering::Acquire) {
            pipes.process.signal(libc::SIGKILL);
        }
        Ok(pipes)
    }

    fn spawn(mut command: Command, input: bool) -> Result<Self, String> {
        command
            .stdin(if input { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let child = command
            .spawn()
            .map_err(|error| format!("Could not start the installed Codex CLI: {error}"))?;
        let mut process = OwnedChild {
            child,
            reaped: false,
            registry: None,
        };
        let stdout = process
            .child
            .stdout
            .take()
            .ok_or("Owned Codex stdout unavailable.")?;
        let stderr = process
            .child
            .stderr
            .take()
            .ok_or("Owned Codex stderr unavailable.")?;
        let stdin = process.child.stdin.take();
        nonblocking(stdout.as_raw_fd())?;
        nonblocking(stderr.as_raw_fd())?;
        if let Some(stdin) = &stdin {
            nonblocking(stdin.as_raw_fd())?;
        }
        Ok(Self {
            process,
            stdout,
            stderr,
            stdin,
        })
    }
}

fn nonblocking(fd: RawFd) -> Result<(), String> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err("Could not prepare nonblocking owned Codex pipes.".into());
    }
    Ok(())
}

struct PumpOutcome {
    status: ExitStatus,
    stopped: bool,
    timed_out: bool,
}
fn pump(
    mut pipes: OwnedPipes,
    input: &[u8],
    timeout: Duration,
    byte_limit: usize,
    cancelled: &impl Fn() -> bool,
    mut accept: impl FnMut(Stream, &[u8]) -> Result<(), String>,
) -> Result<PumpOutcome, String> {
    let deadline = Instant::now() + timeout;
    let mut input_position = 0;
    let mut stdout_closed = false;
    let mut stderr_closed = false;
    let mut bytes = 0usize;
    let mut failure = None;
    let mut stopped = false;
    let mut timed_out = false;
    let mut term_at = None;
    let mut killed = false;
    loop {
        if term_at.is_none() && cancelled() {
            stopped = true;
            pipes.stdin.take();
            pipes.process.signal(libc::SIGTERM);
            term_at = Some(Instant::now());
        }
        let exited = match pipes.process.exited() {
            Ok(exited) => exited,
            Err(error) => return Err(error),
        };
        if failure.is_none() {
            if let Some(stdin) = pipes.stdin.as_mut() {
                match stdin.write(&input[input_position..]) {
                    Ok(count) => input_position += count,
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                        ) => {}
                    Err(error) => {
                        failure = Some(format!("Could not submit the task over stdin: {error}"));
                    }
                }
                if input_position == input.len() {
                    pipes.stdin.take();
                }
            }
            for (stream, reader, closed) in [
                (
                    Stream::Stdout,
                    &mut pipes.stdout as &mut dyn Read,
                    &mut stdout_closed,
                ),
                (
                    Stream::Stderr,
                    &mut pipes.stderr as &mut dyn Read,
                    &mut stderr_closed,
                ),
            ] {
                if let Err(error) = drain(reader, closed, &mut bytes, byte_limit, |chunk| {
                    accept(stream, chunk)
                }) {
                    failure.get_or_insert(error);
                    break;
                }
            }
        }
        // Finish already-written output before timing out a completed child.
        if exited && stdout_closed && stderr_closed {
            pipes.process.signal(libc::SIGKILL);
            let status = pipes.process.reap()?;
            return match failure {
                Some(error) => Err(error),
                None => Ok(PumpOutcome {
                    status,
                    stopped,
                    timed_out,
                }),
            };
        }
        let now = Instant::now();
        if term_at.is_none() {
            stopped = cancelled();
            timed_out = !stopped && now >= deadline;
            // If the leader exited but a descendant holds a pipe open, stop
            // that owned group too; never leak a background tool subprocess.
            if stopped || timed_out || failure.is_some() || exited {
                pipes.stdin.take();
                pipes.process.signal(libc::SIGTERM);
                term_at = Some(now);
            }
        }
        if term_at.is_some_and(|at| now.duration_since(at) >= TERM_GRACE) && !killed {
            pipes.process.signal(libc::SIGKILL);
            killed = true;
        }
        if killed && exited {
            let status = pipes.process.reap()?;
            return match failure {
                Some(error) => Err(error),
                None => Ok(PumpOutcome {
                    status,
                    stopped,
                    timed_out,
                }),
            };
        }
        let mut descriptors = [
            libc::pollfd {
                fd: if stdout_closed || failure.is_some() {
                    -1
                } else {
                    pipes.stdout.as_raw_fd()
                },
                events: libc::POLLIN | libc::POLLHUP,
                revents: 0,
            },
            libc::pollfd {
                fd: if stderr_closed || failure.is_some() {
                    -1
                } else {
                    pipes.stderr.as_raw_fd()
                },
                events: libc::POLLIN | libc::POLLHUP,
                revents: 0,
            },
            libc::pollfd {
                fd: pipes.stdin.as_ref().map_or(-1, AsRawFd::as_raw_fd),
                events: libc::POLLOUT | libc::POLLHUP,
                revents: 0,
            },
        ];
        let polled = unsafe {
            libc::poll(
                descriptors.as_mut_ptr(),
                descriptors.len() as libc::nfds_t,
                POLL_MS,
            )
        };
        if polled == -1 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
        {
            failure.get_or_insert("Could not poll owned Codex output.".into());
        }
    }
}

fn drain(
    reader: &mut dyn Read,
    closed: &mut bool,
    total: &mut usize,
    maximum: usize,
    mut accept: impl FnMut(&[u8]) -> Result<(), String>,
) -> Result<(), String> {
    let mut buffer = [0u8; 8192];
    for _ in 0..16 {
        if *closed {
            break;
        }
        match reader.read(&mut buffer) {
            Ok(0) => {
                *closed = true;
                break;
            }
            Ok(count) => {
                *total = total.saturating_add(count);
                if *total > maximum {
                    return Err("Codex output exceeded its bounded capture limit; its owned process group was stopped.".into());
                }
                accept(&buffer[..count])?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(format!("Could not read owned Codex output: {error}")),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::os::unix::fs::PermissionsExt;

    static ENGINE_TEST: Mutex<()> = Mutex::new(());
    const THREAD: &str = "01a14538-61de-71b2-82bc-bf67dedc2822";

    struct TestFiles {
        root: PathBuf,
    }
    impl TestFiles {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "agentwatcher-engine-test-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            Self { root }
        }
        fn executable(&self, body: &str) -> PathBuf {
            let path = self.root.join("fake-codex");
            fs::write(&path, format!("#!/bin/sh\ncase \"$1\" in\n--version) printf 'codex-cli 0.162.1\\n'; exit 0;;\nlogin) printf 'Logged in using ChatGPT\\n' >&2; exit 0;;\nesac\n{body}\n")).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            path
        }
    }
    impl Drop for TestFiles {
        fn drop(&mut self) {
            // Only the exact private directory created by this test is removed.
            // No provider installation, auth, user workspace, or history is used.
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn request(workspace: &Path) -> LaunchRequest {
        LaunchRequest {
            task: "Inspect this test fixture without changes".into(),
            title: "Fixture inspection".into(),
            workspace: workspace.into(),
            sandbox: SandboxMode::ReadOnly,
            parent: Some(ParentKey {
                source: "deepseek".into(),
                id: "session-parent".into(),
            }),
            timeout: Duration::from_secs(60),
            acknowledge_possible_paid_credit_usage: true,
        }
    }

    fn stored() -> StoredRun {
        StoredRun {
            snapshot: RunSnapshot {
                launch_id: "test".into(),
                title: "Test".into(),
                task: "Test".into(),
                workspace: PathBuf::from("/tmp"),
                sandbox: SandboxMode::ReadOnly,
                parent: None,
                thread_id: None,
                status: RunStatus::Running,
                records: Vec::new(),
                usage: None,
                started_at: "0".into(),
                finished_at: None,
                error: None,
                truncated: false,
            },
            record_bytes: 0,
            sequence: 0,
        }
    }

    fn terminal(engine: &Engine, id: &str) -> RunSnapshot {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let run = engine
                .snapshot()
                .into_iter()
                .find(|run| run.launch_id == id)
                .unwrap();
            if !run.status.is_active() {
                return run;
            }
            assert!(
                Instant::now() < deadline,
                "fake execution did not finish: {:?}",
                run.status
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_running(engine: &Engine, id: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let run = engine
                .snapshot()
                .into_iter()
                .find(|run| run.launch_id == id)
                .unwrap();
            if run.status == RunStatus::Running && run.thread_id.is_some() {
                return;
            }
            assert!(
                run.status.is_active(),
                "fake execution failed: {:?}",
                run.error
            );
            assert!(Instant::now() < deadline, "fake execution did not start");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn validation_rejects_unacknowledged_oversize_and_unbounded_requests() {
        let mut request = request(Path::new("/tmp"));
        request.acknowledge_possible_paid_credit_usage = false;
        assert!(
            validate_request(&request)
                .unwrap_err()
                .contains("paid-credit")
        );
        request.acknowledge_possible_paid_credit_usage = true;
        request.task = "x".repeat(MAX_TASK_BYTES + 1);
        assert!(validate_request(&request).is_err());
        request.task = " ".into();
        assert!(validate_request(&request).is_err());
        request.task = "a\0b".into();
        assert!(validate_request(&request).is_err());
        request.task = "task".into();
        request.title = "語".repeat(121);
        assert!(validate_request(&request).is_err());
        request.title = "語".repeat(120);
        assert!(validate_request(&request).is_ok());
        request.title.clear();
        request.parent = Some(ParentKey {
            source: "codex".into(),
            id: "graph/aw-owned-launch".into(),
        });
        assert!(validate_request(&request).is_ok());
        assert_eq!(launch_title(&request), "task");
        request.timeout = Duration::from_secs(601);
        assert!(validate_request(&request).is_err());
        request.timeout = Duration::from_secs(59);
        assert!(validate_request(&request).is_err());
        request.timeout = Duration::from_secs(60);
        request.workspace = PathBuf::from("relative");
        assert!(validate_request(&request).is_err());
    }

    #[test]
    fn workspace_configuration_is_rejected_through_ancestry_and_symlinks() {
        let files = TestFiles::new();
        let nested = files.root.join("project/nested");
        fs::create_dir_all(&nested).unwrap();
        assert!(validate_workspace(&nested).is_ok());
        fs::create_dir(files.root.join("project/.codex")).unwrap();
        fs::write(
            files.root.join("project/.codex/config.toml"),
            "[mcp_servers.untrusted]\ncommand='anything'",
        )
        .unwrap();
        assert!(
            validate_workspace(&nested)
                .unwrap_err()
                .contains("configuration")
        );
        let link = files.root.join("alias");
        std::os::unix::fs::symlink(&nested, &link).unwrap();
        assert!(validate_workspace(&link).is_err());
        assert!(validate_workspace(&files.root.join("missing")).is_err());
        fs::write(files.root.join("plain-file"), "file").unwrap();
        assert!(validate_workspace(&files.root.join("plain-file")).is_err());
    }

    #[test]
    fn argv_is_separate_from_prompt_and_auth_environment_is_allowlisted() {
        let workspace = Path::new("/tmp/workspace with 'quotes' and $(text)");
        let command = codex_command(Path::new(CODEX), workspace, SandboxMode::WorkspaceWrite);
        assert_eq!(command.get_program(), OsStr::new(CODEX));
        let args = command
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(&args[..4], &["-a", "never", "exec", "--ignore-user-config"]);
        assert!(
            args.windows(2)
                .any(|pair| pair == ["-C", workspace.to_str().unwrap()])
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--sandbox", "workspace-write"])
        );
        for expected in [
            "forced_login_method=\"chatgpt\"",
            "model_provider=\"openai\"",
            "web_search=\"disabled\"",
            "sandbox_workspace_write.network_access=false",
        ] {
            assert!(args.iter().any(|arg| arg == expected));
        }
        assert_eq!(args.last().unwrap(), "-");
        assert!(!args.iter().any(|arg| {
            [
                "resume",
                "fork",
                "--ephemeral",
                "--ignore-rules",
                "--dangerously-bypass-approvals-and-sandbox",
            ]
            .contains(&arg.as_str())
        }));
        let env = command.get_envs().collect::<Vec<_>>();
        for (key, _) in env {
            assert!(
                [
                    "HOME", "USER", "LOGNAME", "TMPDIR", "PATH", "LANG", "LC_ALL"
                ]
                .contains(&key.to_str().unwrap())
            );
        }
        assert!(!args.iter().any(|arg| arg.contains("session-parent")));
    }

    #[test]
    fn json_events_bind_identity_report_tools_usage_and_redact_evidence() {
        let mut run = stored();
        let mut parser = EventParser::default();
        parser.parse(
            &mut run,
            &format!(r#"{{"type":"thread.started","thread_id":"{THREAD}"}}"#),
        );
        parser.parse(&mut run, r#"{"type":"item.completed","item":{"id":"cmd-1","type":"command_execution","command":"printf result","aggregated_output":"OPENAI_API_KEY=top-secret","status":"completed","exit_code":0}}"#);
        parser.parse(&mut run, r#"{"type":"item.completed","item":{"id":"message","type":"agent_message","text":"Inspected the fixture"}}"#);
        parser.parse(&mut run, r#"{"type":"turn.completed","usage":{"input_tokens":13,"cached_input_tokens":3,"output_tokens":7}}"#);
        assert_eq!(run.snapshot.thread_id.as_deref(), Some(THREAD));
        assert!(parser.completed);
        assert_eq!(run.snapshot.usage.unwrap().total, 20);
        assert_eq!(run.snapshot.records[1].kind, "tool");
        assert_eq!(run.snapshot.records[1].tool_status, "Completed");
        let normalized: Value = serde_json::from_str(&run.snapshot.records[1].detail).unwrap();
        assert_eq!(normalized["name"], "exec_command");
        assert_eq!(normalized["input"]["cmd"], "printf result");
        assert_eq!(normalized["output"]["exit_code"], 0);
        assert!(!run.snapshot.records[1].detail.contains("top-secret"));
        assert!(run.snapshot.records[1].detail.contains("redacted"));
        assert_eq!(run.snapshot.records[2].kind, "assistant");
    }

    #[test]
    fn conflicting_thread_and_failed_turn_cannot_be_reported_as_success() {
        let mut run = stored();
        let mut parser = EventParser::default();
        parser.parse(
            &mut run,
            &format!(r#"{{"type":"thread.started","thread_id":"{THREAD}"}}"#),
        );
        parser.parse(
            &mut run,
            r#"{"type":"thread.started","thread_id":"01a14538-61de-71b2-82bc-bf67dedc2823"}"#,
        );
        assert!(parser.failure.as_ref().unwrap().contains("conflicting"));
        assert_eq!(run.snapshot.thread_id.as_deref(), Some(THREAD));
        let mut parser = EventParser::default();
        parser.parse(
            &mut run,
            r#"{"type":"turn.failed","error":{"message":"Usage limit reached"}}"#,
        );
        assert_eq!(parser.failure.as_deref(), Some("Usage limit reached"));
        assert!(!parser.completed);
    }

    #[test]
    fn lines_and_retention_are_bounded_across_utf8_and_chunk_boundaries() {
        let mut lines = LineBuffer::default();
        let mut received = Vec::new();
        lines
            .feed(b"{\"text\":\"", |line| received.push(line.to_vec()))
            .unwrap();
        lines
            .feed("語\"}\nnext".as_bytes(), |line| {
                received.push(line.to_vec())
            })
            .unwrap();
        lines.finish(|line| received.push(line.to_vec()));
        assert_eq!(received.len(), 2);
        assert!(
            String::from_utf8(received.remove(0))
                .unwrap()
                .contains('語')
        );
        assert!(
            lines
                .feed(&vec![b'x'; MAX_LINE_BYTES + 1], |_| panic!(
                    "oversize line emitted"
                ))
                .is_err()
        );
        assert!(lines.pending.is_empty());
        let mut run = stored();
        for index in 0..(MAX_RECORDS + 100) {
            run.push(Record {
                kind: "assistant".into(),
                summary: index.to_string(),
                detail: "語".repeat(4000),
                ..Default::default()
            });
        }
        assert!(run.snapshot.records.len() <= MAX_RECORDS);
        assert!(run.record_bytes <= MAX_RETAINED_BYTES);
        assert!(
            run.snapshot
                .records
                .iter()
                .all(|record| record.detail.len() <= MAX_DETAIL_BYTES)
        );
        assert!(run.snapshot.truncated);
        assert_eq!(
            run.snapshot.records.last().unwrap().summary,
            (MAX_RECORDS + 99).to_string()
        );
    }

    #[test]
    fn inert_fake_execution_streams_stdin_and_keeps_app_parent_separate() {
        let _serial = ENGINE_TEST.lock().unwrap();
        let files = TestFiles::new();
        let fake = files.executable(&format!(r#"
task=$(cat)
test "$task" = 'Inspect this test fixture without changes' || exit 7
printf '%s\n' '{{"type":"thread.started","thread_id":"{THREAD}"}}'
printf '%s\n' '{{"type":"item.completed","item":{{"type":"agent_message","text":"Fixture inspected"}}}}'
printf '%s\n' '{{"type":"turn.completed","usage":{{"input_tokens":3,"output_tokens":4}}}}'
"#));
        let engine = Engine::with_executable(fake);
        let before = engine.revision();
        let info = engine.start(request(&files.root)).unwrap();
        let run = terminal(&engine, &info.launch_id);
        assert_eq!(run.status, RunStatus::Completed, "{:?}", run.error);
        assert_eq!(run.thread_id.as_deref(), Some(THREAD));
        assert_eq!(run.parent.unwrap().source, "deepseek");
        assert_eq!(run.usage.unwrap().total, 7);
        assert!(
            run.records
                .iter()
                .any(|record| record.summary == "Fixture inspected")
        );
        assert!(engine.revision() > before);
        assert!(engine.stop("not-owned").is_err());
    }

    #[test]
    fn owned_execution_supplies_context_skill_reads_and_announcements() {
        let _serial = ENGINE_TEST.lock().unwrap();
        let files = TestFiles::new();
        let skill = files.root.join("sample/SKILL.md");
        fs::create_dir(skill.parent().unwrap()).unwrap();
        fs::write(&skill, "# Sample\nCurrent instructions").unwrap();
        let events = [
            json!({"type":"thread.started","thread_id":THREAD}),
            json!({"type":"item.completed","item":{"id":"read","type":"command_execution","command":format!("cat {}", skill.display()),"aggregated_output":"# Sample\nHistorical instructions","exit_code":0,"status":"completed"}}),
            json!({"type":"item.completed","item":{"id":"announce","type":"agent_message","text":"I am using the sample skill."}}),
            json!({"type":"turn.completed","usage":{"input_tokens":2,"output_tokens":3}}),
        ];
        let mut body = "cat >/dev/null\n".to_string();
        for event in events {
            body.push_str(&format!(
                "printf '%s\\n' '{}'\n",
                event.to_string().replace('\'', "'\"'\"'")
            ));
        }
        let fake = files.executable(&body);
        let engine = Engine::with_executable(fake);
        let info = engine.start(request(&files.root)).unwrap();
        let run = terminal(&engine, &info.launch_id);
        assert_eq!(run.status, RunStatus::Completed, "{:?}", run.error);
        let session = telemetry::Session {
            id: run.thread_id.unwrap(),
            project: files.root.to_string_lossy().into_owned(),
            source: "codex".into(),
            ..Default::default()
        };
        let snapshot = telemetry::Snapshot {
            session: session.clone(),
            records: run.records,
            ..Default::default()
        };
        let context = crate::agent_context::collect(&session, &snapshot);
        assert!(context.skills.iter().any(|evidence| evidence.kind
            == crate::agent_context::SkillEvidenceKind::ConfirmedRead
            && evidence.path.as_deref() == skill.to_str()
            && evidence.recorded_text.as_deref() == Some("# Sample\nHistorical instructions")));
        assert!(context.skills.iter().any(|evidence| evidence.kind
            == crate::agent_context::SkillEvidenceKind::AnnouncedUse
            && evidence.name == "sample"));
    }

    #[test]
    fn failed_command_and_truncated_details_cannot_confirm_successful_reads() {
        let mut run = stored();
        let mut parser = EventParser::default();
        parser.parse(&mut run, r#"{"type":"item.completed","item":{"id":"read","type":"command_execution","command":"cat /tmp/sample/SKILL.md","aggregated_output":"missing","exit_code":1,"status":"completed"}}"#);
        assert_eq!(run.snapshot.records[0].tool_status, "Error");
        parser.parse(&mut run, &json!({"type":"item.completed","item":{"id":"large","type":"command_execution","command":"cat /tmp/sample/SKILL.md","aggregated_output":"x".repeat(MAX_DETAIL_BYTES * 2),"exit_code":0,"status":"completed"}}).to_string());
        let record = run.snapshot.records.last().unwrap();
        assert!(record.detail.len() <= MAX_DETAIL_BYTES);
        let detail: Value = serde_json::from_str(&record.detail).unwrap();
        assert_eq!(detail["detail_truncated"], true);
        assert!(detail["output"].is_null());
        let session = telemetry::Session {
            id: THREAD.into(),
            project: "/tmp".into(),
            source: "codex".into(),
            ..Default::default()
        };
        let snapshot = telemetry::Snapshot {
            session: session.clone(),
            records: run.snapshot.records,
            ..Default::default()
        };
        assert!(
            crate::agent_context::collect(&session, &snapshot)
                .skills
                .is_empty()
        );
        let control_detail = bounded_detail(&"\0".repeat(MAX_DETAIL_BYTES * 2));
        assert!(control_detail.len() <= MAX_DETAIL_BYTES);
        assert!(serde_json::from_str::<Value>(&control_detail).is_ok());
    }

    #[test]
    fn stop_kills_only_owned_group_with_term_kill_fallback_and_no_retry() {
        let _serial = ENGINE_TEST.lock().unwrap();
        let files = TestFiles::new();
        let marker = files.root.join("attempts");
        let fake = files.executable(&format!(
            r#"
cat >/dev/null
printf 'x' >> '{}'
trap '' TERM
printf '%s\n' '{{"type":"thread.started","thread_id":"{THREAD}"}}'
while :; do sleep 1; done
"#,
            marker.display()
        ));
        let mut unrelated = Command::new("/bin/sleep").arg("20").spawn().unwrap();
        let engine = Engine::with_executable(fake);
        let info = engine.start(request(&files.root)).unwrap();
        wait_running(&engine, &info.launch_id);
        assert!(engine.start(request(&files.root)).is_err());
        let stop_started = Instant::now();
        engine.stop(&info.launch_id).unwrap();
        let run = terminal(&engine, &info.launch_id);
        assert_eq!(run.status, RunStatus::Stopped);
        assert!(stop_started.elapsed() < Duration::from_secs(5));
        assert!(unrelated.try_wait().unwrap().is_none());
        assert_eq!(fs::read(&marker).unwrap(), b"x");
        let _ = unrelated.kill();
        let _ = unrelated.wait();
    }

    #[test]
    fn shutdown_returns_immediately_but_idle_waits_for_owned_group_reaping() {
        let _serial = ENGINE_TEST.lock().unwrap();
        let files = TestFiles::new();
        let marker = files.root.join("owned-pid");
        let fake = files.executable(&format!(
            r#"
cat >/dev/null
trap '' TERM
printf '%s' "$$" > '{}'
printf '%s\n' '{{"type":"thread.started","thread_id":"{THREAD}"}}'
while :; do sleep 1; done
"#,
            marker.display()
        ));
        let engine = Engine::with_executable(fake);
        assert!(engine.is_idle());
        let info = engine.start(request(&files.root)).unwrap();
        wait_running(&engine, &info.launch_id);
        let pid: i32 = fs::read_to_string(marker).unwrap().parse().unwrap();
        let started = Instant::now();
        engine.shutdown();
        assert!(started.elapsed() < Duration::from_millis(100));
        assert!(engine.start(request(&files.root)).is_err());
        let deadline = Instant::now() + Duration::from_secs(5);
        while !engine.is_idle() {
            assert!(
                Instant::now() < deadline,
                "shutdown failed to reap owned process"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(engine.snapshot()[0].status, RunStatus::Stopped);
    }

    #[test]
    fn os_quit_handle_kills_owned_group_and_late_registration_cannot_escape() {
        let _serial = ENGINE_TEST.lock().unwrap();
        let files = TestFiles::new();
        let marker = files.root.join("owned-pid");
        let fake = files.executable(&format!(
            r#"
cat >/dev/null
trap '' TERM
printf '%s' "$$" > '{}'
printf '%s\n' '{{"type":"thread.started","thread_id":"{THREAD}"}}'
while :; do sleep 1; done
"#,
            marker.display()
        ));
        let engine = Engine::with_executable(fake.clone());
        let handle = engine.shutdown_handle();
        let info = engine.start(request(&files.root)).unwrap();
        wait_running(&engine, &info.launch_id);
        let pid: i32 = fs::read_to_string(marker).unwrap().parse().unwrap();
        handle.force_shutdown();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !engine.is_idle() {
            assert!(
                Instant::now() < deadline,
                "emergency shutdown did not reap promptly"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert!(engine.shared.owned_group.pid.lock().unwrap().is_none());
        handle.force_shutdown(); // Cleared ownership makes repeated OS callbacks inert.

        let registry = Arc::new(GroupRegistry::default());
        registry.force_closed.store(true, Ordering::Release);
        assert!(
            OwnedPipes::spawn_owned(sanitized_command(&fake, &files.root), true, &registry)
                .is_err()
        );
        assert!(registry.pid.lock().unwrap().is_none());
    }

    #[test]
    fn emergency_shutdown_waits_for_spawn_registration_before_returning() {
        let _serial = ENGINE_TEST.lock().unwrap();
        let files = TestFiles::new();
        let fake = files.executable("trap '' TERM\nwhile :; do sleep 1; done");
        let engine = Engine::with_executable(fake.clone());
        let registry = Arc::clone(&engine.shared.owned_group);
        let check_registry = Arc::clone(&registry);
        let handle = engine.shutdown_handle();
        let command = sanitized_command(&fake, &files.root);
        let (spawned, observed) = std::sync::mpsc::channel();
        let (release, registration_barrier) = std::sync::mpsc::channel();
        let spawning = std::thread::spawn(move || {
            OwnedPipes::spawn_registered(&registry, || {
                let pipes = OwnedPipes::spawn(command, false)?;
                spawned.send(pipes.process.child.id()).unwrap();
                registration_barrier.recv().unwrap();
                Ok(pipes)
            })
            .unwrap()
        });
        let pid = observed.recv_timeout(Duration::from_secs(2)).unwrap();
        let (returned, has_returned) = std::sync::mpsc::channel();
        let quitting = std::thread::spawn(move || {
            handle.force_shutdown();
            returned.send(()).unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while !check_registry.force_closed.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        // The child exists, but its publication is deliberately paused. The
        // OS-quit callback must remain blocked on the ownership lock here.
        assert_eq!(
            has_returned.recv_timeout(Duration::from_millis(30)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        );
        release.send(()).unwrap();
        let pipes = spawning.join().unwrap();
        has_returned.recv_timeout(Duration::from_secs(2)).unwrap();
        quitting.join().unwrap();
        let outcome = pump(
            pipes,
            &[],
            Duration::from_secs(1),
            1024,
            &|| true,
            |_, _| Ok(()),
        )
        .unwrap();
        assert!(outcome.stopped);
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
        assert!(check_registry.pid.lock().unwrap().is_none());
    }

    #[test]
    fn api_auth_and_output_overflow_fail_closed_without_relaunching() {
        let _serial = ENGINE_TEST.lock().unwrap();
        let files = TestFiles::new();
        let fake = files.root.join("api-codex");
        let marker = files.root.join("must-not-run");
        fs::write(&fake, format!("#!/bin/sh\ncase \"$1\" in\n--version) echo 'codex-cli 0.162.1';;\nlogin) echo 'Logged in using an API key';;\n*) touch '{}';;\nesac\n", marker.display())).unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
        let engine = Engine::with_executable(fake);
        let info = engine.start(request(&files.root)).unwrap();
        let run = terminal(&engine, &info.launch_id);
        assert_eq!(run.status, RunStatus::Failed);
        assert!(run.error.unwrap().contains("API-key"));
        assert!(!marker.exists());
        drop(engine);
        let fake = files.executable("cat >/dev/null\nhead -c 70000 /dev/zero | tr '\\000' x\n");
        let engine = Engine::with_executable(fake);
        let info = engine.start(request(&files.root)).unwrap();
        let run = terminal(&engine, &info.launch_id);
        assert_eq!(run.status, RunStatus::Failed);
        assert!(run.error.unwrap().contains("line limit"));
    }

    #[test]
    fn pump_deadline_stops_inert_child_and_reaps_it() {
        let files = TestFiles::new();
        let fake = files.executable("trap '' TERM\nwhile :; do sleep 1; done");
        let command = sanitized_command(&fake, &files.root);
        let process = OwnedPipes::spawn(command, false).unwrap();
        let pid = process.process.child.id();
        let started = Instant::now();
        let outcome = pump(
            process,
            &[],
            Duration::from_millis(30),
            1024,
            &|| false,
            |_, _| Ok(()),
        )
        .unwrap();
        assert!(outcome.timed_out);
        assert!(!outcome.status.success());
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
    }
}
