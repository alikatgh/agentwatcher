//! Bounded local files and an explicitly started, owned plain-text Console.
//! Filesystem collection and Console start belong on a worker, not GPUI render.
use std::collections::VecDeque;
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

const DIRECTORY_ENTRIES: usize = 256;
const DIRECTORY_SCAN: usize = 1024;
const OUTPUT_BYTES: usize = 256 * 1024;
const OUTPUT_WORK_BYTES: usize = 8 * 1024 * 1024;
const WRITE_BYTES: usize = 8 * 1024;
const PENDING_BYTES: usize = 64 * 1024;
const SESSION_LIMIT: Duration = Duration::from_secs(30 * 60);
const STOP_GRACE: Duration = Duration::from_millis(500);
static ACTIVE_CONSOLE: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    pub relative: String,
    pub name: String,
    pub is_dir: bool,
    pub symlink: bool,
}

/// Read one directory level, capped at 256 visible entries / 1,024 inspected
/// entries. Hidden path components (including .git) require show_hidden.
/// Symlinks remain visible but are never traversed as folders.
pub fn list_directory(
    root: &Path,
    relative: &str,
    show_hidden: bool,
) -> Result<Vec<FileEntry>, String> {
    if relative.len() > 4096 {
        return Err("The relative folder path is too long.".into());
    }
    let root = root
        .canonicalize()
        .map_err(|_| "The project folder is unavailable.")?;
    if !root.is_dir() {
        return Err("The project root must be a folder.".into());
    }
    let mut clean = PathBuf::new();
    for (depth, component) in Path::new(relative).components().enumerate() {
        if depth >= 64 {
            return Err("The folder path exceeds the supported depth.".into());
        }
        match component {
            Component::CurDir => {}
            Component::Normal(name) => {
                if !show_hidden && name.to_string_lossy().starts_with('.') {
                    return Err("Hidden folders are disabled.".into());
                }
                clean.push(name);
            }
            _ => return Err("Choose a folder inside this project.".into()),
        }
    }
    // Open each component relative to an owned directory descriptor. A folder
    // replaced by a symlink between checks cannot redirect collection outside
    // the root. Refusing internal directory symlinks also avoids cycle walks.
    let mut folder = open_directory_at(libc::AT_FDCWD, &root)?;
    for component in clean.components() {
        folder = open_directory_at(folder.as_raw_fd(), Path::new(component.as_os_str()))?;
    }
    let descriptor = folder.into_raw_fd();
    let directory = unsafe { libc::fdopendir(descriptor) };
    if directory.is_null() {
        unsafe {
            libc::close(descriptor);
        };
        return Err("The folder could not be read.".into());
    }
    struct Directory(*mut libc::DIR);
    impl Drop for Directory {
        fn drop(&mut self) {
            unsafe {
                libc::closedir(self.0);
            }
        }
    }
    let directory = Directory(directory);
    let mut result = Vec::with_capacity(DIRECTORY_ENTRIES);
    for _ in 0..DIRECTORY_SCAN {
        let entry = unsafe { libc::readdir(directory.0) };
        if entry.is_null() {
            break;
        }
        let raw_name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        let Ok(name) = raw_name.to_str() else {
            continue;
        };
        if name == "." || name == ".." {
            continue;
        }
        if !show_hidden && name.starts_with('.') {
            continue;
        }
        let mut metadata: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe {
            libc::fstatat(
                descriptor,
                raw_name.as_ptr(),
                &mut metadata,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } == -1
        {
            return Err("A folder entry could not be inspected.".into());
        }
        let kind = metadata.st_mode & libc::S_IFMT;
        let symlink = kind == libc::S_IFLNK;
        let is_dir = kind == libc::S_IFDIR;
        result.push(FileEntry {
            relative: clean
                .join(name)
                .to_str()
                .ok_or("The folder path is not supported.")?
                .to_owned(),
            name: name.to_owned(),
            is_dir,
            symlink,
        });
        if result.len() == DIRECTORY_ENTRIES {
            break;
        }
    }
    result.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
    Ok(result)
}

fn open_directory_at(parent: libc::c_int, path: &Path) -> Result<File, String> {
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| "The folder path is not supported.")?;
    let descriptor = unsafe {
        libc::openat(
            parent,
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if descriptor == -1 {
        return Err("The folder is unavailable or is a symbolic link.".into());
    }
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerminalStatus {
    #[default]
    Idle,
    Starting,
    Running,
    Stopped,
    Exited,
    TimedOut,
    Error,
}
impl TerminalStatus {
    pub fn is_active(self) -> bool {
        matches!(self, Self::Starting | Self::Running)
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Not started",
            Self::Starting => "Starting",
            Self::Running => "Running",
            Self::Stopped => "Stopped",
            Self::Exited => "Exited",
            Self::TimedOut => "Time limit reached",
            Self::Error => "Error",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct TerminalSnapshot {
    pub revision: u64,
    pub text: String,
    pub status: TerminalStatus,
    pub error: Option<String>,
    /// Older scrollback was discarded to retain at most 256 KiB.
    pub truncated: bool,
}

struct State {
    snapshot: TerminalSnapshot,
    active: bool,
    stop_requested: bool,
    input: VecDeque<Vec<u8>>,
    pending_bytes: usize,
    dimensions: (u16, u16),
    resize: Option<(u16, u16)>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            snapshot: TerminalSnapshot::default(),
            active: false,
            stop_requested: false,
            input: VecDeque::new(),
            pending_bytes: 0,
            dimensions: (80, 24),
            resize: None,
        }
    }
}
#[derive(Default)]
struct Registry {
    /// Held across spawn/publication and cleared before the leader is reaped.
    pid: Mutex<Option<u32>>,
}
#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    registry: Arc<Registry>,
    closed: AtomicBool,
}
impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }
    fn cancelled(&self) -> bool {
        self.closed.load(Ordering::Acquire) || self.lock().stop_requested
    }
}
struct Owner {
    shared: Arc<Shared>,
}
impl Drop for Owner {
    fn drop(&mut self) {
        force_shutdown(&self.shared);
    }
}

