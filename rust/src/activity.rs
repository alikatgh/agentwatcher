//! Small live-recording summaries. Poll on a worker, never on the GPUI thread.
use crate::telemetry::{self, Record, Session, Snapshot};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque, hash_map::DefaultHasher};
use std::fs;
use std::hash::{Hash, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};

const AGENTS: usize = 64;
const READS_PER_POLL: usize = 2;
const SAMPLES: usize = 24;
const SAMPLE_ID_BYTES: usize = 1024;
const TEXT_CHARS: usize = 160;
const TIMELINE_MINUTES: u64 = 24 * 60;
const HANDOFFS: usize = 16;
const HANDOFF_RECORDS: usize = 64;
const HANDOFF_DETAIL_BYTES: usize = 64 * 1024;
/// A recording provides a short activity lease, not process-liveness proof.
pub const LIVE_ACTIVITY_SECONDS: u64 = 120;
const RECENT_START_SECONDS: u64 = 15 * 60;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AgentLifecycle {
    /// A recent explicit start with a fresh recorded start/work timestamp.
    Active,
    /// Fresh recorded agent/tool work without a sufficiently recent start.
    Activity,
    Stopped,
    Completed,
    Error,
    #[default]
    Unknown,
}

impl AgentLifecycle {
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Active | Self::Activity)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::Activity => "Activity",
            Self::Stopped => "Stopped",
            Self::Completed => "Completed",
            Self::Error => "Error",
            Self::Unknown => "Recorded",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ActivityKind {
    User,
    Agent,
    Tool,
    Completed,
    Error,
    #[default]
    Other,
}

