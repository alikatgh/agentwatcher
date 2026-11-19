//! Read-only collectors and explicitly requested worker/process controls.
//!
//! This module never starts a worker while loading settings or collecting data.
//! Paths and commands are passed as argv, never interpolated into a shell.

use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
#[cfg(target_os = "linux")]
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

const COMMAND_LIMIT: usize = 16;
const MAX_CAPTURE: usize = 32 * 1024 * 1024;
// A normal source-file patch can exceed 160 KiB. Keep full requested patches
// readable while retaining a finite per-command output and rendering bound.
const DIFF_LIMIT: usize = 2 * 1024 * 1024;
const REPOSITORY_LIMIT: usize = 2 * 1024 * 1024;
const MAX_CHANGES: usize = 2_000;
const LOG_LIMIT: usize = 256 * 1024;

/// Local display and collection preferences. Defaults migrate older settings files.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub startup_view: usize,
    pub show_sidebar: bool,
    pub open_inspector: bool,
    pub code_font_size: f32,
    pub trace_font_size: f32,
    pub reduced_motion: bool,
    pub graph_scroll_sensitivity: f32,
    pub graph_show_grid: bool,
    pub graph_show_activity: bool,
    pub graph_show_summaries: bool,
    pub graph_all_agents: bool,
    pub dashboard_hours: u64,
    pub auto_refresh: bool,
    pub activity_refresh_seconds: u64,
    pub repository_refresh_seconds: u64,
    pub follow_latest: bool,
    pub show_timestamps: bool,
    pub trace_previews: bool,
    pub sidebar_agent_limit: usize,
    pub graph_animate_running: bool,
    pub graph_pulse_activity: bool,
    pub graph_animate_handoffs: bool,
    pub graph_flow_speed: f32,
    pub graph_grid_opacity: f32,
    pub graph_compact_nodes: bool,
    pub graph_show_models: bool,
    pub graph_show_session_ids: bool,
    pub graph_show_workspaces: bool,
    pub graph_show_status: bool,
    pub graph_show_sockets: bool,
    pub graph_show_flow_labels: bool,
    pub graph_show_launch_buttons: bool,
    pub graph_activity_samples: usize,
    pub graph_summary_chars: usize,
    pub show_activity_bar: bool,
    pub pulse_activity: bool,
    pub sidebar_show_workspace: bool,
    pub sidebar_show_session_ids: bool,
    pub sidebar_child_limit: usize,
    pub trace_compact_rows: bool,
    pub trace_preview_lines: usize,
    pub trace_preview_chars: usize,
    pub trace_show_kind_icons: bool,
    pub trace_show_separators: bool,
    pub viewer_zoom_step: f32,
    pub activity_lane_samples: usize,
    pub show_usage: bool,
    pub hide_workspace_paths: bool,
    pub monitor_only: bool,
    pub launch_write_access: bool,
    pub launch_deadline_minutes: u64,
    pub launch_show_context: bool,
    pub launch_title_from_task: bool,
    pub trace_line_height: f32,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            startup_view: 0,
            show_sidebar: true,
            open_inspector: false,
            code_font_size: 14.0,
            trace_font_size: 13.0,
            reduced_motion: false,
            graph_scroll_sensitivity: 1.0,
            graph_show_grid: true,
            graph_show_activity: true,
            graph_show_summaries: true,
            graph_all_agents: false,
            dashboard_hours: 24,
            auto_refresh: true,
            activity_refresh_seconds: 1,
            repository_refresh_seconds: 10,
            follow_latest: true,
            show_timestamps: true,
            trace_previews: true,
            sidebar_agent_limit: 3,
            graph_animate_running: true,
            graph_pulse_activity: true,
            graph_animate_handoffs: true,
            graph_flow_speed: 1.0,
            graph_grid_opacity: 1.0,
            graph_compact_nodes: false,
            graph_show_models: true,
            graph_show_session_ids: true,
            graph_show_workspaces: true,
            graph_show_status: true,
            graph_show_sockets: true,
            graph_show_flow_labels: true,
            graph_show_launch_buttons: true,
            graph_activity_samples: 24,
            graph_summary_chars: 160,
            show_activity_bar: true,
            pulse_activity: true,
            sidebar_show_workspace: true,
            sidebar_show_session_ids: true,
            sidebar_child_limit: 4,
            trace_compact_rows: false,
            trace_preview_lines: 2,
            trace_preview_chars: 400,
            trace_show_kind_icons: true,
            trace_show_separators: true,
            viewer_zoom_step: 1.0,
            activity_lane_samples: 24,
            show_usage: true,
            hide_workspace_paths: false,
            monitor_only: false,
            launch_write_access: false,
            launch_deadline_minutes: 10,
            launch_show_context: true,
            launch_title_from_task: false,
            trace_line_height: 1.0,
        }
    }
}

impl Preferences {
    pub fn normalize(&mut self) {
        self.startup_view = self.startup_view.min(8);
        self.code_font_size = finite_width(self.code_font_size, 14.0, 8.0, 32.0);
        self.trace_font_size = finite_width(self.trace_font_size, 13.0, 8.0, 24.0);
        self.graph_scroll_sensitivity = finite_width(self.graph_scroll_sensitivity, 1.0, 0.2, 3.0);
        if !matches!(self.dashboard_hours, 1 | 24) {
            self.dashboard_hours = 24;
        }
        self.activity_refresh_seconds = self.activity_refresh_seconds.clamp(1, 10);
        self.repository_refresh_seconds = self.repository_refresh_seconds.clamp(5, 120);
        self.sidebar_agent_limit = self.sidebar_agent_limit.clamp(1, 8);
        self.graph_flow_speed = finite_width(self.graph_flow_speed, 1.0, 0.25, 3.0);
        self.graph_grid_opacity = finite_width(self.graph_grid_opacity, 1.0, 0.1, 2.0);
        self.viewer_zoom_step = finite_width(self.viewer_zoom_step, 1.0, 0.5, 3.0);
        self.trace_line_height = finite_width(self.trace_line_height, 1.0, 0.8, 1.6);
        self.graph_activity_samples = self.graph_activity_samples.clamp(4, 24);
        self.graph_summary_chars = self.graph_summary_chars.clamp(40, 400);
        self.sidebar_child_limit = self.sidebar_child_limit.clamp(1, 16);
        self.trace_preview_lines = self.trace_preview_lines.clamp(1, 5);
        self.trace_preview_chars = self.trace_preview_chars.clamp(80, 2000);
        self.activity_lane_samples = self.activity_lane_samples.clamp(4, 24);
        self.launch_deadline_minutes = self.launch_deadline_minutes.clamp(1, 10);
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub project: String,
    pub worker: String,
    pub state_dir: String,
    pub appearance: String,
    pub sidebar_width: f32,
    pub dock_width: f32,
    pub preferences: Preferences,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            project: String::new(),
            worker: String::new(),
            state_dir: String::new(),
            appearance: "system".into(),
            sidebar_width: 228.0,
            dock_width: 400.0,
            preferences: Preferences::default(),
        }
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn settings_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    let directory = home().join("Library/Application Support/AgentWatcher");
    #[cfg(not(target_os = "macos"))]
    let directory = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".config"))
        .join("agentwatcher");
    directory.join("agentwatcher-rust.json")
}

fn absolute_setting(value: &str) -> String {
    if value.is_empty() || value.contains('\0') {
        return String::new();
    }
    let path = if value == "~" {
        home()
    } else if let Some(rest) = value.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(value)
    };
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| home())
            .join(path)
    };
    path.to_string_lossy().into_owned()
}

impl Settings {
    pub fn load() -> Self {
        // Read only this application's small configuration, never agent auth files.
        let loaded = read_regular(&settings_path(), 64 * 1024, false)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Self>(&bytes).ok());
        let mut settings = loaded.unwrap_or_else(Self::legacy_settings);
        settings.normalize();
        settings
    }

    fn legacy_settings() -> Self {
        let mut settings = Self::default();
        #[cfg(target_os = "macos")]
        {
            // Read individual preference keys. A domain-wide dump could contain
            // unrelated personal data, and Swift's preference file stays intact.
            let old = |key: &str| {
                run(
                    "/usr/bin/defaults",
                    &["read", "com.alikatgh.agentwatcher", key],
                    Some("/"),
                    Duration::from_secs(1),
                    4096,
                )
                .ok()
                .map(|s| s.trim_end_matches(['\n', '\r']).to_owned())
            };
            settings.project = old("project").unwrap_or_default();
            settings.worker = old("worker").unwrap_or_default();
            settings.state_dir = old("state").unwrap_or_default();
            settings.appearance = old("appearancePreference").unwrap_or_else(|| "system".into());
            settings.sidebar_width = old("workspace.sidebarWidth")
                .and_then(|s| s.parse().ok())
                .unwrap_or(228.0);
            settings.dock_width = old("workspace.inspectorWidth")
                .and_then(|s| s.parse().ok())
                .unwrap_or(400.0);
        }
        settings
    }

    pub fn normalize(&mut self) {
        self.project = absolute_setting(&self.project);
        self.worker = absolute_setting(&self.worker);
        self.state_dir = absolute_setting(&self.state_dir);
        if self.project.is_empty() {
            let cwd = std::env::current_dir().unwrap_or_else(|_| home());
            self.project = cwd
                .to_str()
                .and_then(|cwd| git(cwd, &["rev-parse", "--show-toplevel"], 4096).ok())
                .map(|s| s.trim_end_matches(['\n', '\r']).to_owned())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| cwd.to_string_lossy().into_owned());
        }
        if self.worker.is_empty() {
            self.worker = detect_worker().unwrap_or_default();
        }
        if self.state_dir.is_empty() {
            let name = Path::new(&self.worker)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .trim_end_matches("-loop");
            let derived = home().join(".local/state").join(name);
            self.state_dir = if !name.is_empty() && derived.is_dir() {
                derived
            } else {
                home().join(".local/state/agentwatcher")
            }
            .to_string_lossy()
            .into_owned();
        }
        if !crate::themes::valid_appearance(&self.appearance) {
            self.appearance = "system".into();
        }
        self.sidebar_width = finite_width(self.sidebar_width, 228.0, 190.0, 320.0);
        self.dock_width = finite_width(self.dock_width, 400.0, 300.0, 620.0);
        self.preferences.normalize();
    }

    pub fn save(&self) -> Result<(), String> {
        let mut normalized = self.clone();
        normalized.normalize();
        let bytes = serde_json::to_vec_pretty(&normalized).map_err(|e| e.to_string())?;
        atomic_settings_write(&settings_path(), &bytes)
    }
}

fn finite_width(value: f32, fallback: f32, minimum: f32, maximum: f32) -> f32 {
    if value.is_finite() {
        value.clamp(minimum, maximum)
    } else {
        fallback
    }
}

fn detect_worker() -> Option<String> {
    // One shallow directory, filename and metadata only; never read script content.
    let entries = fs::read_dir(home().join(".local/bin")).ok()?;
    entries
        .take(512)
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_lowercase();
            if !name.ends_with(".sh") || !(name.contains("loop") || name.contains("agent")) {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
                return None;
            }
            Some((
                metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                entry.path(),
            ))
        })
        .max_by_key(|(modified, _)| *modified)
        .and_then(|(_, path)| path.to_str().map(str::to_owned))
}