/// Cloneable UI handles share one owned session; cloning never starts a shell.
#[derive(Clone)]
pub struct Terminal {
    owner: Arc<Owner>,
}
#[derive(Clone)]
pub struct TerminalShutdownHandle {
    shared: Arc<Shared>,
}
impl TerminalShutdownHandle {
    /// OS-quit backstop. Can signal only the process group this Console spawned.
    pub fn force_shutdown(&self) {
        force_shutdown(&self.shared);
    }
}
impl Default for Terminal {
    fn default() -> Self {
        Self::new()
    }
}

struct Admission;
impl Admission {
    fn acquire() -> Result<Self, String> {
        ACTIVE_CONSOLE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| "An AgentWatcher Console is already active.".into())
    }
}
impl Drop for Admission {
    fn drop(&mut self) {
        ACTIVE_CONSOLE.store(false, Ordering::Release);
    }
}

enum Program {
    Shell,
    #[cfg(test)]
    Cat,
}
impl Terminal {
    pub fn new() -> Self {
        Self {
            owner: Arc::new(Owner {
                shared: Arc::new(Shared::default()),
            }),
        }
    }
    /// Explicit admission only; cwd/spawn failures appear in the worker snapshot.
    /// No user startup files, inherited credentials, plugins or model work are
    /// loaded by this library. Commands typed by the user have full shell power.
    pub fn start(&self, cwd: &Path) -> Result<(), String> {
        self.start_program(cwd, Program::Shell, SESSION_LIMIT)
    }
    fn start_program(&self, cwd: &Path, program: Program, limit: Duration) -> Result<(), String> {
        let shared = &self.owner.shared;
        let mut state = shared.lock();
        if shared.closed.load(Ordering::Acquire) {
            return Err("This Console has shut down.".into());
        }
        if state.active {
            return Err("This Console is already active.".into());
        }
        let admission = Admission::acquire()?;
        let revision = state.snapshot.revision.saturating_add(1);
        state.snapshot = TerminalSnapshot {
            revision,
            status: TerminalStatus::Starting,
            ..TerminalSnapshot::default()
        };
        state.active = true;
        state.stop_requested = false;
        state.input.clear();
        state.pending_bytes = 0;
        state.resize = Some(state.dimensions);
        let cwd = cwd.to_owned();
        let worker = Arc::clone(shared);
        if let Err(error) = std::thread::Builder::new()
            .name("agentwatcher-console".into())
            .spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_console(&worker, &cwd, program, limit)
                }))
                .unwrap_or_else(|_| {
                    (
                        TerminalStatus::Error,
                        Some("The Console worker stopped unexpectedly.".into()),
                    )
                });
                let mut state = worker.lock();
                state.snapshot.status = outcome.0;
                state.snapshot.error = outcome.1;
                state.snapshot.revision = state.snapshot.revision.saturating_add(1);
                state.input.clear();
                state.pending_bytes = 0;
                drop(admission);
                state.active = false;
            })
        {
            state.active = false;
            state.snapshot.status = TerminalStatus::Error;
            state.snapshot.error = Some(format!("Could not start the Console worker: {error}"));
            state.snapshot.revision = state.snapshot.revision.saturating_add(1);
            return Err("The Console worker could not start.".into());
        }
        Ok(())
    }
    /// Nonblocking, bounded submission. Newline executes the user's command;
    /// byte 0x03 uses PTY signal handling to interrupt the owned foreground group.
    pub fn write(&self, input: &[u8]) -> Result<(), String> {
        if input.len() > WRITE_BYTES {
            return Err("Console input is limited to 8 KiB per submission.".into());
        }
        let shared = &self.owner.shared;
        let mut state = shared.lock();
        if shared.closed.load(Ordering::Acquire) || !state.active || state.stop_requested {
            return Err("The Console is not accepting input.".into());
        }
        if state.pending_bytes.saturating_add(input.len()) > PENDING_BYTES {
            return Err("The Console input queue is full; wait for it to drain.".into());
        }
        if !input.is_empty() {
            state.input.push_back(input.to_vec());
            state.pending_bytes += input.len();
        }
        Ok(())
    }
    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        if !(2..=500).contains(&cols) || !(2..=200).contains(&rows) {
            return Err("Console dimensions must be 2–500 columns and 2–200 rows.".into());
        }
        if self.owner.shared.closed.load(Ordering::Acquire) {
            return Err("This Console has shut down.".into());
        }
        let mut state = self.owner.shared.lock();
        if state.dimensions == (cols, rows) {
            return Ok(());
        }
        state.dimensions = (cols, rows);
        state.resize = Some((cols, rows));
        Ok(())
    }
    pub fn poll_snapshot(&self) -> TerminalSnapshot {
        self.owner.shared.lock().snapshot.clone()
    }
    pub fn stop(&self) -> Result<(), String> {
        let mut state = self.owner.shared.lock();
        if !state.active {
            return Err("The Console is not running.".into());
        }
        state.stop_requested = true;
        Ok(())
    }
    /// Nonblocking orderly close; poll is_idle before the application's exit.
    pub fn shutdown(&self) {
        self.owner.shared.closed.store(true, Ordering::Release);
        self.owner.shared.lock().stop_requested = true;
    }
    pub fn is_idle(&self) -> bool {
        !self.owner.shared.lock().active
    }
    pub fn shutdown_handle(&self) -> TerminalShutdownHandle {
        TerminalShutdownHandle {
            shared: Arc::clone(&self.owner.shared),
        }
    }
}

