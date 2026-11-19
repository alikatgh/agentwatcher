//! Bounded, read-only guidance and skill evidence for one recorded agent.
//!
//! Call `collect` on a worker thread. Filesystem contents describe the files
//! *now*, whereas trace evidence describes only the loaded recording window.
//! Neither is a reconstruction of the provider's complete effective prompt.
use crate::telemetry::{Record, Session, Snapshot, redact};
use regex::Regex;
use serde_json::Value;
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const FILE_BYTES: usize = 64 * 1024;
const FILE_COUNT: usize = 16;
const FILE_CHECKS: usize = 64;
const RECORD_COUNT: usize = 4096;
const RECORD_BYTES: usize = 256 * 1024;
const TRACE_BYTES: usize = 4 * 1024 * 1024;
const CONTENT_BYTES: usize = 2 * 1024 * 1024;
const RECORDED_CONTENT_BYTES: usize = 1024 * 1024;
const DOCUMENT_COUNT: usize = 32;
const RECORDED_DOCUMENT_COUNT: usize = 16;
const SKILL_COUNT: usize = 32;
const REFERENCE_COUNT: usize = 8;
const PATH_BYTES: usize = 4096;
const COLLECTION_TIME: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, Default)]
pub struct AgentContext {
    pub session_id: String,
    pub source: String,
    pub workspace: String,
    pub agent: String,
    pub guidance: Vec<GuidanceDocument>,
    pub skills: Vec<SkillEvidence>,
    pub warnings: Vec<String>,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuidanceSource {
    CurrentFilesystem,
    RecordedRead,
    RecordedInstructions,
}

impl GuidanceSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::CurrentFilesystem => "Current file",
            Self::RecordedRead => "Recorded read",
            Self::RecordedInstructions => "Recorded instructions",
        }
    }
}

#[derive(Clone, Debug)]
pub struct GuidanceDocument {
    pub id: String,
    pub path: String,
    pub source: GuidanceSource,
    pub scope: String,
    /// Redacted text; UI and copy actions should use the same value.
    pub text: String,
    pub truncated: bool,
    pub error: Option<String>,
    pub trace: Vec<TraceReference>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillEvidenceKind {
    ConfirmedRead,
    AnnouncedUse,
}

impl SkillEvidenceKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::ConfirmedRead => "Read recorded",
            Self::AnnouncedUse => "Use announced",
        }
    }
}

#[derive(Clone, Debug)]
pub struct SkillEvidence {
    pub id: String,
    pub name: String,
    /// Exact path/resource in the read, resolved against its recorded cwd when
    /// possible. Announcements can have no recorded path.
    pub path: Option<String>,
    pub kind: SkillEvidenceKind,
    pub trace: Vec<TraceReference>,
    /// Output of that historical read, when it can be attributed to one file.
    pub recorded_text: Option<String>,
    /// Optional current contents, distinctly labelled and never evidence of use.
    pub current_document: Option<GuidanceDocument>,
}

#[derive(Clone, Debug)]
pub struct TraceReference {
    pub record_id: String,
    pub time: String,
    pub summary: String,
}

struct Budget {
    start: Instant,
    checks: usize,
    files: usize,
    file_bytes: usize,
    content_bytes: usize,
    recorded_content_bytes: usize,
    trace_bytes: usize,
    truncated: bool,
}

impl Budget {
    fn new() -> Self {
        Self {
            start: Instant::now(),
            checks: 0,
            files: 0,
            file_bytes: 0,
            content_bytes: 0,
            recorded_content_bytes: 0,
            trace_bytes: 0,
            truncated: false,
        }
    }

    fn available(&mut self) -> bool {
        let available = self.start.elapsed() < COLLECTION_TIME;
        self.truncated |= !available;
        available
    }

    fn content(&mut self, raw: &str) -> (String, bool) {
        let maximum = FILE_BYTES.min(CONTENT_BYTES.saturating_sub(self.content_bytes));
        // Bound work before applying the shared redaction policy, then bound its
        // possibly expanded result as well. Never clip a UTF-8 code point.
        let (raw_prefix, input_cut) = prefix(raw, maximum);
        let safe = redact_content(raw_prefix);
        let (safe, output_cut) = prefix(&safe, maximum);
        self.content_bytes += safe.len();
        let truncated = input_cut || output_cut;
        self.truncated |= truncated;
        (safe.to_owned(), truncated)
    }

    fn recorded_content(&mut self, raw: &str) -> (String, bool) {
        let remaining = RECORDED_CONTENT_BYTES.saturating_sub(self.recorded_content_bytes);
        let (raw, cut) = prefix(raw, remaining);
        let (text, content_cut) = self.content(raw);
        self.recorded_content_bytes += text.len();
        self.truncated |= cut;
        (text, cut || content_cut)
    }
}