impl ActivityKind {
    /// Index in ActivityBucket::counts: User, Agent, Tool, Completed, Error, Other.
    pub const fn index(self) -> usize {
        match self {
            Self::User => 0,
            Self::Agent => 1,
            Self::Tool => 2,
            Self::Completed => 3,
            Self::Error => 4,
            Self::Other => 5,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActivityBucket {
    /// Unix minute of actual recorded work, independent of file modification time.
    pub minute: u64,
    /// User, Agent, Tool, Completed, Error, Other; access with ActivityKind::index.
    pub counts: [u32; 6],
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentHandoff {
    /// Exact bounded target identity from a recognized normalized agent tool.
    pub target: String,
    pub time: String,
    /// Empty when an exact safe record identity cannot be retained.
    pub record_id: String,
    pub label: String,
}

#[derive(Clone, Debug, Default)]
pub struct AgentActivity {
    pub key: (String, String),
    pub text: String,
    pub model: String,
    pub usage: Option<telemetry::Usage>,
    pub samples: Vec<ActivityKind>,
    /// Exact record identities parallel to samples; empty disables an oversized/missing ID.
    pub sample_ids: Vec<String>,
    /// At most 16 recognized handoffs from the last 64 non-infrastructure tools.
    pub handoffs: Vec<AgentHandoff>,
    /// Nonempty minute buckets in ascending order, within the current 1,440-minute window.
    pub timeline: Vec<ActivityBucket>,
    pub event_count: usize,
    pub tool_count: usize,
    pub error_count: usize,
    pub last_event_time: String,
    /// Current interpretation of exact lifecycle evidence and recorded work.
    /// Re-evaluate with lifecycle_at when rendering between collector polls.
    pub lifecycle: AgentLifecycle,
    /// Timestamp of the latest recognized lifecycle event, never file mtime.
    pub lifecycle_seconds: Option<u64>,
    /// Exact recorded time retains subsecond ordering across bounded windows.
    pub lifecycle_time: String,
    /// A fixed, recognized provider event type; never a message or raw payload.
    pub lifecycle_event: String,
    /// Exact bounded lifecycle record identity, for ordering overlapping windows.
    pub lifecycle_record_id: String,
    /// Latest timestamped non-infrastructure agent/tool work; user input and
    /// recording metadata cannot renew an agent's activity lease.
    pub last_work_seconds: Option<u64>,
    /// Observed recording modification time; this does not imply a running agent.
    pub file_modified: Option<SystemTime>,
    pub loaded: bool,
    /// True only on the poll that first successfully reads this cached agent.
    pub initial_load: bool,
    /// True only when a successful read changes retained, non-infrastructure events.
    /// Initial loads, metadata-only changes and failed reads never pulse.
    pub changed: bool,
    /// Increases on successful fingerprint changes, including the first load.
    pub revision: u64,
    pub error: Option<String>,
}

impl AgentActivity {
    pub fn lifecycle_at(&self, now_seconds: u64) -> AgentLifecycle {
        let explicit = lifecycle_kind(&self.key.0, &self.lifecycle_event);
        if let Some(
            state @ (AgentLifecycle::Stopped | AgentLifecycle::Completed | AgentLifecycle::Error),
        ) = explicit
            && self
                .lifecycle_seconds
                .is_some_and(|seconds| seconds <= now_seconds)
        {
            return state;
        }
        let fresh =
            |seconds: u64, maximum: u64| seconds <= now_seconds && now_seconds - seconds <= maximum;
        let fresh_work = self
            .last_work_seconds
            .is_some_and(|seconds| fresh(seconds, LIVE_ACTIVITY_SECONDS));
        if explicit == Some(AgentLifecycle::Active)
            && self
                .lifecycle_seconds
                .is_some_and(|seconds| fresh(seconds, RECENT_START_SECONDS))
            && (fresh_work
                || self
                    .lifecycle_seconds
                    .is_some_and(|seconds| fresh(seconds, LIVE_ACTIVITY_SECONDS)))
        {
            AgentLifecycle::Active
        } else if fresh_work {
            AgentLifecycle::Activity
        } else {
            AgentLifecycle::Unknown
        }
    }

    pub fn is_active_at(&self, now_seconds: u64) -> bool {
        self.lifecycle_at(now_seconds).is_active()
    }
}

type Key = (String, String);

#[derive(Clone, Debug, PartialEq, Eq)]
struct Fingerprint {
    path: String,
    len: u64,
    modified: SystemTime,
}

#[derive(Default)]
struct Cached {
    view: AgentActivity,
    successful: Option<Fingerprint>,
    /// Fixed-size signature only; no raw event content survives a collection.
    event_signature: Option<u64>,
    metadata_error: bool,
}

/// Retains at most 64 normalized summaries and file fingerprints. The supplied
/// order selects newest/pinned candidates; an independent persistent queue
/// gives each selected recording a turn even if that order changes each poll.
#[derive(Default)]
pub struct ActivityCollector {
    entries: HashMap<Key, Cached>,
    queue: VecDeque<Key>,
}

impl ActivityCollector {
    pub fn poll(&mut self, sessions: &[Session]) -> Vec<AgentActivity> {
        self.poll_with(sessions, metadata, telemetry::collect)
    }

    fn poll_with(
        &mut self,
        sessions: &[Session],
        metadata: impl FnMut(&Session) -> Result<Fingerprint, String>,
        collect: impl FnMut(&Session) -> Snapshot,
    ) -> Vec<AgentActivity> {
        let now_seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs());
        self.poll_at(sessions, now_seconds, metadata, collect)
    }

    fn poll_at(
        &mut self,
        sessions: &[Session],
        now_seconds: u64,
        mut metadata: impl FnMut(&Session) -> Result<Fingerprint, String>,
        mut collect: impl FnMut(&Session) -> Snapshot,
    ) -> Vec<AgentActivity> {
        let current_minute = now_seconds / 60;
        let oldest_minute = current_minute.saturating_sub(TIMELINE_MINUTES - 1);
        let mut selected = Vec::with_capacity(AGENTS);
        let mut wanted = HashSet::with_capacity(AGENTS);
        for session in sessions {
            if session.id.is_empty() {
                continue;
            }
            let key = (session.source.clone(), session.id.clone());
            if wanted.insert(key.clone()) {
                selected.push((key, session));
            }
            if selected.len() == AGENTS {
                break;
            }
        }
        self.entries.retain(|key, _| wanted.contains(key));
        self.queue.retain(|key| wanted.contains(key));
        let mut queued = self.queue.iter().cloned().collect::<HashSet<_>>();
        let mut fingerprints = HashMap::with_capacity(selected.len());
        let by_key = selected
            .iter()
            .map(|(key, session)| (key.clone(), *session))
            .collect::<HashMap<_, _>>();
        for (key, session) in &selected {
            let cached = self.entries.entry(key.clone()).or_insert_with(|| Cached {
                view: AgentActivity {
                    key: key.clone(),
                    ..AgentActivity::default()
                },
                ..Cached::default()
            });
            cached.view.initial_load = false;
            cached.view.changed = false;
            // Recorded activity expires even when file metadata is unchanged.
            cached.view.lifecycle = cached.view.lifecycle_at(now_seconds);
            // Unchanged recordings still age out without triggering an extra read.
            cached
                .view
                .timeline
                .retain(|bucket| (oldest_minute..=current_minute).contains(&bucket.minute));
            if queued.insert(key.clone()) {
                self.queue.push_back(key.clone());
            }
            if !matches!(session.source.as_str(), "codex" | "deepseek") {
                cached.view.error =
                    Some("Live file checks are unavailable for this source.".into());
                cached.metadata_error = true;
                cached.view.file_modified = None;
                continue;
            }
            match metadata(session) {
                Ok(fingerprint) => {
                    cached.view.file_modified = Some(fingerprint.modified);
                    if cached.metadata_error || cached.successful.as_ref() == Some(&fingerprint) {
                        cached.view.error = None;
                        cached.metadata_error = false;
                    }
                    fingerprints.insert(key.clone(), fingerprint);
                }
                Err(error) => {
                    cached.view.error = Some(short_error(&error));
                    cached.metadata_error = true;
                    cached.view.file_modified = None;
                }
            }
        }

        let turns = self.queue.len();
        let mut reads = 0;
        for _ in 0..turns {
            if reads == READS_PER_POLL {
                break;
            }
            let Some(key) = self.queue.pop_front() else {
                break;
            };
            self.queue.push_back(key.clone());
            let Some(fingerprint) = fingerprints.get(&key) else {
                continue;
            };
            let cached = self.entries.get_mut(&key).expect("selected activity entry");
            if cached.successful.as_ref() == Some(fingerprint) {
                continue;
            }
            reads += 1;
            let session = by_key[&key];
            let snapshot = collect(session);
            if let Some(error) = &snapshot.error {
                cached.view.error = Some(short_error(error));
                cached.metadata_error = false;
                continue;
            }
            if snapshot.session.id != session.id || snapshot.session.source != session.source {
                cached.view.error =
                    Some("The recording returned a different agent identity.".into());
                cached.metadata_error = false;
                continue;
            }
            let first = cached.successful.is_none();
            let revision = cached.view.revision.saturating_add(1);
            let signature = event_signature(&snapshot.records);
            let changed = cached
                .event_signature
                .is_some_and(|previous| previous != signature);
            let mut next = summarize(
                &snapshot,
                key,
                fingerprint.modified,
                first,
                revision,
                now_seconds,
            );
            merge_lifecycle(&cached.view, &mut next, now_seconds);
            cached.view = next;
            cached.view.changed = !first && changed;
            cached.event_signature = Some(signature);
            // Commit the pre-read fingerprint: if the file grew during collect,
            // its new metadata is still eligible for the next bounded poll.
            cached.successful = Some(fingerprint.clone());
            cached.metadata_error = false;
            // Snapshot and all its records are dropped here, never cached.
        }
        selected
            .into_iter()
            .map(|(key, _)| self.entries[&key].view.clone())
            .collect()
    }
}

fn metadata(session: &Session) -> Result<Fingerprint, String> {
    let info = fs::metadata(&session.path)
        .map_err(|error| format!("Recording metadata unavailable: {error}"))?;
    if !info.is_file() {
        return Err("The recording is not a regular file.".into());
    }
    let modified = info
        .modified()
        .map_err(|error| format!("Recording modification time unavailable: {error}"))?;
    Ok(Fingerprint {
        path: session.path.clone(),
        len: info.len(),
        modified,
    })
}

fn summarize(
    snapshot: &Snapshot,
    key: Key,
    modified: SystemTime,
    first: bool,
    revision: u64,
    now_seconds: u64,
) -> AgentActivity {
    let mut view = AgentActivity {
        key,
        loaded: true,
        initial_load: first,
        revision,
        file_modified: Some(modified),
        event_count: snapshot.records.len(),
        model: short(&telemetry::redact(&snapshot.model), TEXT_CHARS),
        usage: snapshot.usage.clone(),
        ..AgentActivity::default()
    };
    let mut samples = VecDeque::with_capacity(SAMPLES);
    let mut sample_ids = VecDeque::with_capacity(SAMPLES);
    let mut timeline = BTreeMap::<u64, [u32; 6]>::new();
    let current_minute = now_seconds / 60;
    let oldest_minute = current_minute.saturating_sub(TIMELINE_MINUTES - 1);
    let mut text_time = String::new();
    for record in &snapshot.records {
        let kind = classify(record);
        if let Some(evidence) = lifecycle_evidence(record, &view.key, now_seconds) {
            retain_lifecycle(&mut view, evidence);
        }
        if !record.is_infrastructure {
            if let Some(seconds) = timestamp_seconds(&record.time) {
                if seconds <= now_seconds
                    && matches!(
                        record.kind.as_str(),
                        "assistant" | "agent" | "agent_message" | "tool"
                    )
                {
                    view.last_work_seconds = Some(view.last_work_seconds.unwrap_or(0).max(seconds));
                }
                let minute = seconds / 60;
                if seconds <= now_seconds && (oldest_minute..=current_minute).contains(&minute) {
                    let counts = timeline.entry(minute).or_default();
                    counts[kind.index()] = counts[kind.index()].saturating_add(1);
                }
            }
        }
        if record.kind == "tool" {
            view.tool_count += 1;
        }
        if kind == ActivityKind::Error {
            view.error_count += 1;
        }
        if samples.len() == SAMPLES {
            samples.pop_front();
            sample_ids.pop_front();
        }
        samples.push_back(kind);
        sample_ids.push_back(if record.id.len() <= SAMPLE_ID_BYTES {
            record.id.clone()
        } else {
            String::new()
        });
        if !record.time.is_empty() && later_or_unordered(&record.time, &view.last_event_time) {
            view.last_event_time = short(&telemetry::redact(&record.time), TEXT_CHARS);
        }
        if record.is_infrastructure
            || !matches!(
                record.kind.as_str(),
                "user" | "assistant" | "agent" | "agent_message" | "tool"
            )
        {
            continue;
        }
        let text = if record.kind == "tool" {
            let name = safe_summary(record.summary.split('·').next().unwrap_or(""))
                .map(|name| short(&name, 64))
                .unwrap_or_else(|| "Tool event".into());
            Some(format!(
                "{name} · {}",
                match record.tool_status.as_str() {
                    "Completed" => "Completed",
                    "Error" => "Error",
                    "Result not recorded" => "Result not recorded",
                    "Call not recorded" => "Call not recorded",
                    _ => "Recorded",
                }
            ))
        } else {
            safe_summary(&record.summary)
        };
        if let Some(text) = text {
            if view.text.is_empty() || later_or_unordered(&record.time, &text_time) {
                view.text = text;
                text_time = record.time.clone();
            }
        }
    }
    view.samples = samples.into();
    view.sample_ids = sample_ids.into();
    view.handoffs = recent_handoffs(&snapshot.records);
    view.timeline = timeline
        .into_iter()
        .map(|(minute, counts)| ActivityBucket { minute, counts })
        .collect();
    if view.text.is_empty() {
        view.text = "No message text recorded".into();
    }
    view.lifecycle = view.lifecycle_at(now_seconds);
    view
}

#[derive(Deserialize)]
struct LifecycleRecord<'a> {
    #[serde(rename = "type")]
    kind: Option<&'a str>,
    timestamp: Option<&'a str>,
    thread_id: Option<&'a str>,
    session_id: Option<&'a str>,
    #[serde(borrow)]
    payload: Option<LifecyclePayload<'a>>,
}

#[derive(Deserialize)]
struct LifecyclePayload<'a> {
    #[serde(rename = "type")]
    kind: Option<&'a str>,
    thread_id: Option<&'a str>,
    session_id: Option<&'a str>,
}

struct LifecycleEvent<'a> {
    event: &'a str,
    time: &'a str,
    seconds: u64,
    record_id: &'a str,
}

/// Only lifecycle schemas supported by the provider's recorded runtime apply.
/// Tool completion, assistant prose, child reports and request metadata never
/// imply that the containing agent completed or started a task.
fn lifecycle_kind(source: &str, event: &str) -> Option<AgentLifecycle> {
    if source != "codex" {
        // DeepSeek v4 currently records work but supplies no validated whole-
        // agent lifecycle schema here. Keep its fresh work as Activity.
        return None;
    }
    match event {
        "task_started" | "turn.started" => Some(AgentLifecycle::Active),
        "task_complete" | "turn.completed" => Some(AgentLifecycle::Completed),
        "turn_aborted" | "task_cancelled" | "task_canceled" | "turn.cancelled"
        | "turn.canceled" => Some(AgentLifecycle::Stopped),
        "task_failed" | "turn.failed" | "error" => Some(AgentLifecycle::Error),
        _ => None,
    }
}