fn atomic_settings_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("Invalid settings path")?;
    fs::create_dir_all(parent).map_err(|e| format!("Could not create settings directory: {e}"))?;
    if fs::symlink_metadata(path).is_ok_and(|m| !m.is_file() || m.file_type().is_symlink()) {
        return Err("The Rust settings path is not a regular file.".into());
    }
    static SERIAL: AtomicUsize = AtomicUsize::new(0);
    let temporary = parent.join(format!(
        ".agentwatcher-rust.{}.{}.tmp",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temporary)
        .map_err(|e| format!("Could not prepare settings: {e}"))?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        // Only the create_new scratch file owned by this call is removed.
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|e: std::io::Error| format!("Could not save settings: {e}"))
}

#[derive(Clone, Debug, Default)]
pub struct Commit {
    pub hash: String,
    pub short_hash: String,
    pub subject: String,
    pub author: String,
    pub time: String,
}

#[derive(Clone, Debug, Default)]
pub struct Change {
    pub path: String,
    pub status: String,
    pub added: u32,
    pub removed: u32,
}

#[derive(Clone, Debug, Default)]
pub struct RepositorySnapshot {
    pub branch: String,
    pub commits: Vec<Commit>,
    pub changes: Vec<Change>,
    pub error: Option<String>,
}