/// Collect only the selected agent. Snapshot identity must agree with the
/// selection before any trace-derived path or evidence is used.
pub fn collect(session: &Session, snapshot: &Snapshot) -> AgentContext {
    let mut result = AgentContext {
        session_id: redact(&session.id),
        source: redact(&session.source),
        workspace: redact(&session.project),
        agent: redact(&session.agent),
        ..AgentContext::default()
    };
    if session.id != snapshot.session.id
        || session.source != snapshot.session.source
        || session.project != snapshot.session.project
    {
        result.warnings.push(
            "Guidance unavailable: the loaded trace belongs to a different agent or workspace."
                .into(),
        );
        return result;
    }
    if let Some(error) = &snapshot.error {
        result.warnings.push(format!(
            "The loaded trace could not be verified: {}",
            redact(prefix(error, 1024).0)
        ));
    }
    let mut budget = Budget::new();
    let workspace = absolute_path(&session.project);
    let mut cwd = if snapshot.has_earlier
        || snapshot.window_start > 0
        || snapshot.records.len() > RECORD_COUNT
    {
        None
    } else {
        workspace.clone()
    };
    let mut working_directories = Vec::<(PathBuf, TraceReference)>::new();
    let mut records = Vec::new();
    // Prefer the newest bounded window when a caller supplies a large snapshot.
    for record in snapshot.records.iter().rev().take(RECORD_COUNT) {
        if snapshot.error.is_some() {
            break;
        }
        if !budget.available() {
            break;
        }
        if record.detail.len() > RECORD_BYTES {
            budget.truncated = true;
            continue;
        }
        if budget.trace_bytes.saturating_add(record.detail.len()) > TRACE_BYTES {
            budget.truncated = true;
            break;
        }
        budget.trace_bytes += record.detail.len();
        records.push(record);
    }
    budget.truncated |= snapshot.records.len() > RECORD_COUNT;
    records.reverse();
    for record in records {
        if !budget.available() {
            break;
        }
        let Ok(raw) = serde_json::from_str::<Value>(&record.detail) else {
            // Generic text is not sufficient evidence of a file read or cwd.
            continue;
        };
        if let Some(recorded_cwd) = context_cwd(record, &raw).and_then(absolute_path) {
            if workspace
                .as_ref()
                .is_some_and(|workspace| recorded_cwd.starts_with(workspace))
            {
                cwd = Some(recorded_cwd.clone());
                if !working_directories
                    .iter()
                    .any(|(path, _)| *path == recorded_cwd)
                {
                    if working_directories.len() < FILE_COUNT {
                        working_directories.push((recorded_cwd, reference(record)));
                    } else {
                        budget.truncated = true;
                    }
                }
            }
        }
        if record.kind == "context" && record.is_infrastructure {
            if let Some(message) = message_text(&raw) {
                if let Some((path, contents)) = recorded_instructions(&message) {
                    add_guidance(
                        &mut result,
                        &mut budget,
                        &path,
                        GuidanceSource::RecordedInstructions,
                        "Instructions present in this recorded context",
                        &contents,
                        record,
                    );
                }
            }
        }
        if record.kind == "assistant" && !record.is_infrastructure {
            if let Some(message) = message_text(&raw) {
                for name in announced_skills(&message) {
                    add_skill(
                        &mut result,
                        &mut budget,
                        name,
                        None,
                        SkillEvidenceKind::AnnouncedUse,
                        None,
                        record,
                    );
                }
            }
        }
        if record.kind != "tool" || record.tool_status != "Completed" {
            continue;
        }
        if let Some(workdir) = raw["input"]["workdir"].as_str().and_then(absolute_path) {
            if workspace
                .as_ref()
                .is_some_and(|workspace| workdir.starts_with(workspace))
                && !working_directories.iter().any(|(path, _)| *path == workdir)
            {
                if working_directories.len() < FILE_COUNT {
                    working_directories.push((workdir, reference(record)));
                } else {
                    budget.truncated = true;
                }
            }
        }
        let reads = successful_reads(&raw, cwd.as_deref());
        let contents = (reads.len() == 1)
            .then(|| output_text(&raw["output"]))
            .flatten();
        for path in reads {
            if is_guidance_path(&path) {
                add_guidance(
                    &mut result,
                    &mut budget,
                    &path,
                    GuidanceSource::RecordedRead,
                    "File output preserved in the loaded trace",
                    contents.as_deref().unwrap_or(""),
                    record,
                );
            } else if file_name(&path) == Some("SKILL.md") {
                let name = skill_name(&path);
                add_skill(
                    &mut result,
                    &mut budget,
                    name,
                    Some(path),
                    SkillEvidenceKind::ConfirmedRead,
                    contents.as_deref(),
                    record,
                );
            }
        }
    }

    // Current files are listed outermost first, independently of historical
    // instructions. Never enumerate a directory or traverse sibling projects.
    let mut candidates = Vec::<(PathBuf, String, Vec<TraceReference>)>::new();
    if session.source == "codex" {
        if let Some(home) = codex_home() {
            candidates.push((
                home,
                "Current Codex home guidance (watcher environment)".into(),
                Vec::new(),
            ));
        }
    }
    if let Some(workspace) = &workspace {
        let hierarchy = if session.source == "codex" {
            codex_project_chain(workspace, &mut budget)
        } else {
            ancestor_chain(workspace)
        };
        for directory in hierarchy {
            candidates.push((
                directory.clone(),
                if directory == *workspace {
                    "Workspace guidance".into()
                } else {
                    "Workspace ancestor guidance".into()
                },
                Vec::new(),
            ));
        }
        for (directory, trace) in working_directories {
            // Only subdirectories proven by structured context/tool fields.
            let hierarchy = if session.source == "codex" {
                codex_project_chain(&directory, &mut budget)
            } else {
                ancestor_chain(&directory)
            };
            for directory in hierarchy {
                if directory != *workspace && directory.starts_with(workspace) {
                    if !candidates
                        .iter()
                        .any(|(candidate, _, _)| *candidate == directory)
                    {
                        candidates.push((
                            directory,
                            "Guidance for a recorded working directory".into(),
                            vec![trace.clone()],
                        ));
                    }
                }
            }
        }
    } else {
        result.warnings.push(
            "Current guidance unavailable: no unredacted absolute workspace path was recorded."
                .into(),
        );
    }
    let mut current = Vec::new();
    for (directory, scope, trace) in candidates {
        if current.len() + result.guidance.len() >= DOCUMENT_COUNT {
            budget.truncated = true;
            break;
        }
        if let Some(document) = read_directory_guidance(
            &directory,
            &scope,
            trace,
            session.source == "codex",
            &mut budget,
        ) {
            current.push(document);
        }
    }
    current.append(&mut result.guidance);
    result.guidance = current;
    for skill in &mut result.skills {
        if skill.kind == SkillEvidenceKind::ConfirmedRead {
            if let Some(path) = skill.path.as_deref().and_then(absolute_path) {
                skill.current_document = read_current(
                    &path,
                    "Current contents; recorded read may contain an older version",
                    Vec::new(),
                    &mut budget,
                );
            }
        }
    }
    if result.guidance.is_empty() && workspace.is_some() {
        result.warnings.push(
            "No AGENTS.md was found in the checked workspace hierarchy or loaded trace.".into(),
        );
    }
    result.warnings.push(
        "Current files may differ from the agent's recorded instructions. Skill evidence describes the loaded trace and does not prove a skill is active now.".into(),
    );
    if session.source == "codex" {
        result.warnings.push(
            "Codex may truncate combined guidance. Current files and the watcher's Codex home do not prove the agent's exact effective prompt.".into(),
        );
    }
    if snapshot.has_earlier || snapshot.window_start > 0 {
        result.warnings.push(
            "Earlier trace events are not loaded; guidance and skill evidence may be incomplete."
                .into(),
        );
    }
    result.truncated = budget.truncated;
    if result.truncated {
        result.warnings.push(
            "Collection was bounded; some files, contents, or trace evidence were omitted.".into(),
        );
    }
    result
}

fn codex_home() -> Option<PathBuf> {
    std::env::var("CODEX_HOME")
        .ok()
        .and_then(|path| absolute_path(&path))
        .or_else(|| {
            let home = std::env::var("HOME").ok()?;
            Some(absolute_path(&home)?.join(".codex"))
        })
}