fn lifecycle_evidence<'a>(
    record: &'a Record,
    key: &Key,
    now_seconds: u64,
) -> Option<LifecycleEvent<'a>> {
    if key.0 != "codex"
        || !record.is_infrastructure
        || !matches!(record.kind.as_str(), "runtime" | "context")
    {
        return None;
    }
    // Borrow only metadata strings. Unknown fields (including a large final
    // message) are skipped without making another retained payload allocation.
    let raw: LifecycleRecord<'a> = serde_json::from_str(&record.detail).ok()?;
    let time = raw.timestamp?;
    let order = timestamp_order(time)?;
    if order.0 > now_seconds || timestamp_order(&record.time) != Some(order) {
        return None;
    }
    let payload = raw.payload.as_ref();
    for identity in [
        raw.thread_id,
        raw.session_id,
        payload.and_then(|payload| payload.thread_id),
        payload.and_then(|payload| payload.session_id),
    ]
    .into_iter()
    .flatten()
    {
        if identity != key.1 {
            return None;
        }
    }
    let event = if raw.kind == Some("event_msg") {
        let event = payload?.kind?;
        if !matches!(
            event,
            "task_started"
                | "task_complete"
                | "turn_aborted"
                | "task_cancelled"
                | "task_canceled"
                | "task_failed"
                | "error"
        ) {
            return None;
        }
        event
    } else {
        let event = raw.kind?;
        if !matches!(
            event,
            "turn.started"
                | "turn.completed"
                | "turn.failed"
                | "turn.cancelled"
                | "turn.canceled"
                | "error"
        ) {
            return None;
        }
        event
    };
    lifecycle_kind(&key.0, event)?;
    Some(LifecycleEvent {
        event,
        time,
        seconds: order.0,
        record_id: &record.id,
    })
}

fn lifecycle_offset(source: &str, id: &str) -> Option<u64> {
    id.strip_prefix(source)?.strip_prefix('-')?.parse().ok()
}

fn evidence_is_later(
    source: &str,
    next_time: &str,
    next_id: &str,
    previous_time: &str,
    previous_id: &str,
) -> bool {
    let (Some(next), Some(previous)) = (timestamp_order(next_time), timestamp_order(previous_time))
    else {
        return previous_time.is_empty() && !next_time.is_empty();
    };
    if next < previous {
        return false;
    }
    match (
        lifecycle_offset(source, next_id),
        lifecycle_offset(source, previous_id),
    ) {
        // Telemetry IDs include stable original byte positions. Older pages or
        // delayed records cannot reopen a newer terminal event.
        (Some(next_offset), Some(previous_offset)) => {
            next_offset >= previous_offset && (next > previous || next_offset > previous_offset)
        }
        _ => next > previous,
    }
}

fn retain_lifecycle(view: &mut AgentActivity, evidence: LifecycleEvent<'_>) {
    if view.lifecycle_event.is_empty()
        || evidence_is_later(
            &view.key.0,
            evidence.time,
            evidence.record_id,
            &view.lifecycle_time,
            &view.lifecycle_record_id,
        )
    {
        view.lifecycle_event = evidence.event.into();
        view.lifecycle_time = evidence.time.into();
        view.lifecycle_seconds = Some(evidence.seconds);
        view.lifecycle_record_id = safe_identity(evidence.record_id).unwrap_or_default();
    }
}

fn merge_lifecycle(previous: &AgentActivity, next: &mut AgentActivity, now_seconds: u64) {
    if previous.key != next.key {
        return;
    }
    if !previous.lifecycle_event.is_empty()
        && (next.lifecycle_event.is_empty()
            || !evidence_is_later(
                &next.key.0,
                &next.lifecycle_time,
                &next.lifecycle_record_id,
                &previous.lifecycle_time,
                &previous.lifecycle_record_id,
            ))
    {
        next.lifecycle_event = previous.lifecycle_event.clone();
        next.lifecycle_time = previous.lifecycle_time.clone();
        next.lifecycle_seconds = previous.lifecycle_seconds;
        next.lifecycle_record_id = previous.lifecycle_record_id.clone();
    }
    next.last_work_seconds = previous
        .last_work_seconds
        .into_iter()
        .chain(next.last_work_seconds)
        .filter(|seconds| *seconds <= now_seconds)
        .max();
    next.lifecycle = next.lifecycle_at(now_seconds);
}

fn timestamp_order(value: &str) -> Option<(u64, u32)> {
    let seconds = timestamp_seconds(value)?;
    let fraction = if value.bytes().all(|byte| byte.is_ascii_digit()) {
        let number = value.parse::<u64>().ok()?;
        if number >= 100_000_000_000 {
            ((number % 1000) * 1_000_000) as u32
        } else {
            0
        }
    } else {
        let mut fraction = 0;
        let mut count = 0;
        if let Some(tail) = value.get(19..).and_then(|tail| tail.strip_prefix('.')) {
            for byte in tail.bytes().take_while(u8::is_ascii_digit).take(9) {
                fraction = fraction * 10 + u32::from(byte - b'0');
                count += 1;
            }
        }
        fraction * 10u32.pow(9 - count)
    };
    Some((seconds, fraction))
}

fn recent_handoffs(records: &[Record]) -> Vec<AgentHandoff> {
    let mut handoffs = records
        .iter()
        .rev()
        .filter(|record| record.kind == "tool" && !record.is_infrastructure)
        .take(HANDOFF_RECORDS)
        .filter_map(extract_handoff)
        .take(HANDOFFS)
        .collect::<Vec<_>>();
    handoffs.reverse();
    handoffs
}

fn extract_handoff(record: &Record) -> Option<AgentHandoff> {
    if record.kind != "tool"
        || record.tool_status != "Completed"
        || record.is_infrastructure
        || record.detail.len() > HANDOFF_DETAIL_BYTES
    {
        return None;
    }
    let detail: serde_json::Value = serde_json::from_str(&record.detail).ok()?;
    // These fields are produced by telemetry's normalized, paired tool schema.
    // Never infer communication from prose, summaries or arbitrary tool output.
    if !detail.get("call").is_some_and(serde_json::Value::is_object) {
        return None;
    }
    let input = detail.get("input")?.as_object()?;
    let name = detail
        .get("name")?
        .as_str()?
        .rsplit(['.', ':', '/'])
        .next()?;
    let direct_target = || {
        input
            .get("target")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .or_else(|| input.get("recipient").and_then(serde_json::Value::as_str))
    };
    let (target, label) = match name {
        "send_message" => (direct_target()?, "Message"),
        "followup_task" => (direct_target()?, "Task"),
        "interrupt_agent" => (direct_target()?, "Interrupt"),
        "spawn_agent" => (
            detail
                .get("output")
                .and_then(|output| output.get("agent_id"))
                .and_then(serde_json::Value::as_str)
                .or_else(|| input.get("task_name").and_then(serde_json::Value::as_str))?,
            "Spawn",
        ),
        _ => return None,
    };
    Some(AgentHandoff {
        target: safe_identity(target)?,
        time: short_bytes(&telemetry::redact(&record.time), TEXT_CHARS),
        record_id: safe_identity(&record.id).unwrap_or_default(),
        label: label.into(),
    })
}

fn safe_identity(value: &str) -> Option<String> {
    if value.trim().is_empty() || value.len() > SAMPLE_ID_BYTES {
        return None;
    }
    let redacted = telemetry::redact(value);
    // A transformed or truncated identity could connect a different real agent.
    (redacted == value).then_some(redacted)
}

fn short_bytes(value: &str, maximum: usize) -> String {
    if value.len() <= maximum {
        return value.to_owned();
    }
    let mut end = maximum.saturating_sub('…'.len_utf8());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}

/// Fixed-size signature of retained non-infrastructure event content, excluding
/// positional IDs. Reuse for activity signals without retaining raw records.
pub fn event_signature(records: &[Record]) -> u64 {
    let mut hash = DefaultHasher::new();
    for record in records.iter().filter(|record| !record.is_infrastructure) {
        // Some provider IDs are derived from raw file positions. Inserting an
        // infrastructure record can renumber them without changing visible work.
        (
            &record.time,
            &record.kind,
            &record.summary,
            &record.detail,
            &record.tool_status,
        )
            .hash(&mut hash);
    }
    hash.finish()
}

/// Classify a retained event consistently for charts and scoped trace inspection.
pub fn classify(record: &Record) -> ActivityKind {
    if record.kind == "tool" {
        return match record.tool_status.as_str() {
            "Error" => ActivityKind::Error,
            "Completed" => ActivityKind::Completed,
            _ => ActivityKind::Tool,
        };
    }
    if record.is_infrastructure {
        return ActivityKind::Other;
    }
    match record.kind.as_str() {
        "user" => ActivityKind::User,
        "assistant" | "agent" | "agent_message" => ActivityKind::Agent,
        "error" => ActivityKind::Error,
        _ => ActivityKind::Other,
    }
}

