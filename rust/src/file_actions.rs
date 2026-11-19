//! Explicit file-opening actions. Call discovery, loading and execution on a background worker.
//! A plan never interpolates a path into a shell or passes a file to a terminal.
use agentwatcher::backend;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{ErrorKind, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

const OPEN_TIMEOUT: Duration = Duration::from_secs(5);
const OPEN_OUTPUT_LIMIT: usize = 16 * 1024;
const APP_INFO_LIMIT: u64 = 128 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FileTarget {
    Finder,
    VSCode,
    Zed,
    Xcode,
    AndroidStudio,
    Ghostty,
    Terminal,
}

impl FileTarget {
    pub(super) const ALL: [Self; 7] = [
        Self::Finder,
        Self::VSCode,
        Self::Zed,
        Self::Xcode,
        Self::AndroidStudio,
        Self::Ghostty,
        Self::Terminal,
    ];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Finder => "Finder (default)",
            Self::VSCode => "VS Code",
            Self::Zed => "Zed",
            Self::Xcode => "Xcode",
            Self::AndroidStudio => "Android Studio",
            Self::Ghostty => "Ghostty",
            Self::Terminal => "Terminal",
        }
    }

    fn bundle_names(self) -> &'static [&'static str] {
        match self {
            Self::Finder => &["Finder.app"],
            Self::VSCode => &["Visual Studio Code.app"],
            Self::Zed => &["Zed.app"],
            Self::Xcode => &["Xcode.app", "Xcode-beta.app"],
            Self::AndroidStudio => &["Android Studio.app"],
            Self::Ghostty => &["Ghostty.app"],
            Self::Terminal => &["Terminal.app"],
        }
    }

    fn executable_names(self) -> &'static [&'static str] {
        match self {
            Self::Finder => &["Finder"],
            Self::VSCode => &["Code", "Electron"],
            Self::Zed => &["zed"],
            Self::Xcode => &["Xcode"],
            Self::AndroidStudio => &["studio"],
            Self::Ghostty => &["ghostty"],
            Self::Terminal => &["Terminal"],
        }
    }

    fn opens_containing_folder(self) -> bool {
        matches!(self, Self::Ghostty | Self::Terminal)
    }
}

fn application_roots() -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from("/Applications")];
    if let Some(user_home) = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        roots.push(user_home.join("Applications"));
    }
    roots
}