fn read_directory_guidance(
    directory: &Path,
    scope: &str,
    trace: Vec<TraceReference>,
    codex: bool,
    budget: &mut Budget,
) -> Option<GuidanceDocument> {
    // Codex selects at most one instruction file in each directory. Empty
    // files do not suppress a later fallback; unreadable files remain visible.
    let names: &[&str] = if codex {
        &["AGENTS.override.md", "AGENTS.md"]
    } else {
        &["AGENTS.md"]
    };
    for name in names {
        if let Some(document) = read_current(&directory.join(name), scope, trace.clone(), budget) {
            if !document.text.is_empty() || document.error.is_some() || document.truncated {
                return Some(document);
            }
        }
    }
    None
}

fn read_current(
    path: &Path,
    scope: &str,
    trace: Vec<TraceReference>,
    budget: &mut Budget,
) -> Option<GuidanceDocument> {
    if !budget.available()
        || budget.checks >= FILE_CHECKS
        || budget.files >= FILE_COUNT
        || budget.content_bytes >= CONTENT_BYTES
    {
        budget.truncated = true;
        return None;
    }
    budget.checks += 1;
    // O_NONBLOCK prevents an unexpected pipe/device at an instruction path
    // from hanging the collector. The opened descriptor must be a regular file.
    let opened = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path);
    let mut file = match opened {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            return Some(current_document(
                path,
                scope,
                trace,
                String::new(),
                false,
                Some(redact(&error.to_string())),
            ));
        }
    };
    let metadata = match file.metadata() {
        Ok(metadata) if metadata.is_file() => metadata,
        _ => return None,
    };
    budget.files += 1;
    let maximum = FILE_BYTES
        .min(FILE_COUNT * FILE_BYTES - budget.file_bytes)
        .min(CONTENT_BYTES.saturating_sub(budget.content_bytes));
    let mut bytes = Vec::new();
    let error = file
        .by_ref()
        .take(maximum as u64)
        .read_to_end(&mut bytes)
        .err()
        .map(|error| redact(&error.to_string()));
    budget.file_bytes += bytes.len();
    let file_cut = metadata.len() > bytes.len() as u64;
    let (text, content_cut) = budget.content(&String::from_utf8_lossy(&bytes));
    budget.truncated |= file_cut;
    Some(current_document(
        path,
        scope,
        trace,
        text,
        file_cut || content_cut,
        error,
    ))
}

fn current_document(
    path: &Path,
    scope: &str,
    trace: Vec<TraceReference>,
    text: String,
    truncated: bool,
    error: Option<String>,
) -> GuidanceDocument {
    let path = redact(&path.to_string_lossy());
    GuidanceDocument {
        id: format!("current:{path}"),
        path,
        source: GuidanceSource::CurrentFilesystem,
        scope: scope.into(),
        text,
        truncated,
        error,
        trace,
    }
}

fn add_guidance(
    result: &mut AgentContext,
    budget: &mut Budget,
    path: &str,
    source: GuidanceSource,
    scope: &str,
    text: &str,
    record: &Record,
) {
    if result.guidance.len() >= RECORDED_DOCUMENT_COUNT {
        budget.truncated = true;
        return;
    }
    let previous_content = budget.content_bytes;
    let previous_recorded_content = budget.recorded_content_bytes;
    let (text, truncated) = budget.recorded_content(text);
    let path = redact(prefix(path, PATH_BYTES).0);
    if let Some(document) = result.guidance.iter_mut().find(|document| {
        document.path == path && document.source == source && document.text == text
    }) {
        add_reference(&mut document.trace, record);
        budget.content_bytes = previous_content;
        budget.recorded_content_bytes = previous_recorded_content;
        return;
    }
    result.guidance.push(GuidanceDocument {
        id: format!("recorded:{}:{path}", redact(&record.id)),
        path,
        source,
        scope: scope.into(),
        text,
        truncated,
        error: None,
        trace: vec![reference(record)],
    });
}

fn add_skill(
    result: &mut AgentContext,
    budget: &mut Budget,
    name: String,
    path: Option<String>,
    kind: SkillEvidenceKind,
    contents: Option<&str>,
    record: &Record,
) {
    let name = redact(prefix(&name, 160).0);
    let path = path.map(|path| redact(prefix(&path, PATH_BYTES).0));
    let existing = result.skills.iter_mut().find(|skill| {
        // A bare announcement cannot merge two different files sharing a name.
        if path.is_some() {
            skill.path == path || (skill.path.is_none() && skill.name == name)
        } else {
            skill.path.is_none() && skill.name == name
        }
    });
    if let Some(skill) = existing {
        add_reference(&mut skill.trace, record);
        if kind == SkillEvidenceKind::ConfirmedRead {
            skill.path = path;
            skill.id = format!("skill:{}", skill.path.as_deref().unwrap_or(""));
            skill.kind = kind;
            if let Some(contents) = contents {
                skill.recorded_text = Some(budget.recorded_content(contents).0);
            }
        }
        return;
    }
    if result.skills.len() >= SKILL_COUNT {
        budget.truncated = true;
        return;
    }
    result.skills.push(SkillEvidence {
        id: path
            .as_ref()
            .map(|path| format!("skill:{path}"))
            .unwrap_or_else(|| format!("announced:{name}")),
        name,
        path,
        kind,
        trace: vec![reference(record)],
        recorded_text: contents.map(|contents| budget.recorded_content(contents).0),
        current_document: None,
    });
}

fn reference(record: &Record) -> TraceReference {
    TraceReference {
        record_id: redact(prefix(&record.id, 256).0),
        time: redact(prefix(&record.time, 160).0),
        summary: redact(prefix(&record.summary, 512).0),
    }
}

fn add_reference(references: &mut Vec<TraceReference>, record: &Record) {
    if references.len() < REFERENCE_COUNT && !references.iter().any(|r| r.record_id == record.id) {
        references.push(reference(record));
    }
}

fn prefix(value: &str, maximum: usize) -> (&str, bool) {
    let mut end = maximum.min(value.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    (&value[..end], end < value.len())
}

fn redact_content(raw: &str) -> String {
    let safe = redact(raw);
    // A bounded prefix can stop before a private key's closing marker. The
    // shared policy handles complete blocks; also mask any remaining opener.
    static OPEN_KEY: OnceLock<Regex> = OnceLock::new();
    OPEN_KEY
        .get_or_init(|| {
            Regex::new(r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*")
                .expect("static partial private key regex")
        })
        .replace_all(&safe, "[private key redacted]")
        .into_owned()
}

fn absolute_path(path: &str) -> Option<PathBuf> {
    if path.len() > PATH_BYTES
        || path.contains(['\0', '\n', '\r'])
        || path.contains("[redacted]")
        || redact(path) != path
        || !Path::new(path).is_absolute()
    {
        return None;
    }
    let mut normalized = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            component => normalized.push(component.as_os_str()),
        }
    }
    Some(normalized)
}