/// Parse recorded ISO 8601 timestamps or integer Unix seconds/milliseconds.
/// Fractions are truncated to whole seconds. Numeric values at or above
/// 100,000,000,000 are milliseconds, matching the providers' recording formats.
/// Invalid calendar/time fields, trailing data, overflow and pre-epoch UTC reject.
pub fn timestamp_seconds(value: &str) -> Option<u64> {
    const MAX_SECONDS: u64 = 253_402_300_799; // 9999-12-31T23:59:59Z
    let bytes = value.as_bytes();
    if !bytes.is_empty() && bytes.iter().all(u8::is_ascii_digit) {
        let number = value.parse::<u64>().ok()?;
        let seconds = if number >= 100_000_000_000 {
            number / 1000
        } else {
            number
        };
        return (seconds <= MAX_SECONDS).then_some(seconds);
    }
    if bytes.len() < 20
        || bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || bytes.get(10) != Some(&b'T')
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
    {
        return None;
    }
    let digits = |start: usize, count: usize| -> Option<i64> {
        bytes
            .get(start..start + count)?
            .iter()
            .try_fold(0, |number, byte| {
                byte.is_ascii_digit()
                    .then(|| number * 10 + i64::from(byte - b'0'))
            })
    };
    let year = digits(0, 4)?;
    let month = digits(5, 2)?;
    let day = digits(8, 2)?;
    let hour = digits(11, 2)?;
    let minute = digits(14, 2)?;
    let second = digits(17, 2)?;
    if year == 0 || !(1..=12).contains(&month) || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days = match month {
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if day == 0 || day > month_days {
        return None;
    }
    let mut suffix = 19;
    if bytes.get(suffix) == Some(&b'.') {
        suffix += 1;
        let start = suffix;
        while bytes.get(suffix).is_some_and(u8::is_ascii_digit) {
            suffix += 1;
        }
        if suffix == start {
            return None;
        }
    }
    let offset = match bytes.get(suffix) {
        Some(b'Z') if bytes.len() == suffix + 1 => 0,
        Some(sign @ (b'+' | b'-'))
            if bytes.len() == suffix + 6 && bytes.get(suffix + 3) == Some(&b':') =>
        {
            let hours = digits(suffix + 1, 2)?;
            let minutes = digits(suffix + 4, 2)?;
            if hours > 23 || minutes > 59 {
                return None;
            }
            (hours * 3600 + minutes * 60) * if *sign == b'+' { 1 } else { -1 }
        }
        _ => return None,
    };
    // Gregorian civil date to Unix days; the March-based year keeps leap-day
    // handling within each complete 400-year era.
    let adjusted_year = year - i64::from(month <= 2);
    let era = adjusted_year / 400;
    let year_of_era = adjusted_year - era * 400;
    let march_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * march_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second - offset;
    let seconds = u64::try_from(seconds).ok()?;
    (seconds <= MAX_SECONDS).then_some(seconds)
}

// Compare timestamps only when they use a common recorded format. Otherwise
// file order wins. Numeric recordings may use Unix seconds or milliseconds.
fn later_or_unordered(next: &str, previous: &str) -> bool {
    if next.is_empty() || previous.is_empty() {
        return true;
    }
    if let (Ok(next), Ok(previous)) = (next.parse::<u128>(), previous.parse::<u128>()) {
        let millis = |value: u128| {
            if value > 100_000_000_000 {
                value
            } else {
                value.saturating_mul(1000)
            }
        };
        return millis(next) >= millis(previous);
    }
    if next.as_bytes().get(4) == Some(&b'-') && previous.as_bytes().get(4) == Some(&b'-') {
        if let (Some(next_second), Some(previous_second)) = (next.get(..19), previous.get(..19)) {
            if next_second != previous_second {
                return next_second > previous_second;
            }
            let fraction = |value: &str| {
                let digits = value
                    .get(19..)
                    .and_then(|tail| tail.strip_prefix('.'))
                    .unwrap_or("");
                let mut result = 0u32;
                let mut count = 0;
                for byte in digits.bytes().take_while(u8::is_ascii_digit).take(9) {
                    result = result * 10 + u32::from(byte - b'0');
                    count += 1;
                }
                for _ in count..9 {
                    result *= 10;
                }
                result
            };
            return fraction(next) >= fraction(previous);
        }
    }
    true
}

fn safe_summary(value: &str) -> Option<String> {
    let value = telemetry::redact(value);
    let value = value.trim();
    let lower = value.to_ascii_lowercase();
    if value.is_empty()
        || value.starts_with("Message Type:")
        || value.starts_with(['{', '['])
        || [
            "data:image/",
            "\"image_url\"",
            "\"input_image\"",
            "\"output_image\"",
            "\"image_base64\"",
            "\"b64_json\"",
            "\"base64\"",
        ]
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return None;
    }
    if value.split_whitespace().any(|word| {
        let word = word.trim_matches(|character: char| {
            matches!(character, '"' | '\'' | '(' | ')' | ',' | ';')
        });
        word.len() >= 64
            && word
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"+/=_-".contains(&byte))
    }) {
        return None;
    }
    let text = value.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(short(&text, TEXT_CHARS))
}

fn short_error(value: &str) -> String {
    safe_summary(value).unwrap_or_else(|| "The recording could not be read.".into())
}