fn force_shutdown(shared: &Shared) {
    shared.closed.store(true, Ordering::Release);
    shared.lock().stop_requested = true;
    // This lock spans child creation/publication too: no unregistered spawn can
    // escape an OS-quit callback, and a reaped/reused PID is never signalled.
    let owner = shared
        .registry
        .pid
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(pid) = *owner {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
}

struct OwnedPty {
    child: Child,
    master: File,
    registry: Arc<Registry>,
    reaped: bool,
}
impl OwnedPty {
    fn exited(&self) -> Result<bool, String> {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
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
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            Ok(false)
        } else {
            Err(format!("Could not observe the Console process: {error}"))
        }
    }
    fn signal(&self, signal: i32) {
        unsafe {
            libc::kill(-(self.child.id() as i32), signal);
        }
    }
    fn reap(&mut self) -> Result<ExitStatus, String> {
        // The zombie leader still reserves its PID. Kill remaining ordinary
        // children, clear the registry, then reap, in that order.
        self.signal(libc::SIGKILL);
        let mut owner = self
            .registry
            .pid
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if *owner == Some(self.child.id()) {
            *owner = None;
        }
        drop(owner);
        // A failed wait must not let Drop signal a PID after ownership release.
        self.reaped = true;
        let status = self
            .child
            .wait()
            .map_err(|error| format!("Could not reap the Console process: {error}"))?;
        Ok(status)
    }
}
impl Drop for OwnedPty {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.reap();
        }
    }
}