fn ancestor_chain(path: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<_> = path
        .ancestors()
        .take(FILE_CHECKS)
        .map(Path::to_owned)
        .collect();
    paths.reverse();
    paths
}

fn codex_project_chain(cwd: &Path, budget: &mut Budget) -> Vec<PathBuf> {
    let mut chain = Vec::new();
    for directory in cwd.ancestors().take(FILE_CHECKS) {
        if !budget.available() || budget.checks >= FILE_CHECKS {
            budget.truncated = true;
            break;
        }
        budget.checks += 1;
        chain.push(directory.to_owned());
        if std::fs::metadata(directory.join(".git"))
            .is_ok_and(|metadata| metadata.is_dir() || metadata.is_file())
        {
            chain.reverse();
            return chain;
        }
    }
    // Without a Git root, Codex only checks its working directory. A bounded
    // search that could not reach a root also must not invent ancestor scope.
    vec![cwd.to_owned()]
}

fn file_name(path: &str) -> Option<&str> {
    path.rsplit('/').next()
}

fn is_guidance_path(path: &str) -> bool {
    matches!(file_name(path), Some("AGENTS.md" | "AGENTS.override.md"))
}

fn skill_name(path: &str) -> String {
    path.rsplit('/').nth(1).unwrap_or("Skill").to_owned()
}

fn context_cwd<'a>(record: &Record, raw: &'a Value) -> Option<&'a str> {
    if !record.is_infrastructure || !["context", "runtime"].contains(&record.kind.as_str()) {
        return None;
    }
    match raw["type"].as_str() {
        Some("turn_context" | "session_meta") => raw["payload"]["cwd"].as_str(),
        Some("session") if raw["version"] == 4 => raw["cwd"].as_str(),
        _ if record.id == "opencode-header" => raw["directory"]
            .as_str()
            .or_else(|| raw["cwd"].as_str())
            .or_else(|| raw["location"]["directory"].as_str()),
        _ => None,
    }
}

/// Extract actual message bodies only, never the arbitrary serialized payload.
fn message_text(raw: &Value) -> Option<String> {
    let value = match raw["type"].as_str() {
        Some("response_item") if raw["payload"]["type"] == "message" => &raw["payload"]["content"],
        Some("event_msg") => &raw["payload"]["message"],
        Some("assistant/message" | "user/message" | "system/message" | "developer/message") => {
            let data = &raw["data"];
            let message = if data["message"].is_object() {
                &data["message"]
            } else {
                data
            };
            if message["content"].is_null() {
                &message["text"]
            } else {
                &message["content"]
            }
        }
        _ if raw["parts"].is_array() => &raw["parts"],
        _ => raw.get("content").or_else(|| raw.get("text"))?,
    };
    body_text(value, 0)
}

fn body_text(value: &Value, depth: usize) -> Option<String> {
    if depth > 4 {
        return None;
    }
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Array(values) => {
            let bodies: Vec<_> = values
                .iter()
                .take(64)
                .filter(|value| {
                    value["type"]
                        .as_str()
                        .is_none_or(|kind| ["text", "input_text", "output_text"].contains(&kind))
                })
                .filter_map(|value| body_text(value, depth + 1))
                .collect();
            (!bodies.is_empty()).then(|| bodies.join("\n"))
        }
        Value::Object(_) => value["text"].as_str().map(str::to_owned),
        _ => None,
    }
}

fn recorded_instructions(message: &str) -> Option<(String, String)> {
    let message = message.trim();
    let header = message
        .lines()
        .next()?
        .strip_prefix("# AGENTS.md instructions for ")?;
    let directory = absolute_path(header.trim())?;
    let start = message.find("<INSTRUCTIONS>")? + "<INSTRUCTIONS>".len();
    let end = message[start..].find("</INSTRUCTIONS>")? + start;
    let path = if directory
        .file_name()
        .is_some_and(|name| name == "AGENTS.md" || name == "AGENTS.override.md")
    {
        directory
    } else {
        directory.join("AGENTS.md")
    };
    Some((
        path.to_string_lossy().into_owned(),
        message[start..end].trim().to_owned(),
    ))
}

fn announced_skills(message: &str) -> Vec<String> {
    // Announcements are short assistant prose. Long catalogs, quoted task
    // instructions, code, hypothetical/future plans and user messages are not.
    if message.len() > 2048
        || message.contains("```")
        || message.contains("<INSTRUCTIONS>")
        || message.contains("Available skills")
    {
        return Vec::new();
    }
    static ANNOUNCEMENT: OnceLock<Regex> = OnceLock::new();
    static SEPARATOR: OnceLock<Regex> = OnceLock::new();
    static NAME: OnceLock<Regex> = OnceLock::new();
    let pattern = ANNOUNCEMENT.get_or_init(|| {
        Regex::new(
            r"(?im)^\s*I(?:['’]m| am) (?:using|applying) (?:the )?([^\n.]{1,240}?) (?:skills?\b|workflow\b)",
        )
        .expect("static skill announcement regex")
    });
    let separator = SEPARATOR.get_or_init(|| {
        Regex::new(r"\s*(?:,\s*(?:and\s+)?|\s+and\s+|\s+&\s+)\s*")
            .expect("static skill separator regex")
    });
    let name = NAME.get_or_init(|| {
        Regex::new(r"^[a-z0-9][a-z0-9_-]*(?::[a-z0-9][a-z0-9_-]*)?$")
            .expect("static skill name regex")
    });
    let mut result = Vec::new();
    for capture in pattern.captures_iter(message).take(8) {
        let names: Vec<_> = separator
            .split(&capture[1])
            .take(9)
            .map(|value| value.trim().trim_matches('`').trim_start_matches('$'))
            .collect();
        if names.len() <= 8 && names.iter().all(|value| name.is_match(value)) {
            for name in names {
                if !result.iter().any(|known| known == name) {
                    result.push(name.to_owned());
                }
            }
        }
    }
    result
}