fn app_is_installed(target: FileTarget, path: &Path) -> bool {
    path.is_dir()
        && target.executable_names().iter().any(|name| {
            fs::metadata(path.join("Contents/MacOS").join(name)).is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
}

fn app_in_roots(target: FileTarget, roots: &[PathBuf]) -> Option<PathBuf> {
    roots
        .iter()
        .flat_map(|root| {
            target
                .bundle_names()
                .iter()
                .map(move |name| root.join(name))
        })
        .find(|path| app_is_installed(target, path))
}

fn document_types_accept_folders(info: &str) -> bool {
    let Some((_, after_key)) = info.split_once("<key>CFBundleDocumentTypes</key>") else {
        return false;
    };
    let Some(array) = after_key.trim_start().strip_prefix("<array>") else {
        return false;
    };
    let mut remaining = array;
    let mut depth = 1_usize;
    while let Some(close) = remaining.find("</array>") {
        if let Some(open) = remaining.find("<array>").filter(|open| *open < close) {
            depth += 1;
            remaining = &remaining[open + "<array>".len()..];
        } else {
            depth -= 1;
            if depth == 0 {
                let end = array.len() - remaining.len() + close;
                return array[..end].split("<string>").skip(1).any(|value| {
                    value
                        .split_once("</string>")
                        .is_some_and(|(value, _)| value.trim() == "public.directory")
                });
            }
            remaining = &remaining[close + "</array>".len()..];
        }
    }
    false
}

fn ghostty_accepts_folders(app: &Path) -> bool {
    let Ok(file) = File::open(app.join("Contents/Info.plist")) else {
        return false;
    };
    let mut info = String::new();
    if file
        .take(APP_INFO_LIMIT + 1)
        .read_to_string(&mut info)
        .is_err()
        || info.len() as u64 > APP_INFO_LIMIT
    {
        return false;
    }
    // The installed app must advertise a native directory document handler.
    // Unverifiable/binary plists fail closed; there is no shell fallback.
    document_types_accept_folders(&info)
}

fn installed_app(target: FileTarget) -> Option<PathBuf> {
    let system_paths: &[&str] = match target {
        FileTarget::Finder => &["/System/Library/CoreServices/Finder.app"],
        FileTarget::Terminal => &[
            "/System/Applications/Utilities/Terminal.app",
            "/Applications/Utilities/Terminal.app",
        ],
        _ => &[],
    };
    let app = system_paths
        .iter()
        .map(PathBuf::from)
        .find(|path| app_is_installed(target, path))
        .or_else(|| app_in_roots(target, &application_roots()))?;
    if target == FileTarget::Ghostty && !ghostty_accepts_folders(&app) {
        return None;
    }
    Some(app)
}

/// Discover only named, installed apps. Cache the result; never call from render.
/// Ghostty is included only when its installed bundle advertises native folder opening.
pub(super) fn available_targets() -> Vec<FileTarget> {
    if !Path::new("/usr/bin/open").is_file() {
        return Vec::new();
    }
    FileTarget::ALL
        .into_iter()
        .filter(|target| installed_app(*target).is_some())
        .collect()
}

/// Resolve one repository-relative file without trimming or unquoting its Git path.
/// Internal symlinks retain their selected path; links escaping the canonical workspace reject.
/// This performs filesystem reads and belongs on the same worker as `open_file`.
pub(super) fn resolve_file_path(project: &str, path: &str) -> Result<PathBuf, String> {
    if project.contains('\0') || path.contains('\0') {
        return Err("Workspace and file paths cannot contain null bytes.".into());
    }
    if project.is_empty() || !Path::new(project).is_absolute() {
        return Err("Select an existing absolute workspace folder first.".into());
    }
    if path.is_empty() {
        return Err("Select a file first.".into());
    }
    let relative = Path::new(path);
    if relative.is_absolute()
        || relative.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err("The selected file must be a relative path inside this workspace.".into());
    }
    let root = fs::canonicalize(project)
        .map_err(|error| format!("The workspace folder is unavailable: {error}"))?;
    if !root.is_dir() {
        return Err("The workspace path is not a folder.".into());
    }
    let file = root.join(relative);
    let canonical = fs::canonicalize(&file).map_err(|error| {
        if error.kind() == ErrorKind::NotFound {
            "The file no longer exists. It may have been deleted or renamed.".into()
        } else {
            format!("The selected file cannot be resolved: {error}")
        }
    })?;
    if !canonical.starts_with(&root) {
        return Err("The selected file resolves outside this workspace.".into());
    }
    let metadata = fs::metadata(&canonical)
        .map_err(|error| format!("The selected file is unavailable: {error}"))?;
    if !metadata.is_file() {
        return Err("The selected path is not a regular file.".into());
    }
    Ok(file)
}

/// Load the selected working-tree file for the internal editor. Recorded diffs and
/// trace snapshots never enter this route, and no write occurs until an explicit Save.
pub(super) fn load_repository_document(
    project: &str,
    path: &str,
) -> Result<agentwatcher::documents::Document, String> {
    let file = resolve_file_path(project, path)?;
    let root = fs::canonicalize(project)
        .map_err(|error| format!("The workspace folder is unavailable: {error}"))?;
    let document = agentwatcher::documents::load(&file)?;
    // Loading resolves aliases again. Reject a link retargeted outside the workspace
    // between selection validation and the document read.
    if !document.path.starts_with(&root) {
        return Err("The selected file resolves outside this workspace.".into());
    }
    Ok(document)
}

#[derive(Debug, PartialEq, Eq)]
struct OpenPlan {
    executable: &'static str,
    args: Vec<OsString>,
}

/// Pure argv construction after workspace and installation checks.
fn launch_plan(file: &Path, target: FileTarget, app: &Path) -> Result<OpenPlan, String> {
    if !file.is_absolute() || !app.is_absolute() {
        return Err("File-opening plans require absolute file and application paths.".into());
    }
    if file.as_os_str().as_bytes().contains(&0) || app.as_os_str().as_bytes().contains(&0) {
        return Err("File-opening paths cannot contain null bytes.".into());
    }
    let args = if target == FileTarget::Finder {
        vec![
            OsString::from("-R"),
            OsString::from("--"),
            file.as_os_str().to_owned(),
        ]
    } else {
        let selected = if target.opens_containing_folder() {
            file.parent()
                .ok_or("The selected file has no containing folder.")?
        } else {
            file
        };
        vec![
            OsString::from("-a"),
            app.as_os_str().to_owned(),
            OsString::from("--"),
            selected.as_os_str().to_owned(),
        ]
    };
    Ok(OpenPlan {
        executable: "/usr/bin/open",
        args,
    })
}

/// Only invoke for an explicit human click, from a LatestWorker or equivalent.
/// `open` is bounded to five seconds; apps are handed a file/folder through native
/// Launch Services, never through a shell, command string, `--args`, or script.
pub(super) fn open_file(project: &str, path: &str, target: FileTarget) -> Result<(), String> {
    let app = installed_app(target).ok_or_else(|| {
        if target == FileTarget::Ghostty {
            "Ghostty is unavailable or its native folder opening could not be verified.".into()
        } else {
            format!("{} is no longer available in Applications.", target.label())
        }
    })?;
    let file = resolve_file_path(project, path)?;
    let plan = launch_plan(&file, target, &app)?;
    // All selected paths are UTF-8 strings. Reject an unusual non-UTF-8 symlink
    // destination/root rather than silently changing its bytes with lossy conversion.
    let args = plan
        .args
        .iter()
        .map(|argument| {
            argument
                .to_str()
                .ok_or("The selected file or application path is not valid UTF-8.")
        })
        .collect::<Result<Vec<_>, _>>()?;
    backend::run(
        plan.executable,
        &args,
        None,
        OPEN_TIMEOUT,
        OPEN_OUTPUT_LIMIT,
    )
    .map(|_| ())
    .map_err(|error| format!("Could not open with {}: {error}", target.label()))
}

#[cfg(test)]
mod tests {
    use super::{
        FileTarget, app_in_roots, ghostty_accepts_folders, launch_plan, load_repository_document,
        resolve_file_path,
    };
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            for _ in 0..16 {
                let sequence = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir().join(format!(
                    "agentwatcher-file-actions-{}-{stamp}-{sequence}",
                    std::process::id()
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("Cannot create owned test fixture: {error}"),
                }
            }
            panic!("Cannot create a unique owned test fixture")
        }
        fn workspace(&self) -> PathBuf {
            let path = self.0.join("workspace space 界");
            fs::create_dir_all(&path).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            // Only this test's uniquely created subtree is disposable.
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn unusual_git_paths_remain_one_exact_file_argument() {
        let fixture = Fixture::new();
        let root = fixture.workspace();
        for name in [
            "with spaces.txt",
            "unicode-界-α.txt",
            "line\nbreak.txt",
            "-leading.txt",
            "$(touch nope);`echo nope`.txt",
        ] {
            fs::write(root.join(name), "fixture").unwrap();
            let file = resolve_file_path(root.to_str().unwrap(), name).unwrap();
            assert_eq!(file, fs::canonicalize(&root).unwrap().join(name));
            let plan =
                launch_plan(&file, FileTarget::Zed, Path::new("/Applications/Zed.app")).unwrap();
            assert_eq!(plan.executable, "/usr/bin/open");
            assert_eq!(
                plan.args,
                vec![
                    OsString::from("-a"),
                    OsString::from("/Applications/Zed.app"),
                    OsString::from("--"),
                    file.into_os_string()
                ]
            );
        }
        assert!(!root.join("nope").exists());
    }

    #[test]
    fn internal_editor_loads_working_tree_text_without_modifying_the_file() {
        let fixture = Fixture::new();
        let root = fixture.workspace();
        let path = "source with spaces 界.rs";
        let text = "fn main() {\n    println!(\"working tree\");\n}\n";
        fs::write(root.join(path), text).unwrap();
        let document = load_repository_document(root.to_str().unwrap(), path).unwrap();
        assert_eq!(document.path, fs::canonicalize(root.join(path)).unwrap());
        assert_eq!(document.text, text);
        assert_eq!(fs::read_to_string(root.join(path)).unwrap(), text);
        assert!(document.recovery.is_none());
    }

    #[test]
    fn internal_editor_rejects_binary_oversize_and_escaped_files() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let root = fixture.workspace();
        fs::write(root.join("binary.bin"), b"text\0binary").unwrap();
        fs::write(
            root.join("large.txt"),
            vec![b'x'; agentwatcher::documents::MAX_BYTES + 1],
        )
        .unwrap();
        fs::write(fixture.0.join("outside.txt"), "outside").unwrap();
        symlink(fixture.0.join("outside.txt"), root.join("escape.txt")).unwrap();
        for path in ["binary.bin", "large.txt", "escape.txt", "../outside.txt"] {
            assert!(
                load_repository_document(root.to_str().unwrap(), path).is_err(),
                "accepted {path}"
            );
        }
    }

    #[test]
    fn traversal_absolute_missing_null_and_directories_reject() {
        let fixture = Fixture::new();
        let root = fixture.workspace();
        fs::write(fixture.0.join("outside.txt"), "outside").unwrap();
        fs::create_dir(root.join("folder")).unwrap();
        let project = root.to_str().unwrap();
        for path in [
            "../outside.txt",
            "folder/../../outside.txt",
            "folder/../outside.txt",
            "bad\0name",
            "",
        ] {
            assert!(
                resolve_file_path(project, path).is_err(),
                "accepted {path:?}"
            );
        }
        assert!(
            resolve_file_path(project, fixture.0.join("outside.txt").to_str().unwrap()).is_err()
        );
        assert!(resolve_file_path("relative/workspace", "file.txt").is_err());
        assert!(resolve_file_path("bad\0project", "file.txt").is_err());
        assert!(
            resolve_file_path(project, "missing.txt")
                .unwrap_err()
                .contains("no longer exists")
        );
        assert!(
            resolve_file_path(project, "folder")
                .unwrap_err()
                .contains("not a regular file")
        );
    }

    #[test]
    fn deletion_after_git_selection_is_a_clear_error() {
        let fixture = Fixture::new();
        let root = fixture.workspace();
        fs::write(root.join("deleted.txt"), "fixture").unwrap();
        assert!(resolve_file_path(root.to_str().unwrap(), "deleted.txt").is_ok());
        fs::remove_file(root.join("deleted.txt")).unwrap();
        assert!(
            resolve_file_path(root.to_str().unwrap(), "deleted.txt")
                .unwrap_err()
                .contains("deleted or renamed")
        );
    }

    #[test]
    fn symlink_escape_rejects_but_internal_alias_keeps_its_path() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let root = fixture.workspace();
        fs::write(root.join("inside.txt"), "inside").unwrap();
        fs::write(fixture.0.join("outside.txt"), "outside").unwrap();
        symlink(fixture.0.join("outside.txt"), root.join("escape.txt")).unwrap();
        symlink(&fixture.0, root.join("escape-dir")).unwrap();
        symlink("inside.txt", root.join("alias.txt")).unwrap();
        for path in ["escape.txt", "escape-dir/outside.txt"] {
            assert!(
                resolve_file_path(root.to_str().unwrap(), path)
                    .unwrap_err()
                    .contains("outside this workspace")
            );
        }
        assert_eq!(
            resolve_file_path(root.to_str().unwrap(), "alias.txt").unwrap(),
            fs::canonicalize(&root).unwrap().join("alias.txt")
        );
    }

    #[test]
    fn finder_reveals_file_editors_open_file_and_terminals_only_open_parent() {
        let file = Path::new("/workspace/folder/-script\nname.sh");
        for target in FileTarget::ALL {
            let app = PathBuf::from("/Applications").join(target.bundle_names()[0]);
            let plan = launch_plan(file, target, &app).unwrap();
            assert_eq!(plan.executable, "/usr/bin/open");
            if target == FileTarget::Finder {
                assert_eq!(
                    plan.args,
                    vec![
                        OsString::from("-R"),
                        OsString::from("--"),
                        file.as_os_str().to_owned()
                    ]
                );
            } else {
                assert_eq!(
                    &plan.args[..3],
                    &[
                        OsString::from("-a"),
                        app.into_os_string(),
                        OsString::from("--")
                    ]
                );
                let expected = if matches!(target, FileTarget::Terminal | FileTarget::Ghostty) {
                    file.parent().unwrap()
                } else {
                    file
                };
                assert_eq!(plan.args[3], expected.as_os_str());
            }
            assert!(
                !plan
                    .args
                    .iter()
                    .any(|argument| argument == "--args" || argument == "-c")
            );
        }
        assert!(
            launch_plan(
                Path::new("relative.txt"),
                FileTarget::Finder,
                Path::new("/Applications/Finder.app")
            )
            .is_err()
        );
        assert!(launch_plan(file, FileTarget::Zed, Path::new("Zed.app")).is_err());
    }

    #[test]
    fn discovery_checks_only_named_apps_and_requires_ghostty_folder_support() {
        let fixture = Fixture::new();
        let applications = fixture.0.join("Applications");
        fs::create_dir_all(applications.join("Unrelated.app")).unwrap();
        assert!(app_in_roots(FileTarget::Zed, std::slice::from_ref(&applications)).is_none());
        let zed = applications.join("Zed.app");
        fs::create_dir_all(zed.join("Contents/MacOS")).unwrap();
        assert!(app_in_roots(FileTarget::Zed, std::slice::from_ref(&applications)).is_none());
        let zed_executable = zed.join("Contents/MacOS/zed");
        fs::write(&zed_executable, "test executable; never launched").unwrap();
        fs::set_permissions(&zed_executable, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            app_in_roots(FileTarget::Zed, std::slice::from_ref(&applications)),
            Some(zed)
        );
        let ghostty = applications.join("Ghostty.app");
        fs::create_dir_all(ghostty.join("Contents")).unwrap();
        assert!(!ghostty_accepts_folders(&ghostty));
        fs::write(ghostty.join("Contents/Info.plist"), "<plist><dict><key>CFBundleDocumentTypes</key><array><dict><key>LSItemContentTypes</key><array><string>public.directory</string></array></dict></array></dict></plist>").unwrap();
        assert!(ghostty_accepts_folders(&ghostty));
        fs::write(ghostty.join("Contents/Info.plist"), "<plist><dict><key>CFBundleDocumentTypes</key><array><dict><string>public.plain-text</string></dict></array></dict></plist>").unwrap();
        assert!(!ghostty_accepts_folders(&ghostty));
        fs::write(ghostty.join("Contents/Info.plist"),
            "<plist><dict><key>CFBundleDocumentTypes</key><array><dict><string>public.plain-text</string></dict></array><key>Other</key><string>public.directory</string></dict></plist>").unwrap();
        assert!(!ghostty_accepts_folders(&ghostty));
        fs::write(
            ghostty.join("Contents/Info.plist"),
            "x".repeat(128 * 1024 + 1),
        )
        .unwrap();
        assert!(!ghostty_accepts_folders(&ghostty));
    }
}