fn git_args<'a>(project: &'a str, args: &[&'a str]) -> Vec<&'a str> {
    let mut argv = vec![
        "--no-pager",
        "--no-optional-locks",
        "--literal-pathspecs",
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.pager=cat",
        "-C",
        project,
    ];
    argv.extend_from_slice(args);
    argv
}

fn git(project: &str, args: &[&str], limit: usize) -> Result<String, String> {
    if project.is_empty() || !Path::new(project).is_absolute() {
        return Err("Choose an absolute project directory in Settings.".into());
    }
    run(
        "/usr/bin/git",
        &git_args(project, args),
        Some("/"),
        Duration::from_secs(8),
        limit,
    )
}

fn valid_hash(value: &str) -> bool {
    (4..=64).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn parse_commits(text: &str) -> Result<Vec<Commit>, String> {
    // NUL separates fields and records, so tabs/newlines in commit subjects do
    // not break framing. Git appends one record newline after each five fields.
    let fields: Vec<_> = text.split('\0').collect();
    let mut commits = Vec::new();
    for chunk in fields.chunks(5) {
        if chunk.len() != 5 {
            if chunk.iter().any(|s| !s.trim_matches('\n').is_empty()) {
                return Err("Git returned an incomplete commit record.".into());
            }
            break;
        }
        let hash = chunk[0].trim_start_matches('\n');
        if !valid_hash(hash) || hash.len() < 40 || !valid_hash(chunk[1]) {
            return Err("Git returned an invalid commit identifier.".into());
        }
        commits.push(Commit {
            hash: hash.into(),
            short_hash: chunk[1].into(),
            subject: chunk[2].into(),
            author: chunk[3].into(),
            time: chunk[4].into(),
        });
    }
    Ok(commits)
}

fn parse_status(text: &str) -> Result<Vec<Change>, String> {
    let mut changes = Vec::new();
    for record in text.split('\0').filter(|s| !s.is_empty()) {
        let bytes = record.as_bytes();
        if bytes.len() < 4 || bytes[2] != b' ' || !record.is_char_boundary(3) {
            return Err("Git returned an invalid change record.".into());
        }
        let status = &record[..2];
        if status.bytes().any(|b| !b" MADRCU?!T".contains(&b)) {
            return Err("Git returned an invalid change status.".into());
        }
        let path = &record[3..];
        validate_relative(path)?;
        changes.push(Change {
            path: path.into(),
            status: status.trim().into(),
            ..Change::default()
        });
        if changes.len() > MAX_CHANGES {
            return Err(format!(
                "This repository exceeds the {MAX_CHANGES}-file snapshot limit."
            ));
        }
    }
    Ok(changes)
}

fn parse_numstat(text: &str) -> Result<HashMap<String, (u32, u32)>, String> {
    let mut counts = HashMap::new();
    for record in text.split('\0').filter(|s| !s.is_empty()) {
        let fields: Vec<_> = record.splitn(3, '\t').collect();
        if fields.len() != 3 {
            return Err("Git returned an invalid change count.".into());
        }
        validate_relative(fields[2])?;
        let count = |value: &str| {
            if value == "-" {
                Ok(0)
            } else {
                value
                    .parse::<u32>()
                    .map_err(|_| "Git returned an invalid line count.".to_owned())
            }
        };
        counts.insert(fields[2].into(), (count(fields[0])?, count(fields[1])?));
    }
    Ok(counts)
}

pub fn repository(project: &str) -> RepositorySnapshot {
    let mut snapshot = RepositorySnapshot::default();
    let collect = (|| -> Result<(), String> {
        let status = git(
            project,
            &[
                "status",
                "--porcelain=v1",
                "-z",
                "--no-renames",
                "--untracked-files=all",
            ],
            REPOSITORY_LIMIT,
        )?;
        snapshot.changes = parse_status(&status)?;
        snapshot.branch = git(project, &["branch", "--show-current"], 4096)?
            .trim_end_matches(['\n', '\r'])
            .into();
        if snapshot.branch.is_empty() {
            snapshot.branch = "detached HEAD".into();
        }
        let has_head = git(project, &["rev-parse", "--verify", "HEAD"], 4096).is_ok();
        if has_head {
            let history = git(
                project,
                &[
                    "log",
                    "-40",
                    "--no-show-signature",
                    "--no-notes",
                    "--format=%H%x00%h%x00%s%x00%an%x00%cr%x00",
                ],
                REPOSITORY_LIMIT,
            )?;
            snapshot.commits = parse_commits(&history)?;
            let counts = parse_numstat(&git(
                project,
                &[
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--numstat",
                    "-z",
                    "--no-renames",
                    "HEAD",
                    "--",
                ],
                REPOSITORY_LIMIT,
            )?)?;
            for change in &mut snapshot.changes {
                if let Some((added, removed)) = counts.get(&change.path) {
                    change.added = *added;
                    change.removed = *removed;
                }
            }
        }
        // Count small new files without invoking Git once for every file. Large
        // or binary additions retain zero textual counts; opening their diff
        // gives an explicit binary/size explanation instead of following links.
        let mut budget = 4 * 1024 * 1024;
        for change in &mut snapshot.changes {
            if (change.status == "??" || !has_head) && budget > 0 {
                let path = safe_file_path(project, &change.path)?;
                if let Ok(metadata) = fs::symlink_metadata(&path) {
                    let size = metadata.len() as usize;
                    if size <= LOG_LIMIT && size <= budget {
                        if let Ok(bytes) = repository_content(project, &change.path, LOG_LIMIT) {
                            budget = budget.saturating_sub(bytes.len());
                            if !bytes.contains(&0) {
                                change.added = line_count(&bytes);
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    })();
    if let Err(error) = collect {
        snapshot.error = Some(error);
    }
    snapshot
}

pub fn commit_diff(project: &str, hash: &str) -> Result<String, String> {
    if !valid_hash(hash) {
        return Err("The selected commit identifier is invalid.".into());
    }
    git(
        project,
        &[
            "show",
            "--no-show-signature",
            "--no-notes",
            "--no-ext-diff",
            "--no-textconv",
            "--stat",
            "--patch",
            "--format=commit %h%nAuthor: %an%nDate: %ad%n%n%s%n%n%b",
            hash,
            "--",
        ],
        DIFF_LIMIT,
    )
}

fn validate_relative(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.contains('\0')
        || Path::new(path).is_absolute()
        || Path::new(path)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("The selected file must be a relative path inside the project.".into());
    }
    Ok(())
}

fn safe_file_path(project: &str, path: &str) -> Result<PathBuf, String> {
    validate_relative(path)?;
    let root = fs::canonicalize(project).map_err(|e| format!("Could not open project: {e}"))?;
    let relative = Path::new(path);
    let mut parent = root.clone();
    if let Some(components) = relative.parent() {
        for component in components.components() {
            parent.push(component);
            match fs::symlink_metadata(&parent) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(
                        "A parent of this file is a symbolic link; its contents are not read."
                            .into(),
                    );
                }
                Ok(metadata) if !metadata.is_dir() => {
                    return Err("A parent of this file is not a directory.".into());
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("Could not inspect file parent: {error}")),
                _ => {}
            }
        }
    }
    Ok(root.join(relative))
}

fn read_regular(path: &Path, maximum: usize, tail: bool) -> Result<Vec<u8>, String> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("The selected path is not a regular file.".into());
    }
    if !tail && metadata.len() > maximum as u64 {
        return Err(format!("File exceeds the {maximum}-byte read limit."));
    }
    if tail && metadata.len() > maximum as u64 {
        file.seek(SeekFrom::End(-(maximum as i64)))
            .map_err(|e| e.to_string())?;
    }
    let mut bytes = Vec::with_capacity((metadata.len() as usize).min(maximum));
    file.take(maximum as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > maximum {
        return Err(format!("File exceeds the {maximum}-byte read limit."));
    }
    Ok(bytes)
}

fn repository_content(project: &str, path: &str, maximum: usize) -> Result<Vec<u8>, String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    validate_relative(path)?;
    // Keep each directory open while traversing it. Unlike a preliminary stat
    // followed by an absolute open, this also prevents a concurrent symlink
    // replacement from routing a file read outside the selected repository.
    let root = fs::canonicalize(project).map_err(|e| e.to_string())?;
    let mut directory = File::open(root).map_err(|e| e.to_string())?;
    let components: Vec<_> = Path::new(path).components().collect();
    for component in &components[..components.len() - 1] {
        let name = CString::new(component.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd == -1 {
            return Err(format!(
                "Could not open file parent without following links: {}",
                std::io::Error::last_os_error()
            ));
        }
        directory = unsafe { File::from_raw_fd(fd) };
    }
    let name = CString::new(
        components
            .last()
            .ok_or("Empty file path")?
            .as_os_str()
            .as_bytes(),
    )
    .map_err(|e| e.to_string())?;
    let mut metadata: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            &mut metadata,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } == -1
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if metadata.st_mode & libc::S_IFMT == libc::S_IFLNK {
        // A broken or external symlink is shown as its link text, never followed.
        let mut bytes = vec![0u8; maximum.min(64 * 1024) + 1];
        let count = unsafe {
            libc::readlinkat(
                directory.as_raw_fd(),
                name.as_ptr(),
                bytes.as_mut_ptr() as *mut _,
                bytes.len(),
            )
        };
        if count < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        if count as usize == bytes.len() {
            return Err("Symbolic link text exceeds the read limit.".into());
        }
        bytes.truncate(count as usize);
        return Ok(bytes);
    }
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd == -1 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let file = unsafe { File::from_raw_fd(fd) };
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() {
        return Err("The selected path is not a regular file.".into());
    }
    if metadata.len() > maximum as u64 {
        return Err(format!("File exceeds the {maximum}-byte read limit."));
    }
    let mut bytes = Vec::with_capacity((metadata.len() as usize).min(maximum));
    file.take(maximum as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > maximum {
        return Err(format!("File exceeds the {maximum}-byte read limit."));
    }
    Ok(bytes)
}

fn line_count(bytes: &[u8]) -> u32 {
    (bytes.iter().filter(|&&b| b == b'\n').count()
        + usize::from(!bytes.is_empty() && bytes.last() != Some(&b'\n')))
    .min(u32::MAX as usize) as u32
}

fn new_file_diff(path: &str, bytes: &[u8]) -> String {
    if bytes.contains(&0) || std::str::from_utf8(bytes).is_err() {
        return format!("Binary file: {path}\nNo textual diff is available.");
    }
    let text = std::str::from_utf8(bytes).unwrap_or_default();
    let quoted = serde_json::to_string(&format!("b/{path}")).unwrap_or_default();
    let mut diff = format!(
        "--- /dev/null\n+++ {quoted}\n@@ -0,0 +1,{} @@\n",
        line_count(bytes)
    );
    for line in text.split_inclusive('\n') {
        diff.push('+');
        diff.push_str(line);
        if !line.ends_with('\n') {
            diff.push_str("\n\\ No newline at end of file\n");
        }
    }
    diff
}

pub fn file_diff(project: &str, path: &str) -> Result<String, String> {
    let absolute = safe_file_path(project, path)?;
    let status = git(
        project,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--no-renames",
            "--untracked-files=all",
            "--",
            path,
        ],
        REPOSITORY_LIMIT,
    )?;
    let changes = parse_status(&status)?;
    let untracked = changes.iter().any(|c| c.path == path && c.status == "??");
    let has_head = git(project, &["rev-parse", "--verify", "HEAD"], 4096).is_ok();
    let text = if untracked || !has_head {
        match fs::symlink_metadata(&absolute) {
            Ok(_) => new_file_diff(path, &repository_content(project, path, DIFF_LIMIT / 3)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error.to_string()),
        }
    } else {
        git(
            project,
            &[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                "HEAD",
                "--",
                path,
            ],
            DIFF_LIMIT,
        )?
    };
    if text.is_empty() {
        Ok(format!(
            "No textual diff for {path} (binary, mode change or edits that cancel out)."
        ))
    } else {
        Ok(text)
    }
}

#[derive(Clone, Debug, Default)]
pub struct ProcessInfo {
    pub pid: u32,
    pub ppid: u32,
    pub cpu: String,
    pub memory: String,
    pub elapsed: String,
    pub command: String,
    /// Opaque capture identity. Keep this value with the selected row through
    /// confirmation and stopping; a PID alone does not identify that selection.
    pub identity_token: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Identity {
    pid: u32,
    ppid: u32,
    started: (u64, u64),
    executable: PathBuf,
    cwd: PathBuf,
    command: String,
    argv: Vec<String>,
}

fn identity_token(identity: &Identity) -> String {
    let mut hash = DefaultHasher::new();
    identity.hash(&mut hash);
    // Keep the native creation timestamp explicit as well as hashing all native
    // identity fields. Tokens remain local to the current application process.
    format!(
        "aw1:{}:{}:{}:{:016x}",
        identity.pid,
        identity.started.0,
        identity.started.1,
        hash.finish()
    )
}

fn admit_selected_identity(selected: &ProcessInfo, identity: &Identity) -> Result<(), String> {
    if selected.pid != identity.pid
        || selected.identity_token.is_empty()
        || selected.identity_token != identity_token(identity)
    {
        return Err(
            "The selected process changed after selection. Refresh and select it again.".into(),
        );
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn native_arguments(pid: u32) -> Option<Vec<String>> {
    // KERN_PROCARGS2 is the system interface for another process's argv. Parse
    // exactly argc arguments; do not traverse, decode or retain the environment
    // block that follows them. No agent configuration or auth file is opened.
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as i32];
    let mut bytes = vec![0u8; 256 * 1024];
    let mut length = bytes.len();
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            bytes.as_mut_ptr() as *mut _,
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    } == -1
    {
        return None;
    }
    bytes.truncate(length);
    parse_mac_arguments(&bytes)
}

#[cfg(any(target_os = "macos", test))]
fn parse_mac_arguments(bytes: &[u8]) -> Option<Vec<String>> {
    if bytes.len() > 256 * 1024 {
        return None;
    }
    let count = i32::from_ne_bytes(bytes.get(..4)?.try_into().ok()?);
    if !(1..=4096).contains(&count) {
        return None;
    }
    let mut cursor = 4;
    cursor += bytes.get(cursor..)?.iter().position(|&b| b == 0)? + 1;
    while bytes.get(cursor) == Some(&0) {
        cursor += 1;
    }
    let mut argv = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let end = cursor + bytes.get(cursor..)?.iter().position(|&b| b == 0)?;
        argv.push(
            std::str::from_utf8(bytes.get(cursor..end)?)
                .ok()?
                .to_owned(),
        );
        cursor = end + 1;
    }
    Some(argv)
}

#[cfg(target_os = "linux")]
fn native_arguments(pid: u32) -> Option<Vec<String>> {
    let bytes = read_regular(
        &PathBuf::from(format!("/proc/{pid}/cmdline")),
        256 * 1024,
        false,
    )
    .ok()?;
    let bytes = bytes.strip_suffix(&[0])?;
    let arguments = bytes
        .split(|&byte| byte == 0)
        .map(|bytes| std::str::from_utf8(bytes).map(str::to_owned))
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    (arguments.len() <= 4096).then_some(arguments)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn native_arguments(_: u32) -> Option<Vec<String>> {
    None
}

#[derive(Clone, Debug)]
pub struct ProcessCommand {
    pub executable: PathBuf,
    pub argv: Vec<String>,
}

/// Read exact argv for a retained process candidate. This does not signal it or
/// open any configuration/auth file, and it never parses an environment block.
/// Native creation identity must still match the captured row on both sides.
pub fn process_command(process: &ProcessInfo) -> Option<ProcessCommand> {
    if process.pid <= 1 || process.pid > i32::MAX as u32 || process.identity_token.is_empty() {
        return None;
    }
    let before = identity(process, "")?;
    admit_selected_identity(process, &before).ok()?;
    if !matches!(executable_name(&before.executable), "codex" | "opencode") {
        return None;
    }
    let argv = native_arguments(process.pid)?;
    let after = identity(process, "")?;
    if before != after {
        return None;
    }
    admit_selected_identity(process, &after).ok()?;
    Some(ProcessCommand {
        executable: after.executable,
        argv,
    })
}

#[cfg(target_os = "macos")]
fn identity(process: &ProcessInfo, worker: &str) -> Option<Identity> {
    unsafe {
        let bsd = || {
            let mut info: libc::proc_bsdinfo = std::mem::zeroed();
            let size = std::mem::size_of_val(&info) as i32;
            let count = libc::proc_pidinfo(
                process.pid as i32,
                libc::PROC_PIDTBSDINFO,
                0,
                &mut info as *mut _ as *mut _,
                size,
            );
            (count == size && info.pbi_uid == libc::getuid()).then_some(info)
        };
        let before = bsd()?;
        let mut path = [0i8; 4096];
        if libc::proc_pidpath(
            process.pid as i32,
            path.as_mut_ptr() as *mut _,
            path.len() as u32,
        ) <= 0
        {
            return None;
        }
        let path_length = path.iter().position(|&c| c == 0)?;
        let path_bytes: Vec<_> = path[..path_length].iter().map(|&c| c as u8).collect();
        let executable = PathBuf::from(std::str::from_utf8(&path_bytes).ok()?);
        let mut vnode: libc::proc_vnodepathinfo = std::mem::zeroed();
        let size = std::mem::size_of_val(&vnode) as i32;
        let count = libc::proc_pidinfo(
            process.pid as i32,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            &mut vnode as *mut _ as *mut _,
            size,
        );
        let cwd = if count == size {
            let bytes: Vec<_> = vnode
                .pvi_cdir
                .vip_path
                .iter()
                .flatten()
                .map(|&c| c as u8)
                .take_while(|&c| c != 0)
                .collect();
            PathBuf::from(std::str::from_utf8(&bytes).ok()?)
        } else {
            PathBuf::new()
        };
        let argv = if !worker.is_empty()
            && process.command.contains(worker)
            && matches!(
                executable_name(&executable),
                "bash" | "sh" | "zsh" | "dash" | "ksh" | "caffeinate"
            ) {
            native_arguments(process.pid)?
        } else {
            Vec::new()
        };
        let after = bsd()?;
        if before.pbi_start_tvsec != after.pbi_start_tvsec
            || before.pbi_start_tvusec != after.pbi_start_tvusec
            || after.pbi_ppid != process.ppid
        {
            return None;
        }
        Some(Identity {
            pid: process.pid,
            ppid: process.ppid,
            started: (after.pbi_start_tvsec, after.pbi_start_tvusec),
            executable,
            cwd,
            command: process.command.clone(),
            argv,
        })
    }
}

#[cfg(target_os = "linux")]
fn identity(process: &ProcessInfo, worker: &str) -> Option<Identity> {
    let root = PathBuf::from(format!("/proc/{}", process.pid));
    if fs::metadata(&root).ok()?.uid() != unsafe { libc::getuid() } {
        return None;
    }
    let stat = read_regular(&root.join("stat"), 8192, false).ok()?;
    let stat = std::str::from_utf8(&stat).ok()?;
    let fields: Vec<_> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    let started = fields.get(19)?.parse().ok()?;
    let executable = fs::read_link(root.join("exe")).ok()?;
    let argv = if !worker.is_empty()
        && process.command.contains(worker)
        && matches!(
            executable_name(&executable),
            "bash" | "sh" | "zsh" | "dash" | "ksh"
        ) {
        native_arguments(process.pid)?
    } else {
        Vec::new()
    };
    Some(Identity {
        pid: process.pid,
        ppid: process.ppid,
        started: (started, 0),
        executable,
        cwd: fs::read_link(root.join("cwd")).ok()?,
        command: process.command.clone(),
        argv,
    })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn identity(_: &ProcessInfo, _: &str) -> Option<Identity> {
    None
}

fn parse_processes(text: &str) -> Vec<ProcessInfo> {
    text.lines()
        .filter_map(|line| {
            let mut remainder = line;
            let mut fields = Vec::new();
            for _ in 0..5 {
                remainder = remainder.trim_start_matches([' ', '\t']);
                let end = remainder.find([' ', '\t'])?;
                fields.push(&remainder[..end]);
                remainder = &remainder[end..];
            }
            let pid: u32 = fields[0].parse().ok()?;
            let ppid = fields[1].parse().ok()?;
            if pid <= 1
                || !fields[2].parse::<f32>().ok()?.is_finite()
                || !fields[3].parse::<f32>().ok()?.is_finite()
            {
                return None;
            }
            Some(ProcessInfo {
                pid,
                ppid,
                cpu: fields[2].into(),
                memory: fields[3].into(),
                elapsed: fields[4].into(),
                command: remainder.trim_start_matches([' ', '\t']).into(),
                identity_token: String::new(),
            })
        })
        .collect()
}

fn executable_name(path: &Path) -> &str {
    path.file_name().and_then(|s| s.to_str()).unwrap_or("")
}

fn is_agent_name(name: &str) -> bool {
    matches!(
        name,
        "codex" | "opencode" | "claude" | "aider" | "gemini" | "cursor-agent" | "deepseek"
    )
}

fn worker_argv(argv: &[String], worker: &str, depth: usize) -> bool {
    if worker.is_empty() || depth > 2 {
        return false;
    }
    let Some(program) = argv.first() else {
        return false;
    };
    if program == worker {
        return true;
    }
    let name = executable_name(Path::new(program));
    let caffeinate = name == "caffeinate";
    if !caffeinate && !matches!(name, "bash" | "sh" | "zsh" | "dash" | "ksh") {
        return false;
    }
    let mut index = 1;
    while let Some(flag) = argv.get(index).filter(|s| s.starts_with('-')) {
        if flag == "--" {
            index += 1;
            break;
        }
        if flag.len() <= 1 || flag.starts_with("--") {
            return false;
        }
        if caffeinate {
            if !flag[1..].bytes().all(|c| b"disu".contains(&c)) {
                return false;
            }
        } else if !flag[1..]
            .bytes()
            .all(|c| c.is_ascii_alphabetic() && !b"cs".contains(&c))
        {
            return false;
        }
        index += 1;
    }
    if caffeinate {
        worker_argv(&argv[index..], worker, depth + 1)
    } else {
        argv.get(index).is_some_and(|argument| argument == worker)
    }
}

fn worker_identity(id: &Identity, settings: &Settings) -> bool {
    let name = executable_name(&id.executable);
    if id.executable == Path::new(&settings.worker) && !settings.worker.is_empty() {
        return true;
    }
    matches!(name, "bash" | "sh" | "zsh" | "dash" | "ksh" | "caffeinate")
        && worker_argv(&id.argv, &settings.worker, 0)
}

fn worker_root(id: &Identity, settings: &Settings, project: &Path) -> bool {
    // The same loop script can serve several projects simultaneously. Its argv
    // proves script identity, while cwd establishes which watched project owns
    // this root; only descendants of these qualified roots inherit its scope.
    !id.cwd.as_os_str().is_empty() && id.cwd.starts_with(project) && worker_identity(id, settings)
}

fn descendant(pid: u32, roots: &HashSet<u32>, parents: &HashMap<u32, u32>) -> bool {
    let mut cursor = pid;
    for _ in 0..128 {
        if roots.contains(&cursor) {
            return true;
        }
        let Some(&parent) = parents.get(&cursor) else {
            break;
        };
        if parent <= 1 || parent == cursor {
            break;
        }
        cursor = parent;
    }
    false
}

fn process_scope(
    id: &Identity,
    settings: &Settings,
    project: &Path,
    worker_descendant: bool,
) -> bool {
    if id.pid <= 1 || id.pid == std::process::id() {
        return false;
    }
    if worker_root(id, settings, project) {
        return true;
    }
    is_agent_name(executable_name(&id.executable))
        && (worker_descendant || (!id.cwd.as_os_str().is_empty() && id.cwd.starts_with(project)))
}

type ScopedProcesses = Vec<(ProcessInfo, Identity)>;

struct ProcessSnapshot {
    identities: ScopedProcesses,
    parents: HashMap<u32, u32>,
    worker_roots: HashSet<u32>,
    project: PathBuf,
}

impl ProcessSnapshot {
    fn scoped(
        &self,
        settings: &Settings,
        worker_children_only: bool,
    ) -> Result<ScopedProcesses, String> {
        let scoped: Vec<_> = self
            .identities
            .iter()
            .filter(|(_, id)| {
                let child = descendant(id.pid, &self.worker_roots, &self.parents);
                if worker_children_only {
                    child
                } else {
                    process_scope(id, settings, &self.project, child)
                }
            })
            .take(513)
            .cloned()
            .collect();
        if scoped.len() > 512 {
            return Err("The configured scope exceeds the 512-process safety limit.".into());
        }
        Ok(scoped)
    }
}

fn collect_process_snapshot(settings: &Settings) -> Result<ProcessSnapshot, String> {
    let output = run(
        "/bin/ps",
        &["-axo", "pid=,ppid=,pcpu=,pmem=,etime=,command="],
        Some("/"),
        Duration::from_secs(3),
        REPOSITORY_LIMIT,
    )?;
    let rows = parse_processes(&output);
    let parents: HashMap<_, _> = rows.iter().map(|p| (p.pid, p.ppid)).collect();
    let mut protected = HashSet::new();
    let mut pid = std::process::id();
    for _ in 0..128 {
        protected.insert(pid);
        let Some(&parent) = parents.get(&pid) else {
            break;
        };
        if parent <= 1 || parent == pid {
            break;
        }
        pid = parent;
    }
    let root = fs::canonicalize(&settings.project)
        .map_err(|e| format!("Could not inspect project process scope: {e}"))?;
    // Native identity checks read executable, cwd and start time, never argv's
    // environment block or any credential file.
    let identities: Vec<_> = rows
        .into_iter()
        .filter(|p| !protected.contains(&p.pid))
        .filter_map(|p| identity(&p, &settings.worker).map(|id| (p, id)))
        .collect();
    let roots: HashSet<_> = identities
        .iter()
        .filter(|(_, id)| worker_root(id, settings, &root))
        .map(|(_, id)| id.pid)
        .collect();
    Ok(ProcessSnapshot {
        identities,
        parents,
        worker_roots: roots,
        project: root,
    })
}

fn collect_processes(
    settings: &Settings,
    worker_children_only: bool,
) -> Result<ScopedProcesses, String> {
    collect_process_snapshot(settings)?.scoped(settings, worker_children_only)
}

type ProcessKey = (String, String, u32);
fn process_cache() -> &'static Mutex<HashMap<ProcessKey, Identity>> {
    static CACHE: OnceLock<Mutex<HashMap<ProcessKey, Identity>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn processes(settings: &Settings) -> Result<Vec<ProcessInfo>, String> {
    let rows = collect_processes(settings, false)?;
    cache_processes(settings, rows)
}

fn cache_processes(settings: &Settings, rows: ScopedProcesses) -> Result<Vec<ProcessInfo>, String> {
    let mut cache = process_cache()
        .lock()
        .map_err(|_| "Process identity cache unavailable")?;
    cache.clear();
    for (_, identity) in &rows {
        cache.insert(
            (
                settings.project.clone(),
                settings.worker.clone(),
                identity.pid,
            ),
            identity.clone(),
        );
    }
    Ok(rows
        .into_iter()
        .map(|(mut row, identity)| {
            row.identity_token = identity_token(&identity);
            row
        })
        .collect())
}

fn terminate_checked(
    settings: &Settings,
    expected: &Identity,
    worker_only: bool,
    selected: Option<&ProcessInfo>,
) -> Result<(), String> {
    let current = collect_processes(settings, worker_only)?;
    let Some((row, id)) = current.iter().find(|(_, id)| id.pid == expected.pid) else {
        return Err("The selected process exited or left the configured scope. Refresh and select it again.".into());
    };
    if id != expected || identity(row, &settings.worker).as_ref() != Some(expected) {
        return Err("The selected process changed. Refresh and select it again.".into());
    }
    if let Some(selected) = selected {
        admit_selected_identity(selected, id)?;
    }
    if unsafe { libc::kill(expected.pid as i32, libc::SIGTERM) } == -1 {
        return Err(format!(
            "Could not send SIGTERM: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

pub fn stop_process(settings: &Settings, selected: &ProcessInfo) -> Result<(), String> {
    let pid = selected.pid;
    if pid <= 1 || pid > i32::MAX as u32 || pid == std::process::id() {
        return Err("That process cannot be stopped by AgentWatcher.".into());
    }
    let expected = process_cache()
        .lock()
        .map_err(|_| "Process identity cache unavailable")?
        .get(&(settings.project.clone(), settings.worker.clone(), pid))
        .cloned()
        .ok_or("Refresh Processes and select a current process before stopping it.")?;
    // A refresh may have replaced this cache entry with a new process reusing
    // the same PID. Validate the retained selection before collecting or sending
    // any signal, then revalidate that same token against the fresh identity.
    admit_selected_identity(selected, &expected)?;
    terminate_checked(settings, &expected, false, Some(selected))
}

#[derive(Clone, Debug, Default)]
pub struct WorkerState {
    pub label: String,
    pub running: bool,
    pub pausing: bool,
    pub log: String,
}

fn stop_marker(settings: &Settings) -> Result<PathBuf, String> {
    if settings.state_dir.is_empty()
        || !Path::new(&settings.state_dir).is_absolute()
        || settings.state_dir.contains('\0')
    {
        return Err("Choose an absolute worker state directory in Settings.".into());
    }
    let path = Path::new(&settings.state_dir).join("STOP");
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("The worker STOP marker is not a regular file.".into());
        }
    }
    Ok(path)
}

fn worker_state_from_processes(
    settings: &Settings,
    rows: &Result<ScopedProcesses, String>,
) -> WorkerState {
    let mut state = WorkerState::default();
    state.log = read_regular(
        &Path::new(&settings.state_dir).join("loop.log"),
        LOG_LIMIT,
        true,
    )
    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    .unwrap_or_default();
    let marker = stop_marker(settings);
    state.pausing = marker.as_ref().is_ok_and(|path| path.exists());
    if settings.worker.is_empty() {
        state.label = "No worker configured".into();
        return state;
    }
    match rows {
        Ok(rows) if marker.is_ok() => {
            state.running = !rows.is_empty();
            state.label = if Path::new(&settings.project)
                .join("agent-loop-REPORT.md")
                .exists()
            {
                "Needs attention"
            } else if state.running && state.pausing {
                "Pausing…"
            } else if state.running {
                "Working"
            } else if state.pausing {
                "Paused"
            } else {
                "Stopped"
            }
            .into();
        }
        _ => state.label = "Status unavailable".into(),
    }
    state
}

pub fn worker_state(settings: &Settings) -> WorkerState {
    let rows = if settings.worker.is_empty() {
        Ok(Vec::new())
    } else {
        collect_processes(settings, true)
    };
    worker_state_from_processes(settings, &rows)
}

/// Collect worker status and selectable processes from one native snapshot.
/// Worker descendants have their own scope and safety limit; the process list
/// continues to include only verified agents and configured worker roots.
pub fn status_snapshot(settings: &Settings) -> (WorkerState, Result<Vec<ProcessInfo>, String>) {
    status_snapshot_with(settings, collect_process_snapshot)
}

fn status_snapshot_with(
    settings: &Settings,
    collect: impl FnOnce(&Settings) -> Result<ProcessSnapshot, String>,
) -> (WorkerState, Result<Vec<ProcessInfo>, String>) {
    match collect(settings) {
        Ok(snapshot) => {
            let worker_rows = snapshot.scoped(settings, true);
            let state = worker_state_from_processes(settings, &worker_rows);
            let processes = snapshot
                .scoped(settings, false)
                .and_then(|rows| cache_processes(settings, rows));
            (state, processes)
        }
        Err(error) => {
            let state = worker_state_from_processes(settings, &Err(error.clone()));
            (state, Err(error))
        }
    }
}

fn clear_stop_marker(marker: &Path) -> Result<(), String> {
    match fs::symlink_metadata(marker) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            fs::remove_file(marker).map_err(|e| format!("Could not clear worker pause marker: {e}"))
        }
        Ok(_) => Err("The worker STOP marker is not a regular file.".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

pub fn worker_action(settings: &Settings, action: &str) -> Result<(), String> {
    if !matches!(action, "start" | "pause" | "resume" | "stop") {
        return Err("Unknown worker action.".into());
    }
    static ACTION: Mutex<()> = Mutex::new(());
    let _lock = ACTION.lock().map_err(|_| "Worker control is unavailable")?;
    if settings.worker.is_empty() {
        return Err("Choose a worker script in Settings first.".into());
    }
    let marker = stop_marker(settings)?;
    let existing = collect_processes(settings, true)?;
    if action == "stop" {
        if existing.is_empty() {
            return Ok(());
        }
        // Terminate the configured worker and its verified descendants, rather
        // than every process whose name happens to contain the script's name.
        // Children go first so stopping the parent does not destroy ancestry.
        let parents: HashMap<_, _> = existing.iter().map(|(_, id)| (id.pid, id.ppid)).collect();
        let depth = |pid: u32| {
            let mut cursor = pid;
            let mut count = 0;
            for _ in 0..128 {
                let Some(&parent) = parents.get(&cursor) else {
                    break;
                };
                cursor = parent;
                count += 1;
            }
            count
        };
        let mut ordered: Vec<_> = existing.iter().map(|(_, id)| id).collect();
        ordered.sort_by_key(|id| std::cmp::Reverse(depth(id.pid)));
        let mut errors = Vec::new();
        for id in ordered {
            if let Err(error) = terminate_checked(settings, id, true, None) {
                errors.push(error);
            }
        }
        return if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("\n"))
        };
    }
    if action == "pause" {
        fs::create_dir_all(&settings.state_dir)
            .map_err(|e| format!("Could not create worker state directory: {e}"))?;
        return match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(marker)
        {
            Ok(_) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                stop_marker(settings).map(|_| ())
            }
            Err(error) => Err(format!("Could not pause worker: {error}")),
        };
    }
    if !existing.is_empty() {
        return if action == "resume" {
            clear_stop_marker(&marker)
        } else {
            Err("The configured worker is already running.".into())
        };
    }
    // Validate paths and permissions without executing any worker command.
    let worker = fs::canonicalize(&settings.worker)
        .map_err(|e| format!("The configured worker is unavailable: {e}"))?;
    let metadata = fs::metadata(&worker).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err("The configured worker must be an executable regular file.".into());
    }
    if !Path::new(&settings.project).is_dir() {
        return Err("The configured project directory is unavailable.".into());
    }
    fs::create_dir_all(&settings.state_dir)
        .map_err(|e| format!("Could not create worker state directory: {e}"))?;
    clear_stop_marker(&marker)?;
    let mut command;
    #[cfg(target_os = "macos")]
    {
        command = Command::new("/usr/bin/caffeinate");
        command.args(["-is", "/bin/bash", &settings.worker]);
    }
    #[cfg(not(target_os = "macos"))]
    {
        command = Command::new("/bin/bash");
        command.arg(&settings.worker);
    }
    command
        .current_dir(&settings.project)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            libc::signal(libc::SIGHUP, libc::SIG_IGN);
            Ok(())
        });
    }
    let child = command
        .spawn()
        .map_err(|e| format!("Could not start worker: {e}"))?;
    reap_later(child, None);
    Ok(())
}

struct CommandSlot;
static ACTIVE_COMMANDS: AtomicUsize = AtomicUsize::new(0);
impl CommandSlot {
    fn acquire() -> Result<Self, String> {
        ACTIVE_COMMANDS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < COMMAND_LIMIT).then_some(n + 1)
            })
            .map(|_| Self)
            .map_err(|_| {
                "Command concurrency limit reached (16 running or unreaped children).".into()
            })
    }
}
impl Drop for CommandSlot {
    fn drop(&mut self) {
        ACTIVE_COMMANDS.fetch_sub(1, Ordering::AcqRel);
    }
}