fn successful_reads(raw: &Value, fallback_cwd: Option<&Path>) -> Vec<String> {
    if raw["call"].is_null() || raw["result"].is_null() || output_failed(&raw["output"], 0) {
        return Vec::new();
    }
    let Some(name) = raw["name"].as_str() else {
        return Vec::new();
    };
    let tail = name.rsplit('.').next().unwrap_or(name);
    let tail = tail
        .rsplit("__")
        .next()
        .unwrap_or(tail)
        .to_ascii_lowercase();
    let input = &raw["input"];
    let cwd = input["workdir"]
        .as_str()
        .or_else(|| input["cwd"].as_str())
        .and_then(absolute_path)
        .or_else(|| fallback_cwd.map(Path::to_owned));
    let mut paths = if ["read", "read_file", "read_text_file", "readfile"].contains(&tail.as_str())
    {
        [
            "file_path",
            "filePath",
            "path",
            "filename",
            "resource",
            "uri",
        ]
        .iter()
        .find_map(|key| input[*key].as_str())
        .and_then(|path| resolve_read_path(path, cwd.as_deref()))
        .into_iter()
        .collect()
    } else if ["exec_command", "bash", "shell"].contains(&tail.as_str()) {
        if !shell_completed(&raw["output"], 0) {
            return Vec::new();
        }
        input["cmd"]
            .as_str()
            .or_else(|| input["command"].as_str())
            .map(|command| shell_reads(command, cwd.as_deref()))
            .unwrap_or_default()
    } else if tail == "exec" {
        // A completed JavaScript wrapper does not itself prove that its nested
        // shell commands succeeded. Require one explicit successful result for
        // every statically recognized shell call, with no failed result.
        let code = input.as_str().or_else(|| input["code"].as_str());
        code.map(|code| wrapper_reads(code, &raw["output"], cwd.as_deref()))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let mut seen = HashSet::new();
    paths.retain(|path| seen.insert(path.clone()));
    paths.truncate(FILE_COUNT);
    paths
}

fn shell_completed(value: &Value, depth: usize) -> bool {
    if depth > 8 {
        return false;
    }
    match value {
        Value::Object(_) => {
            value["exit_code"].as_i64() == Some(0)
                || value["status"] == "completed"
                || ["output", "content", "result", "text"]
                    .iter()
                    .any(|key| shell_completed(&value[*key], depth + 1))
        }
        Value::Array(values) => values
            .iter()
            .take(64)
            .any(|value| shell_completed(value, depth + 1)),
        Value::String(text) => {
            serde_json::from_str::<Value>(text)
                .ok()
                .is_some_and(|value| shell_completed(&value, depth + 1))
                || text
                    .lines()
                    .any(|line| line.trim() == "Process exited with code 0")
        }
        _ => false,
    }
}

fn resolve_read_path(path: &str, cwd: Option<&Path>) -> Option<String> {
    if path.len() > PATH_BYTES
        || !(file_name(path) == Some("SKILL.md") || is_guidance_path(path))
        || path.contains(['$', '`', '\0', '\n', '\r'])
        || redact(path) != path
        || path.contains("[redacted]")
    {
        return None;
    }
    if path.starts_with("skill://") {
        return Some(path.into());
    }
    if let Some(path) = absolute_path(path) {
        return Some(path.to_string_lossy().into_owned());
    }
    // Do not guess aliases (~, r0), shell variables or unavailable cwd values.
    if path.starts_with('~')
        || path.split('/').next().is_some_and(|part| {
            part.strip_prefix('r')
                .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
        })
    {
        return None;
    }
    absolute_path(&cwd?.join(path).to_string_lossy())
        .map(|path| path.to_string_lossy().into_owned())
}

fn shell_reads(command: &str, cwd: Option<&Path>) -> Vec<String> {
    let Some(tokens) = shell_tokens(command) else {
        return Vec::new();
    };
    let Some(program) = tokens
        .first()
        .and_then(|token| Path::new(token).file_name())
    else {
        return Vec::new();
    };
    let program = program.to_string_lossy();
    let args = &tokens[1..];
    let candidates: &[String] = match program.as_ref() {
        "cat" if args.iter().all(|arg| !arg.starts_with('-') || arg == "--") => args,
        "sed" if args.len() >= 3 && args[0] == "-n" && sed_range(&args[1]) => &args[2..],
        "head" | "tail"
            if args.len() >= 3
                && ["-n", "-c"].contains(&args[0].as_str())
                && args[1].bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            &args[2..]
        }
        "head" | "tail" if args.iter().all(|arg| !arg.starts_with('-')) => args,
        _ => return Vec::new(),
    };
    candidates
        .iter()
        .filter_map(|path| resolve_read_path(path, cwd))
        .take(FILE_COUNT)
        .collect()
}

fn sed_range(range: &str) -> bool {
    range.strip_suffix('p').is_some_and(|range| {
        !range.is_empty()
            && range.split(',').count() <= 2
            && range.split(',').all(|number| {
                !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
            })
    })
}