fn spawn_pty(shared: &Shared, cwd: &Path, program: Program) -> Result<OwnedPty, String> {
    let cwd = cwd
        .canonicalize()
        .map_err(|_| "The Console workspace is unavailable.")?;
    if !cwd.is_dir() {
        return Err("The Console workspace must be a folder.".into());
    }
    let (cols, rows) = shared.lock().dimensions;
    let mut dimensions = libc::winsize {
        ws_col: cols,
        ws_row: rows,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let (mut master, mut slave) = (-1, -1);
    if unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut dimensions,
        )
    } == -1
    {
        return Err(format!(
            "Could not open the Console PTY: {}",
            io::Error::last_os_error()
        ));
    }
    let master = unsafe { File::from_raw_fd(master) };
    let slave = unsafe { File::from_raw_fd(slave) };
    let mut termios: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(slave.as_raw_fd(), &mut termios) } == -1 {
        return Err("Could not inspect Console PTY settings.".into());
    }
    termios.c_lflag |= libc::ISIG | libc::ICANON | libc::ECHO;
    termios.c_iflag |= libc::ICRNL;
    termios.c_oflag |= libc::OPOST | libc::ONLCR;
    termios.c_cc[libc::VINTR] = 3;
    termios.c_cc[libc::VEOF] = 4;
    if unsafe { libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &termios) } == -1 {
        return Err("Could not configure the Console PTY.".into());
    }
    set_descriptor_flags(&master, true)?;
    set_descriptor_flags(&slave, false)?;
    let stdout = slave
        .try_clone()
        .map_err(|_| "Could not duplicate Console stdout.")?;
    let stderr = slave
        .try_clone()
        .map_err(|_| "Could not duplicate Console stderr.")?;
    let mut command = match program {
        Program::Shell => {
            let mut command = Command::new("/bin/zsh");
            command.args(["-f", "+m"]);
            command
        }
        #[cfg(test)]
        Program::Cat => Command::new("/bin/cat"),
    };
    command.env_clear();
    if let Some(home) = std::env::var_os("HOME") {
        command.env("HOME", home);
    }
    command
        .env("PATH", "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin")
        .env("LANG", "en_US.UTF-8")
        .env("TERM", "dumb")
        .env("PROMPT", "> ")
        .env("RPROMPT", "")
        .current_dir(cwd)
        .stdin(Stdio::from(slave))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    unsafe {
        command.pre_exec(|| {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::signal(libc::SIGTERM, libc::SIG_DFL);
            libc::signal(libc::SIGHUP, libc::SIG_DFL);
            libc::signal(libc::SIGQUIT, libc::SIG_DFL);
            if libc::setsid() == -1
                || libc::ioctl(0, libc::TIOCSCTTY as libc::c_ulong, 0) == -1
                || libc::tcsetpgrp(0, libc::getpgrp()) == -1
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut owner = shared
        .registry
        .pid
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if shared.closed.load(Ordering::Acquire) || shared.cancelled() {
        return Err("The Console start was cancelled.".into());
    }
    if owner.is_some() {
        return Err("The owned Console process slot is not free.".into());
    }
    let child = command
        .spawn()
        .map_err(|error| format!("Could not start the local Console: {error}"))?;
    let process = OwnedPty {
        child,
        master,
        registry: Arc::clone(&shared.registry),
        reaped: false,
    };
    *owner = Some(process.child.id());
    if shared.closed.load(Ordering::Acquire) {
        process.signal(libc::SIGKILL);
    }
    Ok(process)
}

fn set_descriptor_flags(file: &File, nonblocking: bool) -> Result<(), String> {
    let fd = file.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } == -1 {
        return Err("Could not protect the Console PTY descriptor.".into());
    }
    if nonblocking {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1
        {
            return Err("Could not configure nonblocking Console I/O.".into());
        }
    }
    Ok(())
}

fn run_console(
    shared: &Shared,
    cwd: &Path,
    program: Program,
    limit: Duration,
) -> (TerminalStatus, Option<String>) {
    if shared.cancelled() {
        return (TerminalStatus::Stopped, None);
    }
    let mut process = match spawn_pty(shared, cwd, program) {
        Ok(process) => process,
        Err(_) if shared.cancelled() => return (TerminalStatus::Stopped, None),
        Err(error) => return (TerminalStatus::Error, Some(error)),
    };
    {
        let mut state = shared.lock();
        state.snapshot.status = TerminalStatus::Running;
        state.snapshot.revision = state.snapshot.revision.saturating_add(1);
    }
    let deadline = Instant::now() + limit;
    let mut stop_at = None;
    let mut final_status = TerminalStatus::Exited;
    let mut error = None;
    let mut sanitizer = PlainOutput::default();
    let mut bytes = 0usize;
    let mut pending = VecDeque::<u8>::new();
    let mut closed = false;
    loop {
        let now = Instant::now();
        if stop_at.is_none() && (shared.cancelled() || now >= deadline) {
            final_status = if shared.cancelled() {
                TerminalStatus::Stopped
            } else {
                TerminalStatus::TimedOut
            };
            process.signal(libc::SIGTERM);
            stop_at = Some(now);
        }
        let exited = match process.exited() {
            Ok(exited) => exited,
            Err(message) => {
                error = Some(message);
                final_status = TerminalStatus::Error;
                true
            }
        };
        if stop_at.is_none() {
            let resize = {
                let mut state = shared.lock();
                while let Some(input) = state.input.pop_front() {
                    pending.extend(input);
                }
                state.resize.take()
            };
            if let Some((cols, rows)) = resize {
                let dimensions = libc::winsize {
                    ws_col: cols,
                    ws_row: rows,
                    ws_xpixel: 0,
                    ws_ypixel: 0,
                };
                if unsafe { libc::ioctl(process.master.as_raw_fd(), libc::TIOCSWINSZ, &dimensions) }
                    == -1
                {
                    error = Some("Could not resize the Console PTY.".into());
                }
            }
            if !pending.is_empty() {
                let input = pending.make_contiguous();
                match process.master.write(&input[..input.len().min(WRITE_BYTES)]) {
                    Ok(count) => {
                        pending.drain(..count);
                        let mut state = shared.lock();
                        state.pending_bytes = state.pending_bytes.saturating_sub(count);
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                        ) => {}
                    Err(message) => {
                        error = Some(format!("Could not write to the Console PTY: {message}"))
                    }
                }
            }
        }
        for _ in 0..if error.is_none() { 4 } else { 0 } {
            let mut buffer = [0u8; 4096];
            match process.master.read(&mut buffer) {
                Ok(0) => {
                    closed = true;
                    break;
                }
                Ok(count) => {
                    bytes = bytes.saturating_add(count);
                    if bytes > OUTPUT_WORK_BYTES {
                        error=Some("Console output exceeded the 8 MiB work limit; its owned process group was stopped.".into());
                        break;
                    }
                    let text = sanitizer.feed(&buffer[..count]);
                    if !text.is_empty() {
                        append_output(&mut shared.lock().snapshot, &text);
                    }
                }
                Err(message) if message.kind() == io::ErrorKind::Interrupted => {}
                Err(message) if message.kind() == io::ErrorKind::WouldBlock => break,
                // PTY masters report EIO when their slave closes on Unix.
                Err(message) if message.raw_os_error() == Some(libc::EIO) => {
                    closed = true;
                    break;
                }
                Err(message) => {
                    error = Some(format!("Could not read Console output: {message}"));
                    break;
                }
            }
        }
        if error.is_some() && stop_at.is_none() {
            final_status = TerminalStatus::Error;
            process.signal(libc::SIGTERM);
            stop_at = Some(now);
        }
        if closed && stop_at.is_none() {
            process.signal(libc::SIGTERM);
            stop_at = Some(now);
        }
        if exited
            && (closed
                || error.is_some()
                || stop_at.is_some_and(|at| now.duration_since(at) >= STOP_GRACE))
        {
            let status = process.reap();
            let tail = sanitizer.finish();
            if !tail.is_empty() {
                append_output(&mut shared.lock().snapshot, &tail);
            }
            if error.is_none() {
                if let Err(message) = status {
                    error = Some(message);
                    final_status = TerminalStatus::Error;
                }
            }
            return (final_status, error);
        }
        if exited && stop_at.is_none() {
            // Keep the zombie leader unreaped while cleaning its group and
            // draining queued PTY output. Do not discard a completed tail.
            process.signal(libc::SIGKILL);
            stop_at = Some(now);
        }
        if stop_at.is_some_and(|at| now.duration_since(at) >= STOP_GRACE) {
            process.signal(libc::SIGKILL);
        }
        let mut descriptor = libc::pollfd {
            fd: if closed || error.is_some() {
                -1
            } else {
                process.master.as_raw_fd()
            },
            events: libc::POLLIN
                | libc::POLLHUP
                | if pending.is_empty() || stop_at.is_some() {
                    0
                } else {
                    libc::POLLOUT
                },
            revents: 0,
        };
        unsafe {
            libc::poll(&mut descriptor, 1, 40);
        }
    }
}

fn append_output(snapshot: &mut TerminalSnapshot, text: &str) {
    snapshot.text.push_str(text);
    if snapshot.text.len() > OUTPUT_BYTES {
        let mut remove = snapshot.text.len() - OUTPUT_BYTES;
        while !snapshot.text.is_char_boundary(remove) {
            remove += 1;
        }
        snapshot.text.drain(..remove);
        snapshot.truncated = true;
    }
    snapshot.revision = snapshot.revision.saturating_add(1);
}

#[derive(Clone, Copy, Default)]
enum EscapeState {
    #[default]
    Ground,
    Escape,
    Intermediate,
    Csi,
    Osc,
    OscEscape,
    String,
    StringEscape,
}
#[derive(Default)]
struct PlainOutput {
    state: EscapeState,
    utf8: Vec<u8>,
    carriage_return: bool,
}
impl PlainOutput {
    fn feed(&mut self, bytes: &[u8]) -> String {
        self.utf8.extend_from_slice(bytes);
        let mut decoded = String::new();
        let mut offset = 0;
        loop {
            match std::str::from_utf8(&self.utf8[offset..]) {
                Ok(text) => {
                    decoded.push_str(text);
                    offset = self.utf8.len();
                    break;
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    decoded.push_str(
                        std::str::from_utf8(&self.utf8[offset..offset + valid])
                            .expect("validated UTF-8 prefix"),
                    );
                    let Some(invalid) = error.error_len() else {
                        offset += valid;
                        break;
                    };
                    // Standalone 8-bit C1 controls are recognized too; decoding
                    // first prevents a UTF-8 continuation byte becoming CSI.
                    let byte = self.utf8[offset + valid];
                    decoded.push(if invalid == 1 && (0x80..=0x9f).contains(&byte) {
                        char::from(byte)
                    } else {
                        '\u{fffd}'
                    });
                    offset += valid + invalid;
                }
            }
        }
        self.utf8.drain(..offset);
        let mut result = String::new();
        for ch in decoded.chars() {
            self.state = match self.state {
                EscapeState::Ground => {
                    match ch {
                        '\u{1b}' => {
                            self.state = EscapeState::Escape;
                            continue;
                        }
                        '\u{9b}' => {
                            self.state = EscapeState::Csi;
                            continue;
                        }
                        '\u{9d}' => {
                            self.state = EscapeState::Osc;
                            continue;
                        }
                        '\u{90}' | '\u{98}' | '\u{9e}' | '\u{9f}' => {
                            self.state = EscapeState::String;
                            continue;
                        }
                        '\r' => {
                            result.push('\n');
                            self.carriage_return = true;
                            continue;
                        }
                        '\n' if self.carriage_return => {
                            self.carriage_return = false;
                            continue;
                        }
                        '\n' | '\t' => result.push(ch),
                        other if other.is_control() => continue,
                        other => result.push(other),
                    }
                    self.carriage_return = false;
                    EscapeState::Ground
                }
                EscapeState::Escape => match ch {
                    '[' => EscapeState::Csi,
                    ']' => EscapeState::Osc,
                    'P' | 'X' | '^' | '_' => EscapeState::String,
                    '\u{20}'..='\u{2f}' => EscapeState::Intermediate,
                    '\u{1b}' => EscapeState::Escape,
                    _ => EscapeState::Ground,
                },
                EscapeState::Intermediate => {
                    if ('\u{30}'..='\u{7e}').contains(&ch) {
                        EscapeState::Ground
                    } else {
                        EscapeState::Intermediate
                    }
                }
                EscapeState::Csi => {
                    if ch == '\u{1b}' {
                        EscapeState::Escape
                    } else if ('\u{40}'..='\u{7e}').contains(&ch) {
                        EscapeState::Ground
                    } else {
                        EscapeState::Csi
                    }
                }
                EscapeState::Osc => match ch {
                    '\u{7}' | '\u{9c}' => EscapeState::Ground,
                    '\u{1b}' => EscapeState::OscEscape,
                    _ => EscapeState::Osc,
                },
                EscapeState::OscEscape => match ch {
                    '\\' | '\u{7}' | '\u{9c}' => EscapeState::Ground,
                    '\u{1b}' => EscapeState::OscEscape,
                    _ => EscapeState::Osc,
                },
                EscapeState::String => {
                    if ch == '\u{9c}' {
                        EscapeState::Ground
                    } else if ch == '\u{1b}' {
                        EscapeState::StringEscape
                    } else {
                        EscapeState::String
                    }
                }
                EscapeState::StringEscape => {
                    if ch == '\\' || ch == '\u{9c}' {
                        EscapeState::Ground
                    } else if ch == '\u{1b}' {
                        EscapeState::StringEscape
                    } else {
                        EscapeState::String
                    }
                }
            };
        }
        result
    }
    fn finish(&mut self) -> String {
        if self.utf8.is_empty() {
            String::new()
        } else {
            self.utf8.clear();
            self.feed("\u{fffd}".as_bytes())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::AtomicU64;
    static NEXT_FILE: AtomicU64 = AtomicU64::new(0);
    static CONSOLES: Mutex<()> = Mutex::new(());
    struct Files(PathBuf);
    impl Files {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "agentwatcher-local-tools-{}-{}",
                std::process::id(),
                NEXT_FILE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).expect("new exclusively owned fixture directory");
            Self(path)
        }
    }
    impl Drop for Files {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn wait_until(mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !predicate() {
            assert!(Instant::now() < deadline, "Console condition timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn directory_listing_is_one_level_hidden_by_default_and_root_contained() {
        let files = Files::new();
        let root = files.0.join("project");
        let outside = files.0.join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::create_dir(root.join("sub")).unwrap();
        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join("sub/inside.rs"), "fixture").unwrap();
        fs::write(root.join("visible.rs"), "fixture").unwrap();
        fs::write(root.join(".hidden"), "fixture").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("external")).unwrap();
        std::os::unix::fs::symlink(root.join("sub"), root.join("alias")).unwrap();
        let entries = list_directory(&root, "", false).unwrap();
        assert!(entries[0].is_dir);
        assert!(entries.iter().all(|entry| !entry.name.starts_with('.')));
        assert!(
            !entries
                .iter()
                .any(|entry| entry.relative.ends_with("inside.rs"))
        );
        assert!(
            entries
                .iter()
                .filter(|entry| entry.symlink)
                .all(|entry| !entry.is_dir)
        );
        assert_eq!(
            list_directory(&root, "sub", false).unwrap()[0].relative,
            "sub/inside.rs"
        );
        for relative in ["../outside", "/tmp", "external", "alias", ".git"] {
            assert!(
                list_directory(&root, relative, false).is_err(),
                "{relative}"
            );
        }
        assert!(list_directory(&root, "external", true).is_err());
        assert!(
            list_directory(&root, "", true)
                .unwrap()
                .iter()
                .any(|entry| entry.name == ".git")
        );
        assert!(outside.is_dir());
        assert!(root.join("sub/inside.rs").is_file());
    }

    #[test]
    fn directory_entry_and_path_work_are_bounded() {
        let files = Files::new();
        for index in 0..300 {
            fs::write(files.0.join(format!("file-{index:03}")), b"").unwrap();
        }
        let entries = list_directory(&files.0, "", false).unwrap();
        assert_eq!(entries.len(), DIRECTORY_ENTRIES);
        assert!(entries.windows(2).all(|pair| pair[0].name <= pair[1].name));
        assert!(list_directory(&files.0, &"a".repeat(4097), true).is_err());
        assert!(list_directory(&files.0, &vec!["a"; 65].join("/"), true).is_err());
    }

    #[test]
    fn console_output_strips_split_osc_csi_dcs_and_controls_without_actions() {
        let mut output = PlainOutput::default();
        assert_eq!(output.feed(b"before\x1b["), "before");
        assert_eq!(output.feed(b"31mred\x1b[0m\x1b]52;c;private"), "red");
        assert_eq!(output.feed(b"-payload\x07after\x1bPdiscard"), "after");
        assert_eq!(output.feed(b"\x1b\\end\x00\x7f"), "end");
        assert_eq!(output.feed(b"\x9d52;c;discard\x9cZ"), "Z");
        assert_eq!(output.feed("\u{9d}52;c;discard\u{9c}Q".as_bytes()), "Q");
    }

    #[test]
    fn console_output_preserves_split_utf8_and_normalizes_crlf_and_bare_cr() {
        let mut output = PlainOutput::default();
        let value = "Л日本😀";
        let mut result = String::new();
        for byte in value.as_bytes() {
            result.push_str(&output.feed(&[*byte]));
            assert!(output.utf8.len() <= 3);
        }
        assert_eq!(result, value);
        assert_eq!(output.feed(b"a\r"), "a\n");
        assert_eq!(output.feed(b"\nb\rc\n\t"), "b\nc\n\t");
        assert_eq!(output.feed(&[0xf0]), "");
        assert_eq!(output.finish(), "\u{fffd}");
        assert!(output.utf8.is_empty());
    }

    #[test]
    fn console_scrollback_retains_a_bounded_valid_utf8_tail() {
        let mut snapshot = TerminalSnapshot::default();
        append_output(&mut snapshot, &"日本😀\n".repeat(OUTPUT_BYTES / 4));
        assert!(snapshot.text.len() <= OUTPUT_BYTES && snapshot.truncated);
        assert!(snapshot.text.ends_with("日本😀\n"));
        assert_eq!(snapshot.revision, 1);
        append_output(&mut snapshot, "latest");
        assert!(snapshot.text.ends_with("latest"));
        assert_eq!(snapshot.revision, 2);
    }

    #[test]
    fn console_creation_is_inert_and_closed_handles_cannot_start_or_write() {
        let terminal = Terminal::new();
        assert!(terminal.is_idle());
        assert_eq!(terminal.poll_snapshot().status, TerminalStatus::Idle);
        assert!(terminal.write(b"not executed\n").is_err());
        assert!(terminal.write(&vec![0; WRITE_BYTES + 1]).is_err());
        assert!(terminal.resize(0, 24).is_err());
        assert!(terminal.resize(80, 201).is_err());
        assert!(terminal.resize(100, 30).is_ok());
        terminal.shutdown();
        assert!(terminal.start(Path::new("/not/executed")).is_err());
        assert!(terminal.resize(80, 24).is_err());
    }

    #[test]
    fn owned_cat_pty_supports_input_resize_global_admission_and_stop_restart() {
        let _serial = CONSOLES.lock().unwrap_or_else(|error| error.into_inner());
        let files = Files::new();
        let first = Terminal::new();
        first
            .start_program(&files.0, Program::Cat, Duration::from_secs(10))
            .unwrap();
        wait_until(|| first.poll_snapshot().status == TerminalStatus::Running);
        let second = Terminal::new();
        assert!(
            second
                .start_program(&files.0, Program::Cat, Duration::from_secs(10))
                .is_err()
        );
        first.resize(90, 28).unwrap();
        first.write(b"local-pty-sentinel\n").unwrap();
        wait_until(|| {
            first
                .poll_snapshot()
                .text
                .matches("local-pty-sentinel")
                .count()
                >= 2
        });
        assert!(first.write(&vec![b'x'; WRITE_BYTES + 1]).is_err());
        first.stop().unwrap();
        wait_until(|| first.is_idle());
        assert_eq!(first.poll_snapshot().status, TerminalStatus::Stopped);
        second
            .start_program(&files.0, Program::Cat, Duration::from_secs(10))
            .unwrap();
        wait_until(|| second.poll_snapshot().status == TerminalStatus::Running);
        let quit = second.shutdown_handle();
        quit.force_shutdown();
        wait_until(|| second.is_idle());
        quit.force_shutdown();
        assert!(second.owner.shared.registry.pid.lock().unwrap().is_none());
    }

    #[test]
    fn owned_cat_deadline_ctrl_c_and_last_handle_drop_reap_the_registered_group() {
        let _serial = CONSOLES.lock().unwrap_or_else(|error| error.into_inner());
        let files = Files::new();
        let terminal = Terminal::new();
        terminal
            .start_program(&files.0, Program::Cat, Duration::from_millis(60))
            .unwrap();
        wait_until(|| terminal.is_idle());
        assert_eq!(terminal.poll_snapshot().status, TerminalStatus::TimedOut);
        terminal
            .start_program(&files.0, Program::Cat, Duration::from_secs(10))
            .unwrap();
        wait_until(|| terminal.poll_snapshot().status == TerminalStatus::Running);
        terminal.write(&[3]).unwrap();
        wait_until(|| terminal.is_idle());
        assert_eq!(terminal.poll_snapshot().status, TerminalStatus::Exited);
        terminal
            .start_program(&files.0, Program::Cat, Duration::from_secs(10))
            .unwrap();
        wait_until(|| terminal.poll_snapshot().status == TerminalStatus::Running);
        let retained = terminal.shutdown_handle();
        let clone = terminal.clone();
        drop(terminal);
        assert!(retained.shared.lock().active);
        drop(clone);
        wait_until(|| !retained.shared.lock().active);
        assert!(retained.shared.registry.pid.lock().unwrap().is_none());
    }

    #[test]
    fn emergency_quit_waits_for_spawn_registration_and_closes_future_admission() {
        let terminal = Terminal::new();
        let handle = terminal.shutdown_handle();
        let shared = Arc::clone(&handle.shared);
        let owner = shared.registry.pid.lock().unwrap();
        let (done, received) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            handle.force_shutdown();
            done.send(()).unwrap();
        });
        wait_until(|| shared.closed.load(Ordering::Acquire));
        assert!(received.try_recv().is_err());
        drop(owner);
        received.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
        assert!(terminal.start(Path::new("/not/executed")).is_err());
    }
}
