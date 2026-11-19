//! Explicit, per-session handoff plans. Constructing a plan executes nothing.
//! The provider receives no automatic prompt or permission-bypass flags.

use crate::backend::{self, ProcessCommand, ProcessInfo};
use crate::telemetry::Session;
use std::ffi::{CStr, CString};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_PLAN_BYTES: usize = 64 * 1024;
const MAX_PLAN_FILES: usize = 512;
const MAX_DIRECTORY_BYTES: u64 = 4 * 1024 * 1024;
const DSH_CLI: &str = "/Applications/DeepSeek Harness.app/Contents/Resources/runtime/cli/bin/dsh";

#[derive(Clone, Debug)]
pub struct TerminalPlan {
    pub script: String,
    pub title: String,
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

fn installed_cli(source: &str) -> Result<PathBuf, String> {
    let mut candidates = match source {
        "codex" => vec![
            PathBuf::from("/opt/homebrew/bin/codex"),
            PathBuf::from("/usr/local/bin/codex"),
            PathBuf::from("/Applications/Codex.app/Contents/Resources/codex"),
        ],
        "deepseek" => vec![PathBuf::from(DSH_CLI)],
        "opencode" => vec![
            PathBuf::from("/opt/homebrew/bin/opencode"),
            PathBuf::from("/usr/local/bin/opencode"),
        ],
        _ => return Err("This provider does not have a verified session handoff.".into()),
    };
    if let Some(home) = home() {
        if source == "opencode" {
            candidates.insert(0, home.join(".opencode/bin/opencode"));
        }
        if source == "codex" {
            candidates.push(home.join(".local/bin/codex"));
        }
    }
    candidates.into_iter().find(|path| fs::metadata(path).is_ok_and(|metadata|
        path.is_absolute() && metadata.is_file() && metadata.permissions().mode() & 0o111 != 0))
        .ok_or_else(|| format!("The installed {source} command is unavailable. Open the provider app to continue this agent."))
}

fn valid_session_id(source: &str, id: &str) -> bool {
    if id.len() > 160 {
        return false;
    }
    let tail = |value: &str| {
        !value.is_empty()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
    };
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
        "deepseek" => id.strip_prefix("session-").is_some_and(tail),
        "opencode" => id.strip_prefix("ses_").is_some_and(tail),
        _ => false,
    }
}

fn quote(value: &str) -> Result<String, String> {
    if value.contains('\0') {
        return Err("A command argument contains a null byte.".into());
    }
    Ok(format!("'{}'", value.replace('\'', "'\"'\"'")))
}

fn plan_with_cli(
    session: &Session,
    fresh: bool,
    executable: &Path,
) -> Result<TerminalPlan, String> {
    if !fresh && !valid_session_id(&session.source, &session.id) {
        return Err("This recorded session has an invalid provider identity.".into());
    }
    if session.project.is_empty()
        || !Path::new(&session.project).is_absolute()
        || session.project.contains('\0')
    {
        return Err("This recording does not contain an absolute workspace. Select a session with a recorded workspace.".into());
    }
    let executable = executable
        .to_str()
        .filter(|path| Path::new(path).is_absolute())
        .ok_or("The provider command must have an absolute UTF-8 path.")?;
    let (provider, arguments, instruction): (&str, Vec<&str>, &str) = match session.source.as_str()
    {
        "codex" => (
            "Codex",
            if fresh {
                vec!["-C", &session.project]
            } else {
                vec!["resume", &session.id, "-C", &session.project]
            },
            "Review the session and enter your next instruction. AgentWatcher has not submitted a prompt.",
        ),
        "opencode" => (
            "OpenCode",
            if fresh {
                vec![&session.project]
            } else {
                vec![&session.project, "--session", &session.id]
            },
            "Review the session and enter your next instruction. AgentWatcher has not submitted a prompt.",
        ),
        "deepseek" => {
            if !fresh {
                return Err("The installed DeepSeek Harness cannot hand off this exact recorded agent: its headless runner rejects desktop agent presets, and the app exposes no session deep link. Continue this agent in DeepSeek Harness.".into());
            }
            (
                "DeepSeek Harness",
                vec!["--profile", "headless", "-"],
                "Enter the task, then press Control-D to submit it. DeepSeek Harness will run one turn and exit. No task has been submitted automatically.",
            )
        }
        _ => return Err("This provider does not have a verified session handoff.".into()),
    };
    let command = std::iter::once(executable)
        .chain(arguments)
        .map(quote)
        .collect::<Result<Vec<_>, _>>()?
        .join(" ");
    let script = format!(
        "#!/bin/sh\nset -eu\ncd -- {}\nprintf '%s\\n' {}\nexec {command}\n",
        quote(&session.project)?,
        quote(instruction)?
    );
    if script.len() > MAX_PLAN_BYTES {
        return Err("This handoff exceeds the command-file size limit.".into());
    }
    Ok(TerminalPlan {
        script,
        title: format!(
            "{} {provider} agent",
            if fresh { "New" } else { "Continue" }
        ),
    })
}