fn shell_tokens(command: &str) -> Option<Vec<String>> {
    if command.len() > 16 * 1024 {
        return None;
    }
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut quote = None;
    let mut escaped = false;
    for ch in command.chars() {
        if escaped {
            token.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if let Some(active) = quote {
            if ch == active {
                quote = None;
            } else {
                if active != '\'' && ['$', '`'].contains(&ch) {
                    return None;
                }
                token.push(ch);
            }
        } else if ['\'', '"'].contains(&ch) {
            quote = Some(ch);
        } else if [';', '|', '&', '<', '>', '$', '`', '\n', '\r', '(', ')'].contains(&ch) {
            return None;
        } else if ch.is_whitespace() {
            if !token.is_empty() {
                tokens.push(std::mem::take(&mut token));
            }
        } else {
            token.push(ch);
        }
        if tokens.len() > 32 {
            return None;
        }
    }
    if quote.is_some() || escaped {
        return None;
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    Some(tokens)
}

fn wrapper_reads(code: &str, output: &Value, cwd: Option<&Path>) -> Vec<String> {
    let Some(calls) = literal_wrapper_calls(code) else {
        return Vec::new();
    };
    let Some(output) = output_text(output) else {
        return Vec::new();
    };
    static EXIT: OnceLock<Regex> = OnceLock::new();
    let exits = EXIT.get_or_init(|| {
        Regex::new(r#"(?:"exit_code"\s*:\s*|Process exited with code\s+)(-?\d+)"#)
            .expect("static exit evidence regex")
    });
    let statuses: Vec<_> = exits.captures_iter(&output).take(17).collect();
    if statuses.len() != calls.len() || statuses.iter().any(|status| &status[1] != "0") {
        return Vec::new();
    }
    let mut paths = Vec::new();
    for fields in calls {
        let Some(command) = fields.get("cmd").and_then(Value::as_str) else {
            return Vec::new();
        };
        let workdir = fields
            .get("workdir")
            .and_then(Value::as_str)
            .and_then(absolute_path);
        paths.extend(shell_reads(command, workdir.as_deref().or(cwd)));
    }
    paths
}

fn literal_wrapper_calls(code: &str) -> Option<Vec<std::collections::HashMap<String, Value>>> {
    static START: OnceLock<Regex> = OnceLock::new();
    static END: OnceLock<Regex> = OnceLock::new();
    let start = START.get_or_init(|| {
        Regex::new(r"^(?:text|notify)\s*\(\s*await\s+tools\.exec_command\s*\(\s*")
            .expect("static literal wrapper prefix")
    });
    let end =
        END.get_or_init(|| Regex::new(r"^\s*\)\s*\)\s*;?").expect("static literal wrapper suffix"));
    let mut remaining = code.trim();
    if remaining.starts_with("// @exec:") {
        remaining = remaining.split_once('\n')?.1.trim_start();
    }
    let mut calls = Vec::new();
    while !remaining.is_empty() {
        if calls.len() >= FILE_COUNT {
            return None;
        }
        let prefix = start.find(remaining)?;
        let object = js_object(&remaining[prefix.end()..])?;
        let fields = js_literal_fields(object)?;
        let suffix = &remaining[prefix.end() + object.len()..];
        let suffix_end = end.find(suffix)?.end();
        remaining = suffix[suffix_end..].trim_start();
        calls.push(fields);
    }
    (!calls.is_empty()).then_some(calls)
}

// A small literal-only reader: no JS evaluation, interpolation or substitutions.
fn js_object(code: &str) -> Option<&str> {
    if !code.starts_with('{') {
        return None;
    }
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in code.char_indices() {
        if escaped {
            escaped = false;
        } else if quote.is_some() && ch == '\\' {
            escaped = true;
        } else if let Some(active) = quote {
            if ch == active {
                quote = None;
            }
        } else if ['\'', '"', '`'].contains(&ch) {
            quote = Some(ch);
        } else if ch == '{' {
            depth += 1;
        } else if ch == '}' {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(&code[..=index]);
            }
        }
    }
    None
}

fn js_literal_fields(object: &str) -> Option<std::collections::HashMap<String, Value>> {
    let body = object.strip_prefix('{')?.strip_suffix('}')?;
    let mut fields = std::collections::HashMap::new();
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in body
        .char_indices()
        .chain(std::iter::once((body.len(), ',')))
    {
        if escaped {
            escaped = false;
        } else if quote.is_some() && ch == '\\' {
            escaped = true;
        } else if let Some(active) = quote {
            if ch == active {
                quote = None;
            }
        } else if ['\'', '"', '`'].contains(&ch) {
            quote = Some(ch);
        } else if ch == ',' {
            let field = body[start..index].trim();
            start = index + 1;
            if field.is_empty() && index == body.len() {
                continue;
            }
            let (key, value) = field.split_once(':')?;
            let key = key.trim().trim_matches(['\'', '"']);
            if key.is_empty()
                || !key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                || fields.contains_key(key)
            {
                return None;
            }
            let value = value.trim();
            let parsed = if value.starts_with(['\'', '"', '`']) {
                Value::String(js_string(value)?)
            } else {
                let value: Value = serde_json::from_str(value).ok()?;
                if value.is_object() || value.is_array() {
                    return None;
                }
                value
            };
            fields.insert(key.into(), parsed);
        } else if ['{', '}', '[', ']', '/', ';'].contains(&ch) {
            // No comments, expressions or nested execution in literal objects.
            return None;
        }
    }
    if quote.is_some() || escaped {
        return None;
    }
    Some(fields)
}

fn js_string(literal: &str) -> Option<String> {
    let quote = literal.chars().next()?;
    if !['\'', '"', '`'].contains(&quote) {
        return None;
    }
    let mut result = String::new();
    let mut escaped = false;
    for (offset, ch) in literal[1..].char_indices() {
        if escaped {
            result.push(match ch {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '\\' => '\\',
                '\'' => '\'',
                '"' => '"',
                '`' => '`',
                _ => return None,
            });
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == quote {
            if (quote == '`' && result.contains("${")) || !literal[offset + 2..].trim().is_empty() {
                return None;
            }
            return Some(result);
        } else {
            result.push(ch);
        }
    }
    None
}

fn output_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Object(_) => {
            for key in ["output", "content", "text"] {
                if let Some(text) = body_text(&value[key], 0) {
                    return Some(text);
                }
            }
            // Structured shell output may contain an exit_code alongside text.
            serde_json::to_string(value).ok()
        }
        Value::Array(_) => body_text(value, 0),
        _ => None,
    }
}

fn output_failed(value: &Value, depth: usize) -> bool {
    if depth > 8 {
        return true;
    }
    match value {
        Value::Object(_) => {
            value["isError"] == true
                || value["is_error"] == true
                || value["success"] == false
                || value["exit_code"].as_i64().is_some_and(|exit| exit != 0)
                || ["error", "failed", "running", "pending"].contains(
                    &value["status"]
                        .as_str()
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .as_str(),
                )
                || (!value["error"].is_null()
                    && value["error"]
                        .as_str()
                        .is_none_or(|error| !error.is_empty()))
                || ["output", "content", "result", "text"]
                    .iter()
                    .any(|key| output_failed(&value[*key], depth + 1))
        }
        Value::Array(values) => values
            .iter()
            .take(64)
            .any(|value| output_failed(value, depth + 1)),
        Value::String(text) => {
            if let Ok(value) = serde_json::from_str::<Value>(text) {
                output_failed(&value, depth + 1)
            } else {
                text.lines().any(|line| {
                    line.strip_prefix("Process exited with code ")
                        .and_then(|exit| exit.trim().parse::<i64>().ok())
                        .is_some_and(|exit| exit != 0)
                })
            }
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "agentwatcher-context-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            // Only a newly-created test-owned directory is removed.
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn session(path: &Path) -> Session {
        Session {
            id: "selected-agent".into(),
            source: "codex".into(),
            project: path.to_string_lossy().into_owned(),
            agent: "Implementer".into(),
            ..Session::default()
        }
    }

    fn snapshot(session: &Session, records: Vec<Record>) -> Snapshot {
        Snapshot {
            session: session.clone(),
            records,
            ..Snapshot::default()
        }
    }

    fn tool(id: &str, name: &str, input: Value, output: Value) -> Record {
        Record {
            id: id.into(),
            kind: "tool".into(),
            summary: "Read instructions".into(),
            detail: json!({"name":name,"input":input,"output":output,"call":{"id":id},"result":{"id":id}}).to_string(),
            tool_status: "Completed".into(),
            ..Record::default()
        }
    }

    fn message(id: &str, kind: &str, text: &str, infrastructure: bool) -> Record {
        Record {
            id: id.into(),
            kind: kind.into(),
            is_infrastructure: infrastructure,
            detail: json!({"type":"response_item","payload":{"type":"message","role":kind,"content":[{"type":"output_text","text":text}]}}).to_string(),
            ..Record::default()
        }
    }

    #[test]
    fn hierarchy_is_outer_to_inner_and_only_recorded_subdirectories_are_checked() {
        let scratch = Scratch::new();
        let workspace = scratch.0.join("project");
        let child = workspace.join("src");
        let sibling = workspace.join("unvisited");
        fs::create_dir_all(&child).unwrap();
        fs::create_dir_all(&sibling).unwrap();
        fs::create_dir(scratch.0.join(".git")).unwrap();
        fs::write(scratch.0.join("AGENTS.md"), "Parent").unwrap();
        fs::write(workspace.join("AGENTS.md"), "Workspace").unwrap();
        fs::write(child.join("AGENTS.md"), "Child").unwrap();
        fs::write(sibling.join("AGENTS.md"), "Unvisited").unwrap();
        let selected = session(&workspace);
        let cwd = Record {
            id: "context-cwd".into(),
            kind: "context".into(),
            is_infrastructure: true,
            detail: json!({"type":"turn_context","payload":{"cwd":child}}).to_string(),
            ..Record::default()
        };
        let collected = collect(&selected, &snapshot(&selected, vec![cwd]));
        let docs: Vec<_> = collected
            .guidance
            .iter()
            .filter(|doc| doc.path.starts_with(&*scratch.0.to_string_lossy()))
            .collect();
        assert_eq!(
            docs.iter().map(|doc| doc.text.as_str()).collect::<Vec<_>>(),
            ["Parent", "Workspace", "Child"]
        );
        assert_eq!(docs[2].trace[0].record_id, "context-cwd");
        assert!(!collected.guidance.iter().any(|doc| doc.text == "Unvisited"));
    }

    #[test]
    fn missing_files_are_not_invented_and_large_contents_are_bounded() {
        let scratch = Scratch::new();
        let selected = session(&scratch.0);
        let empty = collect(&selected, &snapshot(&selected, vec![]));
        assert!(
            !empty
                .guidance
                .iter()
                .any(|doc| doc.path.starts_with(&*scratch.0.to_string_lossy()))
        );
        fs::write(scratch.0.join("AGENTS.md"), "é".repeat(FILE_BYTES)).unwrap();
        let collected = collect(&selected, &snapshot(&selected, vec![]));
        let document = collected
            .guidance
            .iter()
            .find(|doc| doc.path == scratch.0.join("AGENTS.md").to_string_lossy())
            .unwrap();
        assert!(document.truncated);
        assert!(document.text.len() <= FILE_BYTES);
        assert!(collected.truncated);
    }

    #[test]
    fn successful_reads_and_announcements_are_evidence_but_catalogs_and_mentions_are_not() {
        let scratch = Scratch::new();
        let selected = session(&scratch.0);
        let path = scratch.0.join("frontend-design/SKILL.md");
        fs::create_dir(path.parent().unwrap()).unwrap();
        fs::write(&path, "Current version").unwrap();
        let records = vec![
            message(
                "catalog",
                "context",
                "Available skills\n- frontend-design: /tmp/frontend-design/SKILL.md",
                true,
            ),
            message(
                "mention",
                "assistant",
                "The frontend-design skill is available.",
                false,
            ),
            message(
                "quoted",
                "assistant",
                "> I'm using the frontend-design skill",
                false,
            ),
            message(
                "announced",
                "assistant",
                "I'm using the frontend-design skill to refine this view.",
                false,
            ),
            tool(
                "read",
                "functions.exec_command",
                json!({"cmd":format!("cat '{}'",path.display())}),
                json!({"exit_code":0,"output":"Recorded older version"}),
            ),
            tool(
                "listing",
                "functions.exec_command",
                json!({"cmd":format!("rg --files {}",scratch.0.display())}),
                json!({"exit_code":0,"output":path}),
            ),
        ];
        let collected = collect(&selected, &snapshot(&selected, records));
        assert_eq!(collected.skills.len(), 1);
        let skill = &collected.skills[0];
        assert_eq!(skill.name, "frontend-design");
        assert_eq!(skill.kind, SkillEvidenceKind::ConfirmedRead);
        assert_eq!(skill.path.as_deref(), path.to_str());
        assert_eq!(
            skill
                .trace
                .iter()
                .map(|trace| trace.record_id.as_str())
                .collect::<Vec<_>>(),
            ["announced", "read"]
        );
        assert_eq!(
            skill.recorded_text.as_deref(),
            Some("Recorded older version")
        );
        assert_eq!(
            skill.current_document.as_ref().unwrap().text,
            "Current version"
        );
    }

    #[test]
    fn failed_unpaired_and_dynamic_reads_do_not_confirm_use() {
        let scratch = Scratch::new();
        let selected = session(&scratch.0);
        let mut unpaired = tool(
            "unpaired",
            "read_file",
            json!({"path":"/tmp/a/SKILL.md"}),
            json!("text"),
        );
        unpaired.tool_status = "Result not recorded".into();
        let mut missing_call = tool(
            "no-call",
            "read_file",
            json!({"path":"/tmp/a/SKILL.md"}),
            json!("text"),
        );
        let mut raw: Value = serde_json::from_str(&missing_call.detail).unwrap();
        raw["call"] = Value::Null;
        missing_call.detail = raw.to_string();
        let records = vec![
            tool(
                "failed",
                "read_file",
                json!({"path":"/tmp/a/SKILL.md"}),
                json!({"isError":true,"text":"missing"}),
            ),
            unpaired,
            missing_call,
            tool(
                "dynamic",
                "exec_command",
                json!({"cmd":"cat $HOME/skills/a/SKILL.md"}),
                json!({"exit_code":0,"output":"text"}),
            ),
            tool(
                "compound",
                "exec_command",
                json!({"cmd":"echo cat /tmp/a/SKILL.md; true"}),
                json!({"exit_code":0,"output":"text"}),
            ),
        ];
        assert!(
            collect(&selected, &snapshot(&selected, records))
                .skills
                .is_empty()
        );
    }

    #[test]
    fn wrapper_reads_require_explicit_nested_success() {
        let code = "text(await tools.exec_command({cmd: \"cat '/tmp/frontend-design/SKILL.md'\", workdir: '/tmp'}));";
        let no_status = json!({"name":"functions.exec","call":{},"result":{},"input":code,"output":"ordinary wrapper output"});
        assert!(successful_reads(&no_status, None).is_empty());
        let success = json!({"name":"functions.exec","call":{},"result":{},"input":code,"output":"{\"exit_code\":0,\"output\":\"Skill body\"}"});
        assert_eq!(
            successful_reads(&success, None),
            ["/tmp/frontend-design/SKILL.md"]
        );
        let failure = json!({"name":"functions.exec","call":{},"result":{},"input":code,"output":"{\"exit_code\":1,\"output\":\"missing\"}"});
        assert!(successful_reads(&failure, None).is_empty());
        let quoted = json!({"name":"functions.exec","call":{},"result":{},"input":format!("text({code:?});"),"output":"{\"exit_code\":0}"});
        assert!(successful_reads(&quoted, None).is_empty());
        let conditional = json!({"name":"functions.exec","call":{},"result":{},"input":format!("if (false) {{ {code} }}"),"output":"{\"exit_code\":0}"});
        assert!(successful_reads(&conditional, None).is_empty());
    }

    #[test]
    fn codex_override_precedes_regular_guidance_and_empty_override_falls_back() {
        let scratch = Scratch::new();
        fs::write(scratch.0.join("AGENTS.md"), "Regular").unwrap();
        fs::write(scratch.0.join("AGENTS.override.md"), "Override").unwrap();
        let document =
            read_directory_guidance(&scratch.0, "test", vec![], true, &mut Budget::new()).unwrap();
        assert!(document.path.ends_with("/AGENTS.override.md"));
        assert_eq!(document.text, "Override");
        fs::write(scratch.0.join("AGENTS.override.md"), "").unwrap();
        let fallback =
            read_directory_guidance(&scratch.0, "test", vec![], true, &mut Budget::new()).unwrap();
        assert!(fallback.path.ends_with("/AGENTS.md"));
        assert_eq!(fallback.text, "Regular");
        let non_codex =
            read_directory_guidance(&scratch.0, "test", vec![], false, &mut Budget::new()).unwrap();
        assert!(non_codex.path.ends_with("/AGENTS.md"));
    }

    #[test]
    fn codex_hierarchy_stops_at_git_root_and_non_repository_uses_only_cwd() {
        let scratch = Scratch::new();
        let repository = scratch.0.join("repo");
        let cwd = repository.join("src");
        fs::create_dir_all(&cwd).unwrap();
        assert_eq!(codex_project_chain(&cwd, &mut Budget::new()), [cwd.clone()]);
        // A .git file also denotes a worktree root; its contents are not read.
        fs::write(repository.join(".git"), "gitdir: /unrelated/location").unwrap();
        assert_eq!(
            codex_project_chain(&cwd, &mut Budget::new()),
            [repository, cwd]
        );
    }

    #[test]
    fn short_multiple_skill_announcements_are_not_catalogs_or_future_plans() {
        assert_eq!(
            announced_skills(
                "I’m using the `frontend-design` and `ui-no-slop` skills to refine this view."
            ),
            ["frontend-design", "ui-no-slop"]
        );
        assert_eq!(
            announced_skills(
                "I am applying the frontend-design, ui-no-slop, and apple-native skills."
            ),
            ["frontend-design", "ui-no-slop", "apple-native"]
        );
        assert!(announced_skills("I'll use the frontend-design skill later.").is_empty());
        assert!(
            announced_skills("Available skills\nI'm using the frontend-design skill").is_empty()
        );
    }

    #[test]
    fn truncated_private_key_blocks_are_redacted() {
        let raw = format!(
            "-----BEGIN PRIVATE KEY-----\n{}",
            "sensitive".repeat(FILE_BYTES)
        );
        let (text, cut) = Budget::new().content(&raw);
        assert!(cut);
        assert_eq!(text, "[private key redacted]");
    }

    #[test]
    fn historical_guidance_remains_separate_from_current_contents() {
        let scratch = Scratch::new();
        fs::write(scratch.0.join("AGENTS.md"), "Current instructions").unwrap();
        let selected = session(&scratch.0);
        let historical = format!(
            "# AGENTS.md instructions for {}\n\n<INSTRUCTIONS>\nOld instructions\n</INSTRUCTIONS>",
            scratch.0.display()
        );
        let collected = collect(
            &selected,
            &snapshot(
                &selected,
                vec![message("old", "context", &historical, true)],
            ),
        );
        let docs: Vec<_> = collected
            .guidance
            .iter()
            .filter(|doc| doc.path == scratch.0.join("AGENTS.md").to_string_lossy())
            .collect();
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0].source, GuidanceSource::CurrentFilesystem);
        assert_eq!(docs[0].text, "Current instructions");
        assert_eq!(docs[1].source, GuidanceSource::RecordedInstructions);
        assert_eq!(docs[1].text, "Old instructions");
    }

    #[test]
    fn identity_mismatch_is_rejected_and_secrets_are_redacted_everywhere() {
        let scratch = Scratch::new();
        fs::write(scratch.0.join("AGENTS.md"), "API_KEY=secret-value").unwrap();
        let selected = session(&scratch.0);
        let mut other = snapshot(&selected, vec![]);
        other.session.id = "another-agent".into();
        let rejected = collect(&selected, &other);
        assert_eq!(rejected.session_id, "selected-agent");
        assert!(rejected.guidance.is_empty());
        assert!(rejected.skills.is_empty());
        let record = tool(
            "safe-id",
            "read_file",
            json!({"path":"/tmp/security/SKILL.md"}),
            json!("TOKEN=private-token"),
        );
        let collected = collect(&selected, &snapshot(&selected, vec![record]));
        assert!(
            collected
                .guidance
                .iter()
                .all(|doc| !doc.text.contains("secret-value"))
        );
        assert!(
            !collected.skills[0]
                .recorded_text
                .as_ref()
                .unwrap()
                .contains("private-token")
        );
        assert!(
            collected.skills[0]
                .recorded_text
                .as_ref()
                .unwrap()
                .contains("[redacted]")
        );
    }

    #[test]
    fn file_count_is_global_across_guidance_and_skills() {
        let scratch = Scratch::new();
        let selected = session(&scratch.0);
        let mut records = Vec::new();
        for index in 0..24 {
            let path = scratch.0.join(format!("skill-{index}/SKILL.md"));
            fs::create_dir(path.parent().unwrap()).unwrap();
            fs::write(&path, "body").unwrap();
            records.push(tool(
                &format!("read-{index}"),
                "read_file",
                json!({"path":path}),
                json!("recorded"),
            ));
        }
        let collected = collect(&selected, &snapshot(&selected, records));
        assert!(
            collected
                .skills
                .iter()
                .filter(|skill| skill.current_document.is_some())
                .count()
                <= FILE_COUNT
        );
        assert!(collected.truncated);
    }
}