fn reap_later(mut child: Child, slot: Option<CommandSlot>) {
    // At most sixteen command reapers can exist. Detached worker reapers are
    // created only by an explicit, serialized user action, never by collectors.
    thread::spawn(move || {
        let _slot = slot;
        loop {
            match child.try_wait() {
                Ok(Some(_)) | Err(_) => break,
                Ok(None) => thread::sleep(Duration::from_millis(100)),
            }
        }
    });
}

fn nonblocking(fd: RawFd) -> Result<(), String> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(format!(
            "Could not prepare command output: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

fn observe_exit(pid: u32) -> Result<bool, String> {
    let mut information: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            &mut information,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result == 0 {
        return Ok(unsafe { information.si_pid() } == pid as i32);
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::Interrupted {
        return Ok(false);
    }
    Err(format!("Could not observe command exit: {error}"))
}

fn drain<T: Read>(
    pipe: &mut T,
    closed: &mut bool,
    output: &mut Vec<u8>,
    maximum: usize,
) -> Result<(), String> {
    let mut buffer = [0u8; 8192];
    // Bounded passes ensure a child printing forever cannot starve its timeout.
    for _ in 0..16 {
        if *closed {
            break;
        }
        match pipe.read(&mut buffer) {
            Ok(0) => {
                *closed = true;
                break;
            }
            Ok(count) => {
                let retained = count.min(maximum.saturating_sub(output.len()));
                output.extend_from_slice(&buffer[..retained]);
                if retained < count {
                    return Err(format!(
                        "Command output exceeded the {maximum}-byte capture limit."
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(format!("Could not read command output: {error}")),
        }
    }
    Ok(())
}

/// Execute a bounded command. Nonzero exits, output overflow and timeouts are
/// errors. Successful commands return stdout; diagnostics on stderr cannot
/// corrupt structured output. Both streams share one capture bound and are
/// drained concurrently. Timeout kills the owned process group before the child
/// PID is reaped, including pipe-holding descendants.
pub fn run(
    executable: &str,
    args: &[&str],
    cwd: Option<&str>,
    timeout: Duration,
    max_bytes: usize,
) -> Result<String, String> {
    if timeout.is_zero() || max_bytes == 0 || max_bytes > MAX_CAPTURE {
        return Err(
            "Command requires a positive timeout and a capture limit between 1 byte and 32 MiB."
                .into(),
        );
    }
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or("Command timeout is too large.")?;
    if !Path::new(executable).is_absolute()
        || executable.contains('\0')
        || args.iter().any(|s| s.contains('\0'))
        || cwd.is_some_and(|s| s.contains('\0'))
    {
        return Err(
            "Command paths must be absolute and arguments cannot contain null bytes.".into(),
        );
    }
    let slot = CommandSlot::acquire()?;
    let mut command = Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    command
        .env("LC_ALL", "C")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0");
    if executable_name(Path::new(executable)) == "git" {
        // Environment overrides must not silently route a project read into a
        // different worktree, inject config or execute a custom diff command.
        command
            .env_clear()
            .env("HOME", home())
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("LC_ALL", "C")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
    }
    // A pre_exec hook forces Rust to fork, even when it only calls setpgid.
    // Keep macOS's posix_spawn path available in the multithreaded GPUI process
    // while retaining PGID=PID for bounded termination of owned descendants.
    command.process_group(0);
    let child = command
        .spawn()
        .map_err(|e| format!("Could not launch {executable}: {e}"))?;
    capture_command(child, slot, executable, timeout, deadline, max_bytes)
}

fn capture_command(
    mut child: Child,
    slot: CommandSlot,
    executable: &str,
    timeout: Duration,
    deadline: Instant,
    max_bytes: usize,
) -> Result<String, String> {
    let pid = child.id();
    let mut stdout = child.stdout.take().ok_or("Command stdout unavailable")?;
    let mut stderr = child.stderr.take().ok_or("Command stderr unavailable")?;
    let prepared = nonblocking(stdout.as_raw_fd()).and_then(|_| nonblocking(stderr.as_raw_fd()));
    let pipe_ready = prepared.is_ok();
    let mut failure = prepared.err();
    let mut stdout_output = Vec::with_capacity(max_bytes.min(64 * 1024));
    let mut stderr_output = Vec::new();
    let mut closed_out = false;
    let mut closed_err = false;
    let mut term_at = None;
    let mut kill_at = None;
    let mut ownership_lost = false;
    loop {
        // A pause during observation must not turn that stale pre-deadline
        // observation into a timeout; the next pass checks readiness again.
        let checked_at = Instant::now();
        let exited = match observe_exit(pid) {
            Ok(exited) => exited,
            Err(error) => {
                failure.get_or_insert(error);
                ownership_lost = true;
                break;
            }
        };
        if pipe_ready {
            if let Err(error) = drain(
                &mut stdout,
                &mut closed_out,
                &mut stdout_output,
                max_bytes.saturating_sub(stderr_output.len()),
            ) {
                failure.get_or_insert(error);
            }
            if let Err(error) = drain(
                &mut stderr,
                &mut closed_err,
                &mut stderr_output,
                max_bytes.saturating_sub(stdout_output.len()),
            ) {
                failure.get_or_insert(error);
            }
        }
        // Readiness wins over an expired deadline: the parent may have been
        // descheduled after a successful child finished and closed its pipes.
        // Overflow/read failures and earlier cleanup still take precedence.
        if failure.is_none() && term_at.is_none() && exited && closed_out && closed_err {
            break;
        }
        let now = Instant::now();
        if term_at.is_none() && (failure.is_some() || checked_at >= deadline) {
            if failure.is_none() {
                failure = Some(format!(
                    "Command timed out after {:.2} seconds: {executable}",
                    timeout.as_secs_f64()
                ));
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
                    "Could not wait for command output: {}",
                    std::io::Error::last_os_error()
                )
            });
        }
    }
    let status = if ownership_lost {
        None
    } else {
        match child.try_wait() {
            Ok(Some(status)) => Some(status),
            Ok(None) => {
                reap_later(child, Some(slot));
                return Err(failure.unwrap_or_else(|| {
                    "Command could not be reaped within its cleanup deadline.".into()
                }));
            }
            Err(error) => {
                failure.get_or_insert_with(|| format!("Could not reap command: {error}"));
                None
            }
        }
    };
    let stdout = String::from_utf8(stdout_output).map_err(|_| {
        "Command returned invalid UTF-8; paths were not decoded lossily.".to_owned()
    })?;
    if failure.is_none() && status.is_some_and(|status| status.success()) {
        return Ok(stdout);
    }
    let stderr = String::from_utf8_lossy(&stderr_output);
    let text = if stderr.is_empty() {
        stdout
    } else if stdout.is_empty() {
        stderr.into_owned()
    } else {
        format!("{stdout}\n{stderr}")
    };
    if let Some(error) = failure {
        return Err(if text.is_empty() {
            error
        } else {
            format!("{error}\n\nPartial command output:\n{text}")
        });
    }
    Err(if text.is_empty() {
        format!("Command failed: {executable}")
    } else {
        text
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_without_preferences_keep_existing_paths_and_gain_defaults() {
        let settings: Settings = serde_json::from_str(
            r#"{"project":"/old/workspace","worker":"/old/worker.sh","state_dir":"/old/state","appearance":"dark","sidebar_width":250,"dock_width":420}"#,
        )
        .unwrap();
        assert_eq!(settings.project, "/old/workspace");
        assert_eq!(settings.worker, "/old/worker.sh");
        assert_eq!(settings.state_dir, "/old/state");
        assert_eq!(settings.appearance, "dark");
        assert_eq!(settings.sidebar_width, 250.0);
        assert_eq!(settings.dock_width, 420.0);
        assert_eq!(settings.preferences, Preferences::default());

        let partial: Settings =
            serde_json::from_str(r#"{"preferences":{"show_sidebar":false,"code_font_size":17}}"#)
                .unwrap();
        assert!(!partial.preferences.show_sidebar);
        assert_eq!(partial.preferences.code_font_size, 17.0);
        assert_eq!(partial.preferences.repository_refresh_seconds, 10);
        assert!(partial.preferences.trace_previews);
    }

    #[test]
    fn preferences_are_finite_bounded_and_round_trip() {
        let mut preferences = Preferences {
            startup_view: usize::MAX,
            code_font_size: f32::NAN,
            trace_font_size: f32::INFINITY,
            graph_scroll_sensitivity: f32::NEG_INFINITY,
            dashboard_hours: 2,
            activity_refresh_seconds: 0,
            repository_refresh_seconds: u64::MAX,
            sidebar_agent_limit: usize::MAX,
            ..Preferences::default()
        };
        preferences.normalize();
        assert_eq!(preferences.startup_view, 8);
        assert_eq!(preferences.code_font_size, 14.0);
        assert_eq!(preferences.trace_font_size, 13.0);
        assert_eq!(preferences.graph_scroll_sensitivity, 1.0);
        assert_eq!(preferences.dashboard_hours, 24);
        assert_eq!(preferences.activity_refresh_seconds, 1);
        assert_eq!(preferences.repository_refresh_seconds, 120);
        assert_eq!(preferences.sidebar_agent_limit, 8);
        let encoded = serde_json::to_string(&preferences).unwrap();
        assert!(!encoded.contains("null"));
        assert_eq!(
            serde_json::from_str::<Preferences>(&encoded).unwrap(),
            preferences
        );

        preferences.code_font_size = 100.0;
        preferences.trace_font_size = 0.0;
        preferences.graph_scroll_sensitivity = 100.0;
        preferences.activity_refresh_seconds = u64::MAX;
        preferences.repository_refresh_seconds = 0;
        preferences.sidebar_agent_limit = 0;
        preferences.dashboard_hours = 1;
        preferences.normalize();
        assert_eq!(preferences.code_font_size, 32.0);
        assert_eq!(preferences.trace_font_size, 8.0);
        assert_eq!(preferences.graph_scroll_sensitivity, 3.0);
        assert_eq!(preferences.activity_refresh_seconds, 10);
        assert_eq!(preferences.repository_refresh_seconds, 5);
        assert_eq!(preferences.sidebar_agent_limit, 1);
        assert_eq!(preferences.dashboard_hours, 1);
    }

    #[test]
    fn expanded_preferences_migrate_partial_files_without_enabling_launch_access() {
        let loaded: Preferences = serde_json::from_str(
            r#"{"graph_show_grid":false,"trace_font_size":15,"monitor_only":true}"#,
        )
        .unwrap();
        assert!(!loaded.graph_show_grid);
        assert_eq!(loaded.trace_font_size, 15.0);
        assert!(loaded.monitor_only);
        assert!(!loaded.launch_write_access);
        assert!(!loaded.launch_title_from_task);
        assert_eq!(loaded.launch_deadline_minutes, 10);
        assert_eq!(loaded.graph_activity_samples, 24);
        assert_eq!(loaded.trace_preview_lines, 2);
        assert_eq!(loaded.trace_preview_chars, 400);
        assert_eq!(loaded.graph_flow_speed, 1.0);
        assert_eq!(loaded.sidebar_child_limit, 4);
        let restored: Preferences =
            serde_json::from_str(&serde_json::to_string(&loaded).unwrap()).unwrap();
        assert_eq!(restored, loaded);
    }

    #[test]
    fn expanded_preferences_bound_work_and_recover_nonfinite_values() {
        let mut preferences = Preferences {
            graph_flow_speed: f32::NAN,
            graph_grid_opacity: f32::INFINITY,
            viewer_zoom_step: f32::NEG_INFINITY,
            trace_line_height: f32::NAN,
            graph_activity_samples: usize::MAX,
            graph_summary_chars: usize::MAX,
            sidebar_child_limit: usize::MAX,
            trace_preview_lines: usize::MAX,
            trace_preview_chars: usize::MAX,
            activity_lane_samples: usize::MAX,
            launch_deadline_minutes: u64::MAX,
            ..Preferences::default()
        };
        preferences.normalize();
        assert_eq!(preferences.graph_flow_speed, 1.0);
        assert_eq!(preferences.graph_grid_opacity, 1.0);
        assert_eq!(preferences.viewer_zoom_step, 1.0);
        assert_eq!(preferences.trace_line_height, 1.0);
        assert_eq!(preferences.graph_activity_samples, 24);
        assert_eq!(preferences.graph_summary_chars, 400);
        assert_eq!(preferences.sidebar_child_limit, 16);
        assert_eq!(preferences.trace_preview_lines, 5);
        assert_eq!(preferences.trace_preview_chars, 2000);
        assert_eq!(preferences.activity_lane_samples, 24);
        assert_eq!(preferences.launch_deadline_minutes, 10);
        assert!(
            !serde_json::to_string(&preferences)
                .unwrap()
                .contains("null")
        );

        preferences.graph_flow_speed = 0.0;
        preferences.graph_grid_opacity = 0.0;
        preferences.viewer_zoom_step = 0.0;
        preferences.trace_line_height = 0.0;
        preferences.graph_activity_samples = 0;
        preferences.graph_summary_chars = 0;
        preferences.sidebar_child_limit = 0;
        preferences.trace_preview_lines = 0;
        preferences.trace_preview_chars = 0;
        preferences.activity_lane_samples = 0;
        preferences.launch_deadline_minutes = 0;
        preferences.normalize();
        assert_eq!(preferences.graph_flow_speed, 0.25);
        assert_eq!(preferences.graph_grid_opacity, 0.1);
        assert_eq!(preferences.viewer_zoom_step, 0.5);
        assert_eq!(preferences.trace_line_height, 0.8);
        assert_eq!(preferences.graph_activity_samples, 4);
        assert_eq!(preferences.graph_summary_chars, 40);
        assert_eq!(preferences.sidebar_child_limit, 1);
        assert_eq!(preferences.trace_preview_lines, 1);
        assert_eq!(preferences.trace_preview_chars, 80);
        assert_eq!(preferences.activity_lane_samples, 4);
        assert_eq!(preferences.launch_deadline_minutes, 1);
    }

    struct TestDirectory(PathBuf);
    impl TestDirectory {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "agentwatcher-backend-test-{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            // This private create_dir path and all fixtures beneath it belong
            // exclusively to this test; user repositories are never cleaned.
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn repository_file_reads_preserve_links_and_reject_parent_links() {
        use std::os::unix::fs::symlink;
        let scratch = TestDirectory::new();
        let project = scratch.path().join("project");
        let outside = scratch.path().join("outside");
        fs::create_dir(&project).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("private.txt"), b"not repository content").unwrap();
        symlink(&outside, project.join("linked-parent")).unwrap();
        symlink(outside.join("private.txt"), project.join("direct-link")).unwrap();
        symlink("missing-target", project.join("broken-link")).unwrap();
        let root = project.to_str().unwrap();
        assert!(repository_content(root, "linked-parent/private.txt", 4096).is_err());
        assert_eq!(
            repository_content(root, "direct-link", 4096).unwrap(),
            outside.join("private.txt").as_os_str().as_encoded_bytes()
        );
        assert_eq!(
            repository_content(root, "broken-link", 4096).unwrap(),
            b"missing-target"
        );
        fs::write(project.join("tab\tand\nnewline"), b"one\ntwo\n").unwrap();
        assert_eq!(
            repository_content(root, "tab\tand\nnewline", 4096).unwrap(),
            b"one\ntwo\n"
        );
        assert!(repository_content(root, "tab\tand\nnewline", 2).is_err());
        assert_eq!(
            read_regular(&project.join("tab\tand\nnewline"), 4, true).unwrap(),
            b"two\n"
        );
    }

    #[test]
    fn real_git_snapshot_and_diff_use_literal_paths_and_include_staged_edits() {
        let scratch = TestDirectory::new();
        let root = scratch.path().to_str().unwrap();
        git(root, &["init", "-q"], 4096).unwrap();
        let path = ":(glob)*\tfile\nname";
        fs::write(scratch.path().join(path), b"base\n").unwrap();
        git(root, &["add", "--", path], 4096).unwrap();
        git(
            root,
            &[
                "-c",
                "user.name=Backend Test",
                "-c",
                "user.email=backend-test@localhost",
                "commit",
                "-q",
                "-m",
                "baseline",
            ],
            4096,
        )
        .unwrap();
        fs::write(scratch.path().join(path), b"staged\n").unwrap();
        git(root, &["add", "--", path], 4096).unwrap();
        fs::write(scratch.path().join(path), b"staged\nunstaged\n").unwrap();
        fs::write(scratch.path().join("-untracked"), b"new file\n").unwrap();
        let snapshot = repository(root);
        assert!(snapshot.error.is_none(), "{:?}", snapshot.error);
        assert_eq!(snapshot.commits.len(), 1);
        let changed = snapshot.changes.iter().find(|c| c.path == path).unwrap();
        assert_eq!((changed.added, changed.removed), (2, 1));
        let diff = file_diff(root, path).unwrap();
        assert!(diff.contains("-base"));
        assert!(diff.contains("+staged"));
        assert!(diff.contains("+unstaged"));
        assert!(file_diff(root, "-untracked").unwrap().contains("+new file"));
        assert!(
            commit_diff(root, &snapshot.commits[0].hash)
                .unwrap()
                .contains("baseline")
        );
        assert!(commit_diff(root, "--help").is_err());
    }

    #[test]
    fn empty_repository_and_broken_new_link_are_readable() {
        use std::os::unix::fs::symlink;
        let scratch = TestDirectory::new();
        let root = scratch.path().to_str().unwrap();
        git(root, &["init", "-q"], 4096).unwrap();
        fs::write(scratch.path().join("first"), b"first\n").unwrap();
        git(root, &["add", "--", "first"], 4096).unwrap();
        symlink("does-not-exist", scratch.path().join("broken")).unwrap();
        let snapshot = repository(root);
        assert!(snapshot.error.is_none(), "{:?}", snapshot.error);
        assert!(snapshot.commits.is_empty());
        assert_eq!(
            snapshot
                .changes
                .iter()
                .find(|c| c.path == "first")
                .unwrap()
                .added,
            1
        );
        assert!(
            file_diff(root, "broken")
                .unwrap()
                .contains("+does-not-exist")
        );
    }

    #[test]
    fn tracked_file_patch_larger_than_legacy_limit_is_readable() {
        let scratch = TestDirectory::new();
        let root = scratch.path().to_str().unwrap();
        let path = "large-source.txt";
        git(root, &["init", "-q"], 4096).unwrap();
        fs::write(scratch.path().join(path), b"original\n").unwrap();
        git(root, &["add", "--", path], 4096).unwrap();
        git(
            root,
            &[
                "-c",
                "user.name=Backend Test",
                "-c",
                "user.email=backend-test@localhost",
                "commit",
                "-q",
                "-m",
                "baseline",
            ],
            4096,
        )
        .unwrap();
        let content = format!(
            "{}last synthetic line\n",
            format!("{}\n", "synthetic ".repeat(10)).repeat(2_000)
        );
        fs::write(scratch.path().join(path), content).unwrap();
        let patch = file_diff(root, path).unwrap();
        assert!(patch.len() > 160 * 1024);
        assert!(patch.len() < DIFF_LIMIT);
        assert!(patch.starts_with("diff --git"));
        assert!(patch.contains("-original\n"));
        assert!(patch.ends_with("+last synthetic line\n"));
    }

    #[test]
    fn git_framing_preserves_weird_paths_and_subjects() {
        let status = " M filename\twith\nnewlines\0?? -option:(glob)*.txt\0";
        let changes = parse_status(status).unwrap();
        assert_eq!(changes[0].path, "filename\twith\nnewlines");
        assert_eq!(changes[1].path, "-option:(glob)*.txt");
        let stats = parse_numstat("3\t2\tfilename\twith\nnewlines\0-\t-\tbinary\0").unwrap();
        assert_eq!(stats["filename\twith\nnewlines"], (3, 2));
        let hash = "a".repeat(40);
        let commits = parse_commits(&format!(
            "{hash}\0aaaaaaa\0subject\ttext\0Author\0a minute ago\0\n"
        ))
        .unwrap();
        assert_eq!(commits[0].subject, "subject\ttext");
        assert!(parse_commits("bad\0hash\0x\0y\0z\0").is_err());
    }

    #[test]
    fn path_arguments_are_literal_and_cannot_escape() {
        for path in ["", "../secret", "a/../../secret", "/etc/passwd", "a\0b"] {
            assert!(validate_relative(path).is_err(), "{path:?}");
        }
        for path in ["-n", ":(glob)**", "normal file", "line\nfile", "tab\tfile"] {
            assert!(validate_relative(path).is_ok(), "{path:?}");
        }
        let args = git_args("/project with spaces", &["diff", "HEAD", "--", ":(glob)**"]);
        assert!(args.contains(&"--literal-pathspecs"));
        assert!(args.contains(&"--no-optional-locks"));
        assert!(args.contains(&"core.hooksPath=/dev/null"));
        assert_eq!(&args[args.len() - 2..], &["--", ":(glob)**"]);
    }

    #[test]
    fn worker_matching_excludes_interpolation_and_nearby_names() {
        let worker = "/workers/a b-loop.sh";
        let matches = |argv: &[&str]| {
            worker_argv(
                &argv.iter().map(|s| (*s).into()).collect::<Vec<_>>(),
                worker,
                0,
            )
        };
        assert!(matches(&["/bin/bash", worker]));
        assert!(matches(&[
            "/usr/bin/caffeinate",
            "-is",
            "/bin/bash",
            worker
        ]));
        assert!(!matches(&["/bin/bash", "-c", worker]));
        assert!(!matches(&["/bin/bash", "/workers/a b-loop.sh.bak"]));
        assert!(!matches(&["/bin/echo", worker]));
        assert!(!matches(&["/bin/bash", "/other/loop.sh", worker]));
        // These argv print identically in ps, but only the exact first script
        // argument belongs to the configured worker.
        assert!(!matches(&["/bin/bash", "/workers/a", "b-loop.sh"]));
        assert!(!worker_argv(&["/bin/bash".into(), worker.into()], "", 0));
    }

    #[test]
    fn process_scope_requires_known_executable_and_related_project() {
        let settings = Settings {
            project: "/project".into(),
            worker: "/worker.sh".into(),
            ..Settings::default()
        };
        let mut id = Identity {
            pid: u32::MAX - 1,
            ppid: 1,
            started: (1, 0),
            executable: "/usr/bin/opencode".into(),
            cwd: "/project/subdir".into(),
            command: "opencode".into(),
            argv: Vec::new(),
        };
        assert!(process_scope(&id, &settings, Path::new("/project"), false));
        id.cwd = "/project-other".into();
        assert!(!process_scope(&id, &settings, Path::new("/project"), false));
        assert!(process_scope(&id, &settings, Path::new("/project"), true));
        id.executable = "/bin/echo".into();
        id.command = "echo opencode /worker.sh".into();
        assert!(!process_scope(&id, &settings, Path::new("/project"), true));
        id.pid = std::process::id();
        assert!(!process_scope(&id, &settings, Path::new("/project"), true));
        assert!(!is_agent_name("AgentWatcher"));
        assert!(!is_agent_name("opencode-server-unrelated"));
    }

    #[test]
    fn identical_worker_argv_in_another_project_does_not_qualify_its_tree() {
        let project = Path::new("/projects/a");
        let settings = Settings {
            project: project.to_str().unwrap().into(),
            worker: "/workers/shared-loop.sh".into(),
            ..Settings::default()
        };
        let worker_a = Identity {
            pid: u32::MAX - 10,
            ppid: 1,
            started: (1, 0),
            executable: "/bin/bash".into(),
            cwd: "/projects/a".into(),
            command: "/bin/bash /workers/shared-loop.sh".into(),
            argv: vec!["/bin/bash".into(), settings.worker.clone()],
        };
        let worker_b = Identity {
            pid: u32::MAX - 9,
            cwd: "/projects/b".into(),
            ..worker_a.clone()
        };
        assert!(worker_identity(&worker_a, &settings));
        assert!(worker_identity(&worker_b, &settings));
        assert!(process_scope(&worker_a, &settings, project, false));
        assert!(!process_scope(&worker_b, &settings, project, false));
        let roots: HashSet<_> = [&worker_a, &worker_b]
            .into_iter()
            .filter(|id| worker_root(id, &settings, project))
            .map(|id| id.pid)
            .collect();
        assert_eq!(roots, HashSet::from([worker_a.pid]));
        let child_a = u32::MAX - 8;
        let child_b = u32::MAX - 7;
        let parents = HashMap::from([
            (worker_a.pid, 1),
            (worker_b.pid, 1),
            (child_a, worker_a.pid),
            (child_b, worker_b.pid),
        ]);
        let scoped: Vec<_> = [worker_a.pid, worker_b.pid, child_a, child_b]
            .into_iter()
            .filter(|pid| descendant(*pid, &roots, &parents))
            .collect();
        assert_eq!(scoped, vec![worker_a.pid, child_a]);
    }

    fn process_snapshot_fixture(settings: &Settings, identities: Vec<Identity>) -> ProcessSnapshot {
        let project = PathBuf::from(&settings.project);
        ProcessSnapshot {
            parents: identities.iter().map(|id| (id.pid, id.ppid)).collect(),
            worker_roots: identities
                .iter()
                .filter(|id| worker_root(id, settings, &project))
                .map(|id| id.pid)
                .collect(),
            identities: identities
                .into_iter()
                .map(|id| {
                    (
                        ProcessInfo {
                            pid: id.pid,
                            ppid: id.ppid,
                            command: id.command.clone(),
                            ..ProcessInfo::default()
                        },
                        id,
                    )
                })
                .collect(),
            project,
        }
    }

    #[test]
    fn status_snapshot_collects_once_and_keeps_worker_and_agent_scopes_distinct() {
        let settings = Settings {
            project: "/agentwatcher-status-fixture".into(),
            worker: "/workers/shared-loop.sh".into(),
            state_dir: "/agentwatcher-status-fixture/state".into(),
            ..Settings::default()
        };
        let worker = Identity {
            pid: u32::MAX - 10,
            ppid: 1,
            started: (1, 0),
            executable: "/bin/bash".into(),
            cwd: settings.project.clone().into(),
            command: "/bin/bash /workers/shared-loop.sh".into(),
            argv: vec!["/bin/bash".into(), settings.worker.clone()],
        };
        let helper = Identity {
            pid: u32::MAX - 9,
            ppid: worker.pid,
            executable: "/bin/echo".into(),
            cwd: "/other".into(),
            argv: vec!["/bin/echo".into()],
            ..worker.clone()
        };
        let child_agent = Identity {
            pid: u32::MAX - 8,
            ppid: helper.pid,
            executable: "/usr/bin/codex".into(),
            ..helper.clone()
        };
        let project_agent = Identity {
            pid: u32::MAX - 7,
            ppid: 1,
            cwd: settings.project.clone().into(),
            ..child_agent.clone()
        };
        let other_worker = Identity {
            pid: u32::MAX - 6,
            cwd: "/other".into(),
            ..worker.clone()
        };
        let other_agent = Identity {
            pid: u32::MAX - 5,
            ppid: other_worker.pid,
            ..child_agent.clone()
        };
        let snapshot = process_snapshot_fixture(
            &settings,
            vec![
                worker.clone(),
                helper.clone(),
                child_agent.clone(),
                project_agent.clone(),
                other_worker,
                other_agent,
            ],
        );
        assert_eq!(
            snapshot
                .scoped(&settings, true)
                .unwrap()
                .iter()
                .map(|(row, _)| row.pid)
                .collect::<Vec<_>>(),
            vec![worker.pid, helper.pid, child_agent.pid]
        );
        let collections = std::cell::Cell::new(0);
        let (state, rows) = status_snapshot_with(&settings, |_| {
            collections.set(collections.get() + 1);
            Ok(snapshot)
        });
        assert_eq!(collections.get(), 1);
        assert!(state.running);
        assert_eq!(state.label, "Working");
        let rows = rows.unwrap();
        assert_eq!(
            rows.iter().map(|row| row.pid).collect::<Vec<_>>(),
            vec![worker.pid, child_agent.pid, project_agent.pid]
        );
        assert!(rows.iter().all(|row| !row.identity_token.is_empty()));
        admit_selected_identity(&rows[0], &worker).unwrap();
    }

    #[test]
    fn status_snapshot_retains_independent_scope_limits_and_collection_errors() {
        let settings = Settings {
            project: "/agentwatcher-status-fixture".into(),
            worker: "/workers/shared-loop.sh".into(),
            state_dir: "/agentwatcher-status-fixture/state".into(),
            ..Settings::default()
        };
        let worker = Identity {
            pid: u32::MAX - 1,
            ppid: 1,
            started: (1, 0),
            executable: "/bin/bash".into(),
            cwd: settings.project.clone().into(),
            command: "worker".into(),
            argv: vec!["/bin/bash".into(), settings.worker.clone()],
        };
        let mut many_children = vec![worker.clone()];
        many_children.extend((1..=512).map(|offset| Identity {
            pid: worker.pid - offset,
            ppid: worker.pid,
            executable: "/bin/echo".into(),
            argv: vec!["/bin/echo".into()],
            ..worker.clone()
        }));
        let snapshot = process_snapshot_fixture(&settings, many_children);
        let (state, rows) = status_snapshot_with(&settings, |_| Ok(snapshot));
        assert_eq!(state.label, "Status unavailable");
        assert_eq!(rows.unwrap().len(), 1);

        let many_agents = (0..513)
            .map(|offset| Identity {
                pid: worker.pid - offset,
                executable: "/usr/bin/codex".into(),
                argv: vec!["/usr/bin/codex".into()],
                ..worker.clone()
            })
            .collect();
        let snapshot = process_snapshot_fixture(&settings, many_agents);
        let (state, rows) = status_snapshot_with(&settings, |_| Ok(snapshot));
        assert_eq!(state.label, "Stopped");
        assert!(rows.unwrap_err().contains("512-process safety limit"));

        let collections = std::cell::Cell::new(0);
        let (state, rows) = status_snapshot_with(&settings, |_| {
            collections.set(collections.get() + 1);
            Err("snapshot unavailable".into())
        });
        assert_eq!(collections.get(), 1);
        assert_eq!(state.label, "Status unavailable");
        assert_eq!(rows.unwrap_err(), "snapshot unavailable");
        let no_worker = Settings {
            worker: String::new(),
            ..settings
        };
        let (state, rows) =
            status_snapshot_with(&no_worker, |_| Err("snapshot unavailable".into()));
        assert_eq!(state.label, "No worker configured");
        assert_eq!(rows.unwrap_err(), "snapshot unavailable");
    }

    #[test]
    fn command_spawns_keep_owned_groups_under_concurrent_callers() {
        let callers: Vec<_> = (0..4)
            .map(|_| {
                thread::spawn(|| {
                    for _ in 0..3 {
                        let output = run(
                            "/bin/sh",
                            &["-c", "kill -0 -- \"-$$\" && printf '%s' \"$$\""],
                            Some("/"),
                            Duration::from_secs(2),
                            128,
                        )
                        .unwrap();
                        // The shell builtin checks group -PID while that child
                        // is alive. Success proves the owned PGID exists without
                        // ps or delivery of an actual signal.
                        let child_group = output.parse::<i32>().unwrap();
                        assert_ne!(
                            child_group,
                            unsafe { libc::getpgrp() },
                            "Child must not share the application's process group"
                        );
                    }
                })
            })
            .collect();
        for caller in callers {
            caller.join().unwrap();
        }
    }

    #[test]
    fn successful_stdout_is_separate_from_bounded_stderr_diagnostics() {
        assert_eq!(
            run(
                "/bin/sh",
                &["-c", "printf framed; printf warning >&2"],
                None,
                Duration::from_secs(1),
                1024
            )
            .unwrap(),
            "framed"
        );
        let failure = run(
            "/bin/sh",
            &["-c", "printf partial; printf diagnostic >&2; exit 3"],
            None,
            Duration::from_secs(1),
            1024,
        )
        .unwrap_err();
        assert!(failure.contains("partial"));
        assert!(failure.contains("diagnostic"));
        assert!(
            run(
                "/bin/sh",
                &["-c", "printf 1234; printf 5678 >&2"],
                None,
                Duration::from_secs(1),
                6
            )
            .unwrap_err()
            .contains("capture limit")
        );
    }

    #[test]
    fn completed_command_is_not_timed_out_by_delayed_parent_collection() {
        let slot = CommandSlot::acquire().unwrap();
        let child = Command::new("/usr/bin/printf")
            .args(["%s", "completed before parent collection"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        let ready_deadline = Instant::now() + Duration::from_secs(1);
        while !observe_exit(child.id()).unwrap() {
            if Instant::now() >= ready_deadline {
                let result = capture_command(
                    child,
                    slot,
                    "/usr/bin/printf",
                    Duration::from_secs(1),
                    ready_deadline,
                    1024,
                );
                panic!("The synthetic command did not complete promptly: {result:?}");
            }
            thread::sleep(Duration::from_millis(1));
        }
        // The child has really finished before this deadline. Delay collecting
        // its already-ready pipes, modelling an App Nap/scheduling pause.
        let timeout = Duration::from_millis(5);
        let deadline = Instant::now() + timeout;
        thread::sleep(Duration::from_millis(10));
        assert!(Instant::now() >= deadline);
        assert_eq!(
            capture_command(child, slot, "/usr/bin/printf", timeout, deadline, 1024).unwrap(),
            "completed before parent collection"
        );
    }

    #[test]
    fn incomplete_commands_still_time_out_when_output_or_leader_has_closed() {
        for script in [
            "exec >/dev/null 2>&1; /bin/sleep 5",
            "/bin/sleep 5 & exit 0",
        ] {
            assert!(
                run(
                    "/bin/sh",
                    &["-c", script],
                    None,
                    Duration::from_millis(50),
                    1024,
                )
                .unwrap_err()
                .starts_with("Command timed out")
            );
        }
    }

    #[test]
    fn read_capture_is_bounded_without_deadlock() {
        let output = run(
            "/usr/bin/printf",
            &["%s", "one argument; $(no shell)"],
            None,
            Duration::from_secs(2),
            128,
        )
        .unwrap();
        assert_eq!(output, "one argument; $(no shell)");
        let overflow = run(
            "/usr/bin/printf",
            &["123456789"],
            None,
            Duration::from_secs(2),
            4,
        )
        .unwrap_err();
        assert!(overflow.starts_with("Command output exceeded the 4-byte capture limit."));
        assert!(overflow.contains("Partial command output:\n1234"));
        assert!(
            run(
                "/bin/sh",
                &[
                    "-c",
                    "while :; do printf xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx; done"
                ],
                None,
                Duration::from_millis(100),
                1024
            )
            .is_err()
        );
        assert!(
            run(
                "/bin/sh",
                &["-c", "sleep 5 & wait"],
                None,
                Duration::from_millis(50),
                1024
            )
            .unwrap_err()
            .contains("timed out")
        );
        assert!(
            run(
                "/bin/echo",
                &["bad\0arg"],
                None,
                Duration::from_secs(1),
                1024
            )
            .is_err()
        );
        assert!(run("echo", &[], None, Duration::from_secs(1), 1024).is_err());
        assert!(run("/bin/echo", &[], None, Duration::ZERO, 1024).is_err());
    }

    #[test]
    fn new_file_diff_counts_partial_lines_and_binary() {
        assert_eq!(line_count(b"one\ntwo"), 2);
        assert_eq!(line_count(b"one\n"), 1);
        assert_eq!(line_count(b""), 0);
        assert!(new_file_diff("a", b"one\ntwo").contains("\\ No newline at end of file"));
        assert!(new_file_diff("a", b"one\0two").starts_with("Binary file"));
        assert_eq!(finite_width(f32::NAN, 228.0, 190.0, 320.0), 228.0);
        assert_eq!(finite_width(900.0, 228.0, 190.0, 320.0), 320.0);
    }

    #[test]
    fn native_argument_framing_stops_at_argc_without_decoding_environment() {
        let mut bytes = 3i32.to_ne_bytes().to_vec();
        bytes.extend_from_slice(b"/installed/opencode\0\0\0opencode\0--session\0ses_fixture\0");
        bytes.extend_from_slice(b"UNRELATED_ENV=\xff\xfe\0");
        assert_eq!(
            parse_mac_arguments(&bytes).unwrap(),
            vec!["opencode", "--session", "ses_fixture"]
        );
        let mut empty_argument = 3i32.to_ne_bytes().to_vec();
        empty_argument.extend_from_slice(b"/installed/opencode\0\0opencode\0\0last\0");
        assert_eq!(
            parse_mac_arguments(&empty_argument).unwrap(),
            vec!["opencode", "", "last"]
        );
        bytes.truncate(12);
        assert!(parse_mac_arguments(&bytes).is_none());
        assert!(
            process_command(&ProcessInfo {
                pid: std::process::id(),
                ..ProcessInfo::default()
            })
            .is_none()
        );
    }

    #[test]
    fn retained_selection_rejects_a_refreshed_process_reusing_the_same_pid() {
        let captured_a = Identity {
            pid: 12345,
            ppid: 100,
            started: (10, 42),
            executable: "/usr/bin/opencode".into(),
            cwd: "/projects/a".into(),
            command: "opencode".into(),
            argv: Vec::new(),
        };
        let selected_a = ProcessInfo {
            pid: captured_a.pid,
            command: captured_a.command.clone(),
            identity_token: identity_token(&captured_a),
            ..ProcessInfo::default()
        };
        let mut refreshed_cache = HashMap::from([(captured_a.pid, captured_a.clone())]);
        assert!(admit_selected_identity(&selected_a, &refreshed_cache[&selected_a.pid]).is_ok());
        // The process exits while confirmation remains open. The replacement
        // has the same PID, executable, command and cwd; a later refresh updates
        // the cache, but it must not transfer the retained selection's intent.
        let replacement_b = Identity {
            started: (11, 7),
            ..captured_a.clone()
        };
        refreshed_cache.insert(replacement_b.pid, replacement_b.clone());
        assert_ne!(selected_a.identity_token, identity_token(&replacement_b));
        assert!(admit_selected_identity(&selected_a, &refreshed_cache[&selected_a.pid]).is_err());
        assert_eq!(selected_a.identity_token, identity_token(&captured_a));
        let selected_b = ProcessInfo {
            identity_token: identity_token(&replacement_b),
            ..selected_a.clone()
        };
        assert!(admit_selected_identity(&selected_b, &refreshed_cache[&selected_b.pid]).is_ok());
        assert!(
            admit_selected_identity(
                &ProcessInfo {
                    identity_token: String::new(),
                    ..selected_b
                },
                &replacement_b
            )
            .is_err()
        );
    }

    #[test]
    fn invalid_actions_never_execute_a_worker() {
        assert_eq!(
            worker_action(&Settings::default(), "validate").unwrap_err(),
            "Unknown worker action."
        );
        assert!(worker_action(&Settings::default(), "start").is_err());
        assert!(
            stop_process(
                &Settings::default(),
                &ProcessInfo {
                    pid: std::process::id(),
                    ..ProcessInfo::default()
                }
            )
            .is_err()
        );
    }
}