fn short(value: &str, maximum: usize) -> String {
    if value.chars().count() <= maximum {
        return value.to_owned();
    }
    let mut text = value
        .chars()
        .take(maximum.saturating_sub(1))
        .collect::<String>();
    text.push('…');
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::time::{Duration, UNIX_EPOCH};

    fn session(id: usize) -> Session {
        Session {
            id: format!("agent-{id}"),
            source: "codex".into(),
            path: format!("/synthetic/agent-{id}.jsonl"),
            ..Session::default()
        }
    }

    fn stamp(session: &Session, revision: u64) -> Fingerprint {
        Fingerprint {
            path: session.path.clone(),
            len: 10,
            modified: UNIX_EPOCH + Duration::from_secs(revision),
        }
    }

    fn snapshot(session: &Session, text: &str) -> Snapshot {
        Snapshot {
            session: session.clone(),
            records: vec![Record {
                kind: "assistant".into(),
                summary: text.into(),
                ..Record::default()
            }],
            ..Snapshot::default()
        }
    }

    fn handoff_record(
        id: usize,
        name: &str,
        input: serde_json::Value,
        output: serde_json::Value,
    ) -> Record {
        Record {
            id: format!("tool-{id}"),
            time: "2026-10-14T12:00:00Z".into(),
            kind: "tool".into(),
            tool_status: "Completed".into(),
            detail: serde_json::json!({"name": name, "input": input, "output": output, "call": {}, "result": {}}).to_string(),
            ..Record::default()
        }
    }

    fn lifecycle_record(event: &str, seconds: u64, offset: u64) -> Record {
        let time = seconds.to_string();
        Record {
            id: format!("codex-{offset}"),
            time: time.clone(),
            kind: "runtime".into(),
            detail: serde_json::json!({
                "type": "event_msg", "timestamp": time, "payload": {"type": event}
            })
            .to_string(),
            is_infrastructure: true,
            ..Record::default()
        }
    }

    fn work_record(seconds: u64) -> Record {
        Record {
            kind: "assistant".into(),
            time: seconds.to_string(),
            summary: "Recorded agent work".into(),
            ..Record::default()
        }
    }

    fn lifecycle_view(session: &Session, records: Vec<Record>, now: u64) -> AgentActivity {
        summarize(
            &Snapshot {
                session: session.clone(),
                records,
                ..Snapshot::default()
            },
            (session.source.clone(), session.id.clone()),
            // A fresh mtime intentionally does not establish agent activity.
            UNIX_EPOCH + Duration::from_secs(now),
            true,
            1,
            now,
        )
    }

    #[test]
    fn explicit_start_has_a_recorded_activity_lease_that_ages_without_reads() {
        let now = 1_700_000_000;
        let view = lifecycle_view(
            &session(1),
            vec![lifecycle_record("task_started", now, 1)],
            now,
        );
        assert_eq!(view.lifecycle, AgentLifecycle::Active);
        assert_eq!(view.lifecycle_seconds, Some(now));
        assert_eq!(view.lifecycle_event, "task_started");
        assert!(view.is_active_at(now + LIVE_ACTIVITY_SECONDS));
        assert_eq!(
            view.lifecycle_at(now + LIVE_ACTIVITY_SECONDS + 1),
            AgentLifecycle::Unknown
        );
        let refreshed = lifecycle_view(
            &session(1),
            vec![
                lifecycle_record("task_started", now, 1),
                work_record(now + 200),
            ],
            now + 200,
        );
        assert_eq!(refreshed.lifecycle, AgentLifecycle::Active);
        assert_eq!(refreshed.last_work_seconds, Some(now + 200));
    }

    #[test]
    fn old_start_never_becomes_current_running_evidence_from_a_new_message() {
        let now = 1_700_000_000;
        let old_start = lifecycle_record("task_started", now - RECENT_START_SECONDS - 1, 1);
        let stale = lifecycle_view(&session(1), vec![old_start.clone()], now);
        assert_eq!(stale.lifecycle, AgentLifecycle::Unknown);
        let recent_work = lifecycle_view(&session(1), vec![old_start, work_record(now)], now);
        assert_eq!(recent_work.lifecycle, AgentLifecycle::Activity);
        assert_eq!(recent_work.lifecycle_at(now + 121), AgentLifecycle::Unknown);
        let historical = lifecycle_view(&session(1), vec![work_record(now - 3600)], now);
        assert_eq!(historical.lifecycle, AgentLifecycle::Unknown);
    }

    #[test]
    fn explicit_completion_stop_cancel_and_error_are_distinct_terminal_states() {
        let now = 1_700_000_000;
        for (event, expected) in [
            ("task_complete", AgentLifecycle::Completed),
            ("turn_aborted", AgentLifecycle::Stopped),
            ("task_cancelled", AgentLifecycle::Stopped),
            ("task_canceled", AgentLifecycle::Stopped),
            ("task_failed", AgentLifecycle::Error),
            ("error", AgentLifecycle::Error),
        ] {
            let view = lifecycle_view(
                &session(1),
                vec![
                    lifecycle_record("task_started", now - 10, 1),
                    lifecycle_record(event, now - 1, 2),
                    work_record(now),
                ],
                now,
            );
            assert_eq!(view.lifecycle, expected, "{event}");
            assert!(!view.is_active_at(now), "{event}");
            assert_eq!(view.lifecycle_at(now + 86400), expected, "{event}");
        }
    }

    #[test]
    fn codex_turn_stream_requires_exact_top_level_lifecycle_and_timestamp() {
        let now = 1_700_000_000;
        for (event, expected) in [
            ("turn.started", AgentLifecycle::Active),
            ("turn.completed", AgentLifecycle::Completed),
            ("turn.failed", AgentLifecycle::Error),
            ("turn.cancelled", AgentLifecycle::Stopped),
        ] {
            let mut record = lifecycle_record(event, now, 1);
            record.detail =
                serde_json::json!({"type": event, "timestamp": now.to_string()}).to_string();
            assert_eq!(
                lifecycle_view(&session(1), vec![record], now).lifecycle,
                expected
            );
        }
    }

    #[test]
    fn a_terminal_event_survives_older_windows_until_a_new_explicit_start() {
        let now = 1_700_000_000;
        let previous = lifecycle_view(
            &session(1),
            vec![lifecycle_record("task_complete", now - 5, 50)],
            now,
        );
        for records in [
            vec![lifecycle_record("task_started", now - 10, 10)],
            vec![lifecycle_record("task_started", now - 1, 10)],
            vec![lifecycle_record("task_started", now - 10, 60)],
            vec![work_record(now)],
        ] {
            let mut next = lifecycle_view(&session(1), records, now);
            merge_lifecycle(&previous, &mut next, now);
            assert_eq!(next.lifecycle, AgentLifecycle::Completed);
            assert_eq!(next.lifecycle_record_id, "codex-50");
        }
        let mut resumed = lifecycle_view(
            &session(1),
            vec![lifecycle_record("task_started", now - 1, 60)],
            now,
        );
        merge_lifecycle(&previous, &mut resumed, now);
        assert_eq!(resumed.lifecycle, AgentLifecycle::Active);
    }

    #[test]
    fn lifecycle_order_retains_subseconds_and_stable_original_record_positions() {
        let now = timestamp_seconds("2026-10-14T12:00:00Z").unwrap();
        let mut start = lifecycle_record("task_started", now, 10);
        start.time = "2026-10-14T12:00:00.100Z".into();
        start.detail = serde_json::json!({"type":"event_msg","timestamp":start.time,"payload":{"type":"task_started"}}).to_string();
        let mut completed = lifecycle_record("task_complete", now, 20);
        completed.time = "2026-10-14T12:00:00.200Z".into();
        completed.detail = serde_json::json!({"type":"event_msg","timestamp":completed.time,"payload":{"type":"task_complete"}}).to_string();
        let previous = lifecycle_view(&session(1), vec![start.clone(), completed], now);
        assert_eq!(previous.lifecycle, AgentLifecycle::Completed);
        start.id = "codex-30".into();
        let mut delayed = lifecycle_view(&session(1), vec![start], now);
        merge_lifecycle(&previous, &mut delayed, now);
        assert_eq!(delayed.lifecycle, AgentLifecycle::Completed);
        assert_eq!(
            timestamp_order("1700000000123"),
            Some((1_700_000_000, 123_000_000))
        );
    }

    #[test]
    fn lifecycle_never_comes_from_prose_tools_child_reports_or_other_provider_identity() {
        let now = 1_700_000_000;
        let mut prose = lifecycle_record("task_started", now, 1);
        prose.kind = "assistant".into();
        prose.is_infrastructure = false;
        let mut tool = lifecycle_record("task_complete", now, 2);
        tool.kind = "tool".into();
        tool.is_infrastructure = false;
        tool.tool_status = "Completed".into();
        let mut child = lifecycle_record("task_complete", now, 3);
        child.detail = serde_json::json!({"type":"event_msg","timestamp":now.to_string(),"payload":{"type":"item_completed","item":{"type":"SubAgentActivity","kind":"completed"}}}).to_string();
        let mut another_agent = lifecycle_record("task_started", now, 4);
        another_agent.detail = serde_json::json!({"type":"event_msg","timestamp":now.to_string(),"payload":{"type":"task_started","thread_id":"agent-2"}}).to_string();
        for record in [prose, tool, child, another_agent] {
            let view = lifecycle_view(&session(1), vec![record], now);
            assert!(view.lifecycle_event.is_empty());
            assert_ne!(view.lifecycle, AgentLifecycle::Active);
            assert_ne!(view.lifecycle, AgentLifecycle::Completed);
        }
        let mut deepseek = session(1);
        deepseek.source = "deepseek".into();
        let other_provider = lifecycle_view(
            &deepseek,
            vec![lifecycle_record("task_started", now, 1), work_record(now)],
            now,
        );
        assert_eq!(other_provider.lifecycle, AgentLifecycle::Activity);
        assert!(other_provider.lifecycle_event.is_empty());
        let mut cross_provider = lifecycle_view(&deepseek, vec![], now);
        let codex_terminal = lifecycle_view(
            &session(1),
            vec![lifecycle_record("task_complete", now, 1)],
            now,
        );
        merge_lifecycle(&codex_terminal, &mut cross_provider, now);
        assert_eq!(cross_provider.lifecycle, AgentLifecycle::Unknown);
    }

    #[test]
    fn metadata_user_input_future_and_invalid_times_cannot_create_live_activity() {
        let now = 1_700_000_000;
        let mut metadata = work_record(now);
        metadata.is_infrastructure = true;
        let mut user = work_record(now);
        user.kind = "user".into();
        let mut invalid = lifecycle_record("task_started", now, 1);
        invalid.time = "invalid time".into();
        let mut mismatched = lifecycle_record("task_started", now, 2);
        mismatched.time = (now - 1).to_string();
        for records in [
            vec![],
            vec![metadata, user],
            vec![invalid, mismatched],
            vec![
                lifecycle_record("task_started", now + 1, 3),
                work_record(now + 1),
            ],
        ] {
            let view = lifecycle_view(&session(1), records, now);
            assert_eq!(view.lifecycle, AgentLifecycle::Unknown);
            assert_eq!(view.last_work_seconds, None);
            assert!(view.lifecycle_event.is_empty());
        }
    }

    #[test]
    fn collector_ages_live_state_without_reading_an_unchanged_recording() {
        let now = 1_700_000_000;
        let sessions = [session(1)];
        let mut collector = ActivityCollector::default();
        let initial = collector.poll_at(
            &sessions,
            now,
            |session| Ok(stamp(session, 1)),
            |session| Snapshot {
                session: session.clone(),
                records: vec![lifecycle_record("task_started", now, 1)],
                ..Snapshot::default()
            },
        );
        assert_eq!(initial[0].lifecycle, AgentLifecycle::Active);
        let aged = collector.poll_at(
            &sessions,
            now + LIVE_ACTIVITY_SECONDS + 1,
            |session| Ok(stamp(session, 1)),
            |_| panic!("Unchanged recordings must not be re-read to age a lease"),
        );
        assert_eq!(aged[0].lifecycle, AgentLifecycle::Unknown);
    }

    #[test]
    fn handoffs_use_only_known_normalized_tool_names_and_exact_schema_targets() {
        use serde_json::json;
        let records = vec![
            handoff_record(
                0,
                "functions.collaboration.send_message",
                json!({"target": "/root/task", "message": "Payload never cached"}),
                json!(null),
            ),
            handoff_record(
                1,
                "collaboration/followup_task",
                json!({"recipient": "agent-2"}),
                json!(null),
            ),
            handoff_record(
                2,
                "collaboration::spawn_agent",
                json!({"task_name": "fallback-task"}),
                json!({"agent_id": "actual-session"}),
            ),
            handoff_record(
                3,
                "interrupt_agent",
                json!({"target": "agent-4"}),
                json!(null),
            ),
            handoff_record(
                4,
                "spawn_agent",
                json!({"task_name": "recorded-task"}),
                json!({"session_id": "unverified-field"}),
            ),
        ];
        let handoffs = recent_handoffs(&records);
        assert_eq!(
            handoffs
                .iter()
                .map(|handoff| (handoff.target.as_str(), handoff.label.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("/root/task", "Message"),
                ("agent-2", "Task"),
                ("actual-session", "Spawn"),
                ("agent-4", "Interrupt"),
                ("recorded-task", "Spawn"),
            ]
        );
        assert_eq!(handoffs[0].record_id, "tool-0");
        assert_eq!(handoffs[0].time, "2026-10-14T12:00:00Z");
        assert!(!format!("{handoffs:?}").contains("Payload never cached"));
    }

    #[test]
    fn handoffs_never_infer_agent_edges_from_generic_text_or_unverified_json() {
        use serde_json::json;
        let mut prose = handoff_record(0, "send_message", json!({"target": "agent"}), json!(null));
        prose.detail = "send_message to agent; target=agent".into();
        prose.summary = "send_message · agent".into();
        let mut missing_call =
            handoff_record(1, "send_message", json!({"target": "agent"}), json!(null));
        missing_call.detail =
            json!({"name": "send_message", "input": {"target": "agent"}}).to_string();
        let mut infrastructure =
            handoff_record(2, "send_message", json!({"target": "agent"}), json!(null));
        infrastructure.is_infrastructure = true;
        let mut assistant =
            handoff_record(3, "send_message", json!({"target": "agent"}), json!(null));
        assistant.kind = "assistant".into();
        let records = vec![
            prose,
            missing_call,
            infrastructure,
            assistant,
            handoff_record(
                4,
                "exec",
                json!({"target": "agent", "message": "send_message"}),
                json!(null),
            ),
            handoff_record(
                5,
                "send_message",
                json!("{\"target\":\"agent\"}"),
                json!(null),
            ),
            handoff_record(6, "send_message", json!({"target": 123}), json!(null)),
            handoff_record(
                7,
                "spawn_agent",
                json!({}),
                json!({"text": "agent_id: invented"}),
            ),
        ];
        assert!(recent_handoffs(&records).is_empty());
    }

    #[test]
    fn failed_pending_and_unpaired_agent_tools_never_become_delivered_handoffs() {
        use serde_json::json;
        let mut record = handoff_record(0, "send_message", json!({"target": "agent"}), json!(null));
        assert!(extract_handoff(&record).is_some());
        for status in ["Error", "Result not recorded", "Call not recorded", ""] {
            record.tool_status = status.into();
            assert!(extract_handoff(&record).is_none(), "{status}");
            assert!(recent_handoffs(&[record.clone()]).is_empty(), "{status}");
        }
    }

    #[test]
    fn handoff_scan_and_retained_fields_obey_byte_and_record_bounds() {
        use serde_json::json;
        let records = (0..40)
            .map(|id| {
                handoff_record(
                    id,
                    "send_message",
                    json!({"target": format!("agent-{id}")}),
                    json!(null),
                )
            })
            .collect::<Vec<_>>();
        let handoffs = recent_handoffs(&records);
        assert_eq!(handoffs.len(), HANDOFFS);
        assert_eq!(handoffs[0].record_id, "tool-24");
        assert_eq!(handoffs.last().unwrap().record_id, "tool-39");
        let mut old = vec![records[0].clone()];
        old.extend(
            (0..HANDOFF_RECORDS).map(|id| handoff_record(id, "exec", json!({}), json!(null))),
        );
        assert!(recent_handoffs(&old).is_empty());
        let oversized = handoff_record(
            0,
            "send_message",
            json!({"target": "日".repeat(342)}),
            json!(null),
        );
        assert!(extract_handoff(&oversized).is_none());
        let mut bounded = handoff_record(
            0,
            "send_message",
            json!({"target": "valid-agent"}),
            json!(null),
        );
        bounded.id = "日".repeat(342);
        bounded.time = "日".repeat(200);
        let handoff = extract_handoff(&bounded).unwrap();
        assert!(handoff.record_id.is_empty());
        assert!(handoff.time.len() <= TEXT_CHARS);
        let mut oversized_detail = records[0].clone();
        oversized_detail
            .detail
            .push_str(&" ".repeat(HANDOFF_DETAIL_BYTES));
        assert!(extract_handoff(&oversized_detail).is_none());
    }

    #[test]
    fn timestamp_parser_normalizes_codex_iso_and_deepseek_unix_units() {
        for value in [
            "2023-11-14T22:13:20Z",
            "2023-11-14T22:13:20.123456789Z",
            "2023-11-15T06:13:20+08:00",
            "2023-11-14T15:13:20.500-07:00",
            "1700000000",
            "1700000000123",
        ] {
            assert_eq!(timestamp_seconds(value), Some(1_700_000_000), "{value}");
        }
        assert_eq!(timestamp_seconds("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(timestamp_seconds("0"), Some(0));
        assert_eq!(timestamp_seconds("2000-02-29T00:00:00Z"), Some(951_782_400));
        assert_eq!(
            timestamp_seconds("2024-02-29T00:00:00Z"),
            Some(1_709_164_800)
        );
        assert_eq!(
            timestamp_seconds("9999-12-31T23:59:59Z"),
            Some(253_402_300_799)
        );
        // Offsets are applied before deciding whether a timestamp precedes Unix time.
        assert_eq!(timestamp_seconds("1969-12-31T23:00:00-01:00"), Some(0));
    }

    #[test]
    fn timestamp_parser_rejects_invalid_dates_times_suffixes_and_overflow() {
        for value in [
            "",
            "-1",
            "+1700000000",
            "1700000000.5",
            " 1700000000",
            "1700000000 ",
            "18446744073709551616",
            "18446744073709551615",
            "１７００００００００",
            "2023-02-29T00:00:00Z",
            "2100-02-29T00:00:00Z",
            "2024-02-30T00:00:00Z",
            "2024-00-01T00:00:00Z",
            "2024-13-01T00:00:00Z",
            "2024-01-00T00:00:00Z",
            "2024-04-31T00:00:00Z",
            "0000-01-01T00:00:00Z",
            "1969-12-31T23:59:59Z",
            "2024-01-01T24:00:00Z",
            "2024-01-01T00:60:00Z",
            "2024-01-01T00:00:60Z",
            "2024-01-01T00:00:00.Z",
            "2024-01-01T00:00:00.12xZ",
            "2024-01-01T00:00:00",
            "2024-01-01T00:00:00Zjunk",
            "2024-01-01T00:00:00+24:00",
            "2024-01-01T00:00:00+01:60",
            "2024-01-01T00:00:00+0100",
            "2024-01-01 00:00:00Z",
            "2024-01-01T00:00:00é",
            "9999-12-31T23:59:59-00:01",
        ] {
            assert_eq!(timestamp_seconds(value), None, "{value}");
        }
    }

    #[test]
    fn timeline_uses_actual_minutes_and_normalized_kinds_without_infrastructure() {
        let selected = session(0);
        let base = 1_700_000_100u64; // A minute boundary.
        let now = base + 45;
        let mut data = snapshot(&selected, "ignored");
        data.records = [
            ("user", ""),
            ("assistant", ""),
            ("tool", ""),
            ("tool", "Completed"),
            ("tool", "Error"),
            ("status", ""),
            ("user", ""),
        ]
        .into_iter()
        .map(|(kind, status)| Record {
            kind: kind.into(),
            tool_status: status.into(),
            time: (base + 10).to_string(),
            ..Record::default()
        })
        .collect();
        data.records.extend([
            Record {
                kind: "tool".into(),
                time: (base - 60).to_string(),
                ..Record::default()
            },
            Record {
                kind: "tool".into(),
                tool_status: "Error".into(),
                time: base.to_string(),
                is_infrastructure: true,
                ..Record::default()
            },
            Record {
                kind: "user".into(),
                time: (now + 1).to_string(),
                ..Record::default()
            },
            Record {
                kind: "user".into(),
                time: (base - TIMELINE_MINUTES * 60).to_string(),
                ..Record::default()
            },
            Record {
                kind: "user".into(),
                time: "not a timestamp".into(),
                ..Record::default()
            },
        ]);
        let view = summarize(
            &data,
            (selected.source, selected.id),
            UNIX_EPOCH,
            true,
            1,
            now,
        );
        assert_eq!(
            view.timeline,
            vec![
                ActivityBucket {
                    minute: base / 60 - 1,
                    counts: [0, 0, 1, 0, 0, 0]
                },
                ActivityBucket {
                    minute: base / 60,
                    counts: [2, 1, 1, 1, 1, 1]
                },
            ]
        );
        assert_eq!(view.event_count, 12); // The pre-existing summary includes all records.
        assert_eq!(view.error_count, 2);
        assert_eq!(view.tool_count, 5);
    }

    #[test]
    fn timeline_is_bounded_to_1440_clock_minutes_and_never_uses_metadata_time() {
        let selected = session(0);
        let base = 1_700_000_100u64;
        let now = base + 45;
        let mut data = snapshot(&selected, "ignored");
        data.records = (0..2000)
            .rev()
            .map(|index| Record {
                kind: "assistant".into(),
                time: (base - index * 60).to_string(),
                ..Record::default()
            })
            .collect();
        let view = summarize(
            &data,
            (selected.source.clone(), selected.id.clone()),
            UNIX_EPOCH,
            true,
            1,
            now,
        );
        assert_eq!(view.timeline.len(), TIMELINE_MINUTES as usize);
        assert_eq!(view.timeline.first().unwrap().minute, base / 60 - 1439);
        assert_eq!(view.timeline.last().unwrap().minute, base / 60);
        assert!(
            view.timeline
                .iter()
                .all(|bucket| bucket.counts == [0, 1, 0, 0, 0, 0])
        );
        assert!(
            view.timeline
                .windows(2)
                .all(|pair| pair[0].minute + 1 == pair[1].minute)
        );
        data.records = vec![Record {
            kind: "assistant".into(),
            time: "".into(),
            ..Record::default()
        }];
        let modified = UNIX_EPOCH + Duration::from_secs(now);
        let view = summarize(
            &data,
            (selected.source, selected.id),
            modified,
            true,
            1,
            now,
        );
        assert!(view.timeline.is_empty());
        assert_eq!(view.file_modified, Some(modified));
    }

    #[test]
    fn unchanged_cached_timelines_age_out_without_extra_reads_or_revisions() {
        let selected = session(0);
        let base = 1_700_000_100u64;
        let now = base + 45;
        let mut data = snapshot(&selected, "Recorded work");
        data.records[0].time = base.to_string();
        data.records.push(Record {
            kind: "tool".into(),
            time: (base - 1439 * 60).to_string(),
            ..Record::default()
        });
        let mut collector = ActivityCollector::default();
        let sessions = [selected];
        let first = collector.poll_at(
            &sessions,
            now,
            |session| Ok(stamp(session, 1)),
            |_| data.clone(),
        );
        assert_eq!(first[0].timeline.len(), 2);
        let aged = collector.poll_at(
            &sessions,
            now + 60,
            |session| Ok(stamp(session, 1)),
            |_| panic!("aging an unchanged file must not reparse it"),
        );
        assert_eq!(aged[0].timeline.len(), 1);
        assert_eq!(aged[0].timeline[0].minute, base / 60);
        assert_eq!(aged[0].revision, 1);
        assert!(!aged[0].changed && !aged[0].initial_load);
        let expired = collector.poll_at(
            &sessions,
            now + 24 * 60 * 60,
            |session| Ok(stamp(session, 1)),
            |_| panic!("expiring an unchanged file must not reparse it"),
        );
        assert!(expired[0].timeline.is_empty());
        assert_eq!(expired[0].revision, 1);
    }

    #[test]
    fn reads_only_two_per_poll_and_round_robin_survives_input_reordering() {
        let mut collector = ActivityCollector::default();
        let mut sessions = (0..64).map(session).collect::<Vec<_>>();
        let mut read_ids = Vec::new();
        for tick in 1..=32 {
            let before = read_ids.len();
            let activities = collector.poll_with(
                &sessions,
                |session| Ok(stamp(session, tick)),
                |session| {
                    read_ids.push(session.id.clone());
                    snapshot(session, "Recorded work")
                },
            );
            assert_eq!(read_ids.len() - before, 2);
            assert_eq!(activities.len(), 64);
            sessions.reverse();
        }
        assert_eq!(read_ids.iter().collect::<HashSet<_>>().len(), 64);
        assert!(collector.entries.values().all(|entry| entry.view.loaded));
        assert_eq!(collector.queue.len(), 64);
    }

    #[test]
    fn unchanged_recordings_are_not_reparsed_and_first_load_is_not_a_new_update() {
        let mut collector = ActivityCollector::default();
        let sessions = vec![session(0)];
        let reads = Cell::new(0);
        let first = collector.poll_with(
            &sessions,
            |session| Ok(stamp(session, 1)),
            |session| {
                reads.set(reads.get() + 1);
                snapshot(session, "First")
            },
        );
        assert!(first[0].initial_load && first[0].loaded);
        assert!(!first[0].changed);
        assert_eq!(first[0].revision, 1);
        let same = collector.poll_with(
            &sessions,
            |session| Ok(stamp(session, 1)),
            |_| panic!("unchanged file was reparsed"),
        );
        assert!(!same[0].initial_load && !same[0].changed);
        assert_eq!(same[0].revision, 1);
        let next = collector.poll_with(
            &sessions,
            |session| Ok(stamp(session, 2)),
            |session| {
                reads.set(reads.get() + 1);
                snapshot(session, "Next")
            },
        );
        assert!(next[0].changed && !next[0].initial_load);
        assert_eq!(next[0].revision, 2);
        assert_eq!(reads.get(), 2);
        let length_changed = collector.poll_with(
            &sessions,
            |session| {
                let mut fingerprint = stamp(session, 2);
                fingerprint.len += 1;
                Ok(fingerprint)
            },
            |session| snapshot(session, "Length changed"),
        );
        assert!(length_changed[0].changed);
        assert_eq!(length_changed[0].revision, 3);
    }

    #[test]
    fn touching_or_rewriting_identical_events_updates_revision_without_a_change_pulse() {
        let mut collector = ActivityCollector::default();
        let sessions = [session(0)];
        let reads = Cell::new(0);
        for revision in 1..=3 {
            let views = collector.poll_with(
                &sessions,
                |session| {
                    let mut fingerprint = stamp(session, revision);
                    // The second poll changes mtime only; the third rewrites length too.
                    if revision == 3 {
                        fingerprint.len += 100;
                    }
                    Ok(fingerprint)
                },
                |session| {
                    reads.set(reads.get() + 1);
                    snapshot(session, "The same recorded work")
                },
            );
            assert!(views[0].loaded);
            assert_eq!(views[0].initial_load, revision == 1);
            assert!(!views[0].changed);
            assert_eq!(views[0].revision, revision);
        }
        assert_eq!(reads.get(), 3);
    }

    #[test]
    fn a_real_retained_event_edit_pulses_even_when_its_summary_stays_the_same() {
        let mut collector = ActivityCollector::default();
        let sessions = [session(0)];
        let mut data = snapshot(&sessions[0], "exec · Completed");
        data.records[0].kind = "tool".into();
        data.records[0].tool_status = "Completed".into();
        data.records[0].detail = "First exact tool result".into();
        let first =
            collector.poll_with(&sessions, |session| Ok(stamp(session, 1)), |_| data.clone());
        assert!(!first[0].changed);
        data.records[0].detail = "A changed exact tool result".into();
        let changed =
            collector.poll_with(&sessions, |session| Ok(stamp(session, 2)), |_| data.clone());
        assert!(changed[0].changed && !changed[0].initial_load);
        assert_eq!(changed[0].text, first[0].text);
        assert_eq!(changed[0].revision, 2);
    }

    #[test]
    fn infrastructure_edits_and_positional_id_shifts_do_not_pulse_recorded_work() {
        let mut collector = ActivityCollector::default();
        let sessions = [session(0)];
        let mut data = snapshot(&sessions[0], "Visible agent work");
        data.records[0].id = "codex-1".into();
        let first =
            collector.poll_with(&sessions, |session| Ok(stamp(session, 1)), |_| data.clone());
        assert!(!first[0].changed);
        data.records.insert(
            0,
            Record {
                id: "codex-1".into(),
                kind: "runtime".into(),
                time: "1700000000".into(),
                summary: "Updated infrastructure metadata".into(),
                detail: "Different runtime settings".into(),
                is_infrastructure: true,
                ..Record::default()
            },
        );
        data.records[1].id = "codex-2".into();
        let metadata =
            collector.poll_with(&sessions, |session| Ok(stamp(session, 2)), |_| data.clone());
        assert!(!metadata[0].changed && !metadata[0].initial_load);
        assert_eq!(metadata[0].revision, 2);
        assert_eq!(metadata[0].text, "Visible agent work");
        data.records[0].detail = "A further infrastructure-only edit".into();
        let edited =
            collector.poll_with(&sessions, |session| Ok(stamp(session, 3)), |_| data.clone());
        assert!(!edited[0].changed);
        assert_eq!(edited[0].revision, 3);
    }

    #[test]
    fn failures_preserve_prior_summary_and_retry_without_claiming_an_update() {
        let mut collector = ActivityCollector::default();
        let sessions = vec![session(0)];
        collector.poll_with(
            &sessions,
            |session| Ok(stamp(session, 1)),
            |session| snapshot(session, "Known good activity"),
        );
        let failed = collector.poll_with(
            &sessions,
            |session| Ok(stamp(session, 2)),
            |session| Snapshot {
                session: session.clone(),
                error: Some("API_KEY=synthetic-secret read failed".into()),
                ..Snapshot::default()
            },
        );
        assert_eq!(failed[0].text, "Known good activity");
        assert_eq!(failed[0].revision, 1);
        assert!(!failed[0].changed && failed[0].loaded);
        assert!(
            !failed[0]
                .error
                .as_ref()
                .unwrap()
                .contains("synthetic-secret")
        );
        let recovered = collector.poll_with(
            &sessions,
            |session| Ok(stamp(session, 2)),
            |session| snapshot(session, "Recovered"),
        );
        assert!(recovered[0].changed);
        assert_eq!(recovered[0].revision, 2);
        assert!(recovered[0].error.is_none());
        let missing = collector.poll_with(
            &sessions,
            |_| Err("Recording metadata unavailable".into()),
            |_| panic!("metadata failure must not parse"),
        );
        assert_eq!(missing[0].text, "Recovered");
        assert_eq!(missing[0].revision, 2);
        assert!(missing[0].error.is_some());
        let restored = collector.poll_with(
            &sessions,
            |session| Ok(stamp(session, 2)),
            |_| panic!("restored unchanged file must use its successful summary"),
        );
        assert!(restored[0].error.is_none());
        assert!(!restored[0].changed);
    }

    #[test]
    fn selection_is_bounded_deduplicated_and_prunes_removed_agents() {
        let mut collector = ActivityCollector::default();
        let mut sessions = (0..70).map(session).collect::<Vec<_>>();
        sessions.insert(0, session(0));
        let activities = collector.poll_with(
            &sessions,
            |session| Ok(stamp(session, 1)),
            |session| snapshot(session, "Work"),
        );
        assert_eq!(activities.len(), 64);
        assert_eq!(collector.entries.len(), 64);
        assert_eq!(activities[0].key.1, "agent-0");
        let selected = vec![session(68), session(69)];
        let activities = collector.poll_with(
            &selected,
            |session| Ok(stamp(session, 1)),
            |session| snapshot(session, "Selected work"),
        );
        assert_eq!(activities.len(), 2);
        assert_eq!(collector.entries.len(), 2);
        assert_eq!(collector.queue.len(), 2);
        collector.poll_with(&[], |_| panic!("no metadata"), |_| panic!("no collect"));
        assert!(collector.entries.is_empty() && collector.queue.is_empty());
    }

    #[test]
    fn samples_and_text_are_bounded_and_errors_use_normalized_tool_status() {
        let selected = session(0);
        let mut data = snapshot(&selected, "ignored");
        data.records = (0..60)
            .map(|index| Record {
                kind: "tool".into(),
                tool_status: if index % 3 == 0 { "Error" } else { "Completed" }.into(),
                summary: format!("exec · {}", index),
                time: (1_700_000_000_000u64 + index).to_string(),
                ..Record::default()
            })
            .collect();
        data.records.push(Record {
            kind: "assistant".into(),
            summary: "An error mentioned in ordinary prose".into(),
            time: "1700000000061".into(),
            ..Record::default()
        });
        data.records.push(Record {
            kind: "assistant".into(),
            summary: format!("Latest readable message {}", "word ".repeat(100)),
            time: "1700000000062".into(),
            ..Record::default()
        });
        data.records.push(Record {
            kind: "assistant".into(),
            summary: "{\"image_url\":\"data:image/png;base64,synthetic-transport\"}".into(),
            time: "1700000000063".into(),
            ..Record::default()
        });
        let view = summarize(
            &data,
            (selected.source, selected.id),
            UNIX_EPOCH,
            true,
            1,
            1_700_000_001,
        );
        assert_eq!(view.event_count, 63);
        assert_eq!(view.tool_count, 60);
        assert_eq!(view.error_count, 20);
        assert_eq!(view.samples.len(), 24);
        assert_eq!(view.samples.last(), Some(&ActivityKind::Agent));
        assert!(view.text.starts_with("Latest readable message"));
        assert!(view.text.chars().count() <= 160);
        assert!(!view.text.contains("synthetic-transport"));
        assert_eq!(view.last_event_time, "1700000000063");
    }

    #[test]
    fn sample_ids_keep_last_24_kinds_aligned_and_disable_oversized_utf8_identities() {
        let selected = session(0);
        let mut data = snapshot(&selected, "ignored");
        data.records = (0..60)
            .map(|index| Record {
                id: format!("record-{index}"),
                kind: if index % 2 == 0 { "assistant" } else { "user" }.into(),
                ..Record::default()
            })
            .collect();
        let maximum_utf8_id = format!("{}a", "日".repeat(341));
        assert_eq!(maximum_utf8_id.len(), SAMPLE_ID_BYTES);
        data.records[36].id = maximum_utf8_id.clone();
        data.records[37].id = "日".repeat(342); // 342 characters, but 1,026 UTF-8 bytes.
        data.records[40].is_infrastructure = true;
        let view = summarize(
            &data,
            (selected.source, selected.id),
            UNIX_EPOCH,
            true,
            1,
            1_700_000_000,
        );
        assert_eq!(view.samples.len(), SAMPLES);
        assert_eq!(view.sample_ids.len(), SAMPLES);
        assert_eq!(view.sample_ids[0], maximum_utf8_id);
        assert!(view.sample_ids[1].is_empty());
        for (slot, index) in (36..60).enumerate() {
            let expected = if index == 40 {
                ActivityKind::Other
            } else if index % 2 == 0 {
                ActivityKind::Agent
            } else {
                ActivityKind::User
            };
            assert_eq!(view.samples[slot], expected);
            if index >= 38 {
                assert_eq!(view.sample_ids[slot], format!("record-{index}"));
            }
        }
        assert!(view.sample_ids.iter().all(|id| id.len() <= SAMPLE_ID_BYTES));
    }

    #[test]
    fn paired_tools_use_completion_time_instead_of_their_old_record_position() {
        let selected = session(0);
        let mut data = snapshot(&selected, "Newer user message");
        data.records = vec![
            Record {
                kind: "tool".into(),
                tool_status: "Completed".into(),
                summary: "exec · Completed · Latest tool result".into(),
                time: "1700000000003".into(),
                ..Record::default()
            },
            Record {
                kind: "user".into(),
                summary: "Earlier user message".into(),
                time: "1700000000002".into(),
                ..Record::default()
            },
        ];
        let view = summarize(
            &data,
            (selected.source, selected.id),
            UNIX_EPOCH,
            false,
            2,
            1_700_000_001,
        );
        assert_eq!(view.text, "exec · Completed");
        assert!(!view.text.contains("Latest tool result"));
        assert_eq!(view.last_event_time, "1700000000003");
        assert!(later_or_unordered(
            "2026-10-14T12:00:00.123Z",
            "2026-10-14T12:00:00Z"
        ));
        assert!(!later_or_unordered(
            "2026-10-14T12:00:00Z",
            "2026-10-14T12:00:00.123Z"
        ));
    }

    #[test]
    fn opencode_never_invokes_metadata_or_external_collection() {
        let mut selected = session(0);
        selected.source = "opencode".into();
        let mut collector = ActivityCollector::default();
        let views = collector.poll_with(
            &[selected],
            |_| panic!("unsupported provider metadata"),
            |_| panic!("OpenCode CLI must never be polled"),
        );
        assert!(!views[0].loaded && !views[0].changed);
        assert!(views[0].error.is_some());
    }

    #[test]
    fn a_failed_initial_read_and_cross_agent_result_cannot_create_a_false_update() {
        let mut collector = ActivityCollector::default();
        let sessions = vec![session(0)];
        let failed = collector.poll_with(
            &sessions,
            |session| Ok(stamp(session, 1)),
            |session| Snapshot {
                session: session.clone(),
                error: Some("Unavailable recording".into()),
                ..Snapshot::default()
            },
        );
        assert!(!failed[0].loaded && !failed[0].initial_load && !failed[0].changed);
        assert_eq!(failed[0].revision, 0);
        let first = collector.poll_with(
            &sessions,
            |session| Ok(stamp(session, 1)),
            |session| snapshot(session, "Agent zero"),
        );
        assert!(first[0].initial_load && !first[0].changed);
        assert_eq!(first[0].revision, 1);
        let wrong = collector.poll_with(
            &sessions,
            |session| Ok(stamp(session, 2)),
            |_| snapshot(&session(1), "Another agent's activity"),
        );
        assert_eq!(wrong[0].key.1, "agent-0");
        assert_eq!(wrong[0].text, "Agent zero");
        assert_eq!(wrong[0].revision, 1);
        assert!(!wrong[0].changed);
        assert!(
            wrong[0]
                .error
                .as_ref()
                .unwrap()
                .contains("different agent identity")
        );
    }
}