/// Resume the exact recorded identity, or open a fresh agent in that recording's
/// workspace. The resulting script waits for user input in the provider CLI.
pub fn terminal_plan(session: &Session, fresh: bool) -> Result<TerminalPlan, String> {
    if !Path::new(&session.project).is_dir() {
        return Err("This session's recorded workspace is unavailable. Restore that folder before continuing the agent.".into());
    }
    plan_with_cli(session, fresh, &installed_cli(&session.source)?)
}

struct DirectoryStream(*mut libc::DIR);
impl Drop for DirectoryStream {
    fn drop(&mut self) {
        unsafe {
            libc::closedir(self.0);
        }
    }
}

struct DirectoryLock<'a>(&'a File);
impl<'a> DirectoryLock<'a> {
    fn acquire(directory: &'a File) -> Result<Self, String> {
        if unsafe { libc::flock(directory.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == -1 {
            let error = std::io::Error::last_os_error();
            return Err(if error.kind() == std::io::ErrorKind::WouldBlock {
                "Another handoff is updating this directory. Try again.".into()
            } else {
                format!("Could not lock private handoff directory: {error}")
            });
        }
        Ok(Self(directory))
    }
}
impl Drop for DirectoryLock<'_> {
    fn drop(&mut self) {
        // Closing our descriptor alone cannot release flock while a concurrent
        // fork retains the same open-file description before exec. Unlock it
        // explicitly so a completed handoff never leaves that inherited lock.
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

fn directory_usage(directory: &File) -> Result<(usize, u64), String> {
    let duplicated = unsafe { libc::fcntl(directory.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
    if duplicated == -1 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let stream = unsafe { libc::fdopendir(duplicated) };
    if stream.is_null() {
        let error = std::io::Error::last_os_error();
        unsafe {
            libc::close(duplicated);
        }
        return Err(error.to_string());
    }
    let stream = DirectoryStream(stream);
    let mut files = 0;
    let mut bytes = 0u64;
    loop {
        // Clear errno so the terminating null distinguishes EOF from a failure.
        #[cfg(target_os = "macos")]
        unsafe {
            *libc::__error() = 0;
        }
        #[cfg(target_os = "linux")]
        unsafe {
            *libc::__errno_location() = 0;
        }
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error().unwrap_or(0) != 0 {
                return Err(format!("Could not inspect handoff directory: {error}"));
            }
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if name.to_bytes() == b"." || name.to_bytes() == b".." {
            continue;
        }
        files += 1;
        if files >= MAX_PLAN_FILES {
            return Err("The handoff directory has reached its 512-file limit.".into());
        }
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
        if metadata.st_mode & libc::S_IFMT != libc::S_IFREG {
            return Err("The private handoff directory contains a non-regular entry.".into());
        }
        bytes = bytes.saturating_add(metadata.st_size.max(0) as u64);
        if bytes > MAX_DIRECTORY_BYTES {
            return Err("The handoff directory has reached its 4 MiB limit.".into());
        }
    }
    Ok((files, bytes))
}

/// Materialize one new .command file in an app-owned, private directory. Existing
/// files are never replaced or pruned. Bounds apply across all retained plans.
pub fn write_plan_in(plan: &TerminalPlan, directory: &Path) -> Result<PathBuf, String> {
    if !directory.is_absolute() {
        return Err("The handoff directory must be an absolute private path.".into());
    }
    if !plan.script.starts_with("#!/bin/sh\n")
        || plan.script.contains('\0')
        || plan.script.len() > MAX_PLAN_BYTES
    {
        return Err("The handoff script is invalid or exceeds its 64 KiB limit.".into());
    }
    // Missing application-support parents are normal on a fresh install.
    // Recursive DirBuilder applies mode only to newly created directories;
    // existing parents and their contents/permissions remain untouched.
    match fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)
    {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(format!(
                "Could not create private handoff directory: {error}"
            ));
        }
    }
    let directory_file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(directory)
        .map_err(|error| format!("Could not open private handoff directory: {error}"))?;
    let metadata = directory_file
        .metadata()
        .map_err(|error| error.to_string())?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::getuid() }
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(
            "The handoff directory must be owned by the current user with mode 0700.".into(),
        );
    }
    let _lock = DirectoryLock::acquire(&directory_file)?;
    let (_, existing_bytes) = directory_usage(&directory_file)?;
    if existing_bytes.saturating_add(plan.script.len() as u64) > MAX_DIRECTORY_BYTES {
        return Err("The handoff directory has reached its 4 MiB limit.".into());
    }
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let name = format!(
        "agentwatcher-{}-{stamp}-{}.command",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let filename = CString::new(name.as_bytes()).map_err(|error| error.to_string())?;
    let descriptor = unsafe {
        libc::openat(
            directory_file.as_raw_fd(),
            filename.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o700,
        )
    };
    if descriptor == -1 {
        return Err(format!(
            "Could not create handoff file: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mut file = unsafe { File::from_raw_fd(descriptor) };
    let written = (|| {
        file.set_permissions(fs::Permissions::from_mode(0o700))?;
        file.write_all(plan.script.as_bytes())?;
        file.sync_all()
    })();
    if let Err(error) = written {
        // Only this create_new file, anchored to this opened directory, is ours.
        unsafe {
            libc::unlinkat(directory_file.as_raw_fd(), filename.as_ptr(), 0);
        }
        return Err(format!("Could not write handoff file: {error}"));
    }
    Ok(directory.join(name))
}

fn command_addresses_session(session: &Session, command: &ProcessCommand) -> bool {
    let executable = command
        .executable
        .file_name()
        .and_then(|name| name.to_str());
    if executable != Some(session.source.as_str()) {
        return false;
    }
    let arguments = command.argv.get(1..).unwrap_or_default();
    match session.source.as_str() {
        // A fork names the source conversation but runs a different one, and
        // app-server hosts many conversations. Neither proves this identity.
        "codex" => {
            arguments.first().is_some_and(|arg| arg == "resume")
                && arguments.get(1).is_some_and(|id| id == &session.id)
        }
        "opencode" => {
            let mut selected = None;
            let mut project = None;
            let mut index = 0;
            while let Some(argument) = arguments.get(index) {
                match argument.as_str() {
                    "--session" | "-s" => {
                        if selected.is_some() {
                            return false;
                        }
                        selected = arguments.get(index + 1).map(String::as_str);
                        if selected.is_none() {
                            return false;
                        }
                        index += 2;
                    }
                    "--model" | "-m" | "--agent" | "--prompt" | "--port" | "--hostname"
                    | "--mdns-domain" | "--cors" | "--log-level" => {
                        if arguments.get(index + 1).is_none() {
                            return false;
                        }
                        index += 2;
                    }
                    "--continue" | "-c" | "--mdns" | "--pure" | "--print-logs" => index += 1,
                    // Forked sessions, subcommands and unknown flag grammars
                    // cannot establish the selected original session's owner.
                    "--fork" | "--" => return false,
                    value if value.starts_with("--session=") => {
                        if selected.is_some() {
                            return false;
                        }
                        selected = value.strip_prefix("--session=");
                        index += 1;
                    }
                    value if value.starts_with('-') => return false,
                    value => {
                        if project.is_some()
                            || value != session.project
                            || !Path::new(value).is_absolute()
                        {
                            return false;
                        }
                        project = Some(value);
                        index += 1;
                    }
                }
            }
            selected == Some(session.id.as_str())
        }
        // The installed DeepSeek desktop binary owns several agents, and no
        // standalone exact-session cancel CLI is provided. Never kill that app.
        _ => false,
    }
}

fn session_process_with(
    session: &Session,
    processes: &[ProcessInfo],
    verified_executable: &Path,
    inspect: impl Fn(&ProcessInfo) -> Option<ProcessCommand>,
) -> Option<ProcessInfo> {
    if !matches!(session.source.as_str(), "codex" | "opencode")
        || !valid_session_id(&session.source, &session.id)
    {
        return None;
    }
    let mut matched = None;
    for process in processes.iter().take(512) {
        // This text check only bounds expensive native reads; it never admits
        // a match. Exact native argv and executable are the proof below.
        if process.identity_token.is_empty() || !process.command.contains(&session.id) {
            continue;
        }
        let Some(command) = inspect(process) else {
            continue;
        };
        if command.executable == verified_executable && command_addresses_session(session, &command)
        {
            if matched.is_some() {
                return None;
            }
            matched = Some(process.clone());
        }
    }
    matched
}

/// Return a retained row only when exact native executable/argv prove its
/// selected session, and only one process matches. This performs no signaling.
/// Shared provider apps and ambiguous matches deliberately return None.
pub fn session_process(session: &Session, processes: &[ProcessInfo]) -> Option<ProcessInfo> {
    let executable = fs::canonicalize(installed_cli(&session.source).ok()?).ok()?;
    session_process_with(session, processes, &executable, backend::process_command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_graph_agent_needs_workspace_and_provider_but_no_recorded_identity() {
        for source in ["codex", "deepseek", "opencode"] {
            let session = Session {
                source: source.into(),
                project: "/workspaces/project".into(),
                ..Session::default()
            };
            let plan = plan_with_cli(&session, true, Path::new("/installed/provider")).unwrap();
            assert!(plan.script.contains("'/workspaces/project'"));
            assert!(!plan.script.contains("'resume'"));
            assert!(plan_with_cli(&session, false, Path::new("/installed/provider")).is_err());
        }
        let missing_workspace = Session {
            source: "codex".into(),
            ..Session::default()
        };
        assert!(plan_with_cli(&missing_workspace, true, Path::new("/installed/provider")).is_err());
    }

    fn session(source: &str) -> Session {
        Session {
            source: source.into(),
            id: match source {
                "codex" => "01234567-89ab-cdef-0123-456789abcdef",
                "deepseek" => "session-fixture_123",
                _ => "ses_fixture_123",
            }
            .into(),
            project: "/workspaces/a 'quoted' $value; $(literal)\nfolder".into(),
            ..Session::default()
        }
    }

    #[test]
    fn plans_preserve_recorded_workspace_and_wait_for_input() {
        let codex = session("codex");
        let plan = plan_with_cli(&codex, false, Path::new("/installed/codex")).unwrap();
        assert!(plan.script.contains(
            "exec '/installed/codex' 'resume' '01234567-89ab-cdef-0123-456789abcdef' '-C'"
        ));
        assert!(
            plan.script
                .contains("cd -- '/workspaces/a '\"'\"'quoted'\"'\"' $value; $(literal)\nfolder'")
        );
        assert!(!plan.script.contains("--dangerously"));
        assert!(!plan.script.contains("--yolo"));
        let fresh = plan_with_cli(&codex, true, Path::new("/installed/codex")).unwrap();
        assert!(fresh.title.starts_with("New "));
        assert!(!fresh.script.contains(&codex.id));
        let deepseek = session("deepseek");
        assert!(plan_with_cli(&deepseek, false, Path::new(DSH_CLI)).is_err());
        let plan = plan_with_cli(&deepseek, true, Path::new(DSH_CLI)).unwrap();
        assert!(plan.script.contains("'--profile' 'headless' '-'"));
        assert!(!plan.script.contains("--session-id"));
        assert!(plan.script.contains("Control-D"));
        let opencode = session("opencode");
        let plan = plan_with_cli(&opencode, false, Path::new("/installed/opencode")).unwrap();
        assert!(plan.script.contains("'--session' 'ses_fixture_123'"));
        assert!(!plan.script.contains("'--prompt'"));
        assert!(!plan.script.contains("'--auto'"));
    }

    #[test]
    fn stop_mapping_requires_exact_native_provider_session_and_unique_owner() {
        let codex = session("codex");
        let row = ProcessInfo {
            pid: 12345,
            command: format!("codex resume {}", codex.id),
            identity_token: "retained-a".into(),
            ..ProcessInfo::default()
        };
        let native = ProcessCommand {
            executable: "/installed/codex".into(),
            argv: vec!["codex".into(), "resume".into(), codex.id.clone()],
        };
        let verified = native.executable.clone();
        let mapped =
            session_process_with(&codex, &[row.clone()], &verified, |_| Some(native.clone()))
                .unwrap();
        assert_eq!(mapped.identity_token, row.identity_token);
        let mut wrong = native.clone();
        wrong.argv[2].push_str("-other");
        assert!(
            session_process_with(&codex, &[row.clone()], &verified, |_| Some(wrong.clone()))
                .is_none()
        );
        wrong = native.clone();
        wrong.executable = "/installed/echo".into();
        assert!(
            session_process_with(&codex, &[row.clone()], &verified, |_| Some(wrong.clone()))
                .is_none()
        );
        wrong = native.clone();
        wrong.executable = "/unrelated/codex".into();
        assert!(
            session_process_with(&codex, &[row.clone()], &verified, |_| Some(wrong.clone()))
                .is_none()
        );
        wrong = native.clone();
        wrong.argv[1] = "fork".into();
        assert!(
            session_process_with(&codex, &[row.clone()], &verified, |_| Some(wrong.clone()))
                .is_none()
        );
        wrong = native.clone();
        wrong.argv = vec![
            "codex".into(),
            "--model".into(),
            "resume".into(),
            codex.id.clone(),
        ];
        assert!(
            session_process_with(&codex, &[row.clone()], &verified, |_| Some(wrong.clone()))
                .is_none()
        );
        let mut duplicate = row.clone();
        duplicate.pid += 1;
        assert!(
            session_process_with(&codex, &[row.clone(), duplicate], &verified, |_| Some(
                native.clone()
            ))
            .is_none()
        );
        let mut uncaptured = row.clone();
        uncaptured.identity_token.clear();
        assert!(
            session_process_with(&codex, &[uncaptured], &verified, |_| Some(native.clone()))
                .is_none()
        );
        let deepseek = session("deepseek");
        assert!(
            session_process_with(&deepseek, &[row], &verified, |_| panic!(
                "Shared desktop host must not be inspected for a stop"
            ))
            .is_none()
        );
    }

    #[test]
    fn stop_mapping_does_not_treat_prompt_contents_as_session_arguments() {
        let opencode = session("opencode");
        let genuine = ProcessCommand {
            executable: "/installed/opencode".into(),
            argv: vec![
                "opencode".into(),
                opencode.project.clone(),
                "--session".into(),
                opencode.id.clone(),
            ],
        };
        assert!(command_addresses_session(&opencode, &genuine));
        let prompted = ProcessCommand {
            argv: vec![
                "opencode".into(),
                "--prompt".into(),
                format!("--session {}", opencode.id),
            ],
            ..genuine.clone()
        };
        assert!(!command_addresses_session(&opencode, &prompted));
        let option_value = ProcessCommand {
            argv: vec![
                "opencode".into(),
                "--prompt".into(),
                "--session".into(),
                opencode.id.clone(),
            ],
            ..genuine.clone()
        };
        assert!(!command_addresses_session(&opencode, &option_value));
        let forked = ProcessCommand {
            argv: vec![
                "opencode".into(),
                "--session".into(),
                opencode.id.clone(),
                "--fork".into(),
            ],
            ..genuine.clone()
        };
        assert!(!command_addresses_session(&opencode, &forked));
        let shared = ProcessCommand {
            argv: vec![
                "opencode".into(),
                "serve".into(),
                "--session".into(),
                opencode.id.clone(),
            ],
            ..genuine
        };
        assert!(!command_addresses_session(&opencode, &shared));
    }

    #[test]
    fn invalid_or_unsupported_session_handoffs_fail_before_execution() {
        let mut recording = session("codex");
        recording.id = "--help".into();
        assert!(plan_with_cli(&recording, false, Path::new("/installed/codex")).is_err());
        recording = session("codex");
        recording.project = String::new();
        assert!(plan_with_cli(&recording, false, Path::new("/installed/codex")).is_err());
        recording = session("deepseek");
        recording.agent = "standard".into();
        assert!(
            plan_with_cli(&recording, false, Path::new(DSH_CLI))
                .unwrap_err()
                .contains("presets")
        );
        recording.agent.clear();
        assert!(plan_with_cli(&recording, false, Path::new(DSH_CLI)).is_err());
        recording.parent_id = "session-parent".into();
        assert!(plan_with_cli(&recording, false, Path::new(DSH_CLI)).is_err());
        assert!(plan_with_cli(&recording, true, Path::new(DSH_CLI)).is_ok());
        assert!(quote("bad\0argument").is_err());
    }

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "agentwatcher-handoff-test-{}-{stamp}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn command_files_are_private_unique_and_preserve_existing_files() {
        let scratch = Scratch::new();
        let directory = scratch.0.join("plans");
        let plan = plan_with_cli(&session("codex"), false, Path::new("/installed/codex")).unwrap();
        let first = write_plan_in(&plan, &directory).unwrap();
        let first_bytes = fs::read(&first).unwrap();
        let second = write_plan_in(&plan, &directory).unwrap();
        assert_ne!(first, second);
        assert_eq!(fs::read(&first).unwrap(), first_bytes);
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&first).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(fs::read_to_string(&first).unwrap(), plan.script);
    }

    #[test]
    fn directory_lock_releases_with_a_retained_duplicate_description() {
        let scratch = Scratch::new();
        let open = || {
            OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&scratch.0)
                .unwrap()
        };
        // A duplicated descriptor models what fork retains before exec,
        // without launching a process or depending on a scheduling race.
        let original = open();
        assert_eq!(
            unsafe { libc::flock(original.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        let retained = original.try_clone().unwrap();
        drop(original);
        let contender = open();
        assert!(DirectoryLock::acquire(&contender).is_err());
        assert_eq!(
            unsafe { libc::flock(retained.as_raw_fd(), libc::LOCK_UN) },
            0
        );

        let lock = DirectoryLock::acquire(&contender).unwrap();
        let inherited = contender.try_clone().unwrap();
        let next = open();
        assert!(DirectoryLock::acquire(&next).is_err());
        drop(lock);
        let _next_lock = DirectoryLock::acquire(&next).unwrap();
        assert!(inherited.metadata().unwrap().is_dir());
    }

    #[test]
    fn command_files_create_missing_private_app_parents_without_changing_existing_parent() {
        let scratch = Scratch::new();
        let library = scratch.0.join("Library");
        fs::create_dir(&library).unwrap();
        fs::set_permissions(&library, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(library.join("existing.txt"), b"preserved").unwrap();
        let support = library.join("Application Support");
        let app = support.join("AgentWatcher");
        let directory = app.join("terminal-plans");
        assert!(!support.exists());
        let plan = plan_with_cli(&session("codex"), false, Path::new("/installed/codex")).unwrap();
        let path = write_plan_in(&plan, &directory).unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), plan.script);
        for created in [&support, &app, &directory] {
            assert_eq!(
                fs::metadata(created).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        assert_eq!(
            fs::metadata(&library).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            fs::read(library.join("existing.txt")).unwrap(),
            b"preserved"
        );
    }

    #[test]
    fn command_file_bounds_and_symlink_directories_are_rejected() {
        use std::os::unix::fs::symlink;
        let scratch = Scratch::new();
        let plan = plan_with_cli(&session("codex"), false, Path::new("/installed/codex")).unwrap();
        let full = scratch.0.join("full");
        fs::DirBuilder::new().mode(0o700).create(&full).unwrap();
        File::create(full.join("existing.command"))
            .unwrap()
            .set_len(MAX_DIRECTORY_BYTES)
            .unwrap();
        assert!(write_plan_in(&plan, &full).is_err());
        let many = scratch.0.join("many");
        fs::DirBuilder::new().mode(0o700).create(&many).unwrap();
        for i in 0..MAX_PLAN_FILES {
            File::create(many.join(format!("{i}.command"))).unwrap();
        }
        assert!(write_plan_in(&plan, &many).is_err());
        let linked = scratch.0.join("linked");
        symlink(&full, &linked).unwrap();
        assert!(write_plan_in(&plan, &linked).is_err());
        let public = scratch.0.join("public");
        fs::create_dir(&public).unwrap();
        fs::set_permissions(&public, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(write_plan_in(&plan, &public).is_err());
        let oversized = TerminalPlan {
            script: format!("#!/bin/sh\n{}", "x".repeat(MAX_PLAN_BYTES)),
            title: "test".into(),
        };
        assert!(write_plan_in(&oversized, &scratch.0.join("new")).is_err());
    }
}
