//! Bounded local UTF-8 documents; explicit saves reject changed files.
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const MAX_BYTES: usize = 1024 * 1024;
static NEXT_SAVE: AtomicU64 = AtomicU64::new(1);
#[derive(Clone, Debug)]
pub struct Document {
    pub path: PathBuf,
    pub text: String,
    pub recovery: Option<PathBuf>,
    device: u64,
    inode: u64,
    mode: u32,
}

pub fn load(path: &Path) -> Result<Document, String> {
    if !path.is_absolute() {
        return Err("Choose an absolute local file path.".into());
    }
    let path = path
        .canonicalize()
        .map_err(|e| format!("Cannot open file: {e}"))?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)
        .map_err(|e| format!("Cannot open file: {e}"))?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > MAX_BYTES as u64 {
        return Err("The editor supports regular UTF-8 files up to 1 MiB.".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_BYTES {
        return Err("File grew beyond the 1 MiB editor limit.".into());
    }
    let text = String::from_utf8(bytes).map_err(|_| "This file is not UTF-8 text.".to_owned())?;
    if text.contains('\0') {
        return Err("Binary files cannot be edited as text.".into());
    }
    Ok(Document {
        path,
        text,
        device: metadata.dev(),
        inode: metadata.ino(),
        mode: metadata.mode(),
        recovery: None,
    })
}

pub fn save(document: &Document, text: String) -> Result<Document, String> {
    let root = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("Home directory is unavailable.")?
        .join("Library/Application Support/AgentWatcher/Recovery");
    save_at_commit(document, text, &root, || {}, || {})
}
fn same_version(a: &Document, b: &Document) -> bool {
    a.device == b.device && a.inode == b.inode && a.text == b.text
}
fn exchange(a: &Path, b: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let a = std::ffi::CString::new(a.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
        let b = std::ffi::CString::new(b.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
        if unsafe {
            libc::renameatx_np(
                libc::AT_FDCWD,
                a.as_ptr(),
                libc::AT_FDCWD,
                b.as_ptr(),
                libc::RENAME_SWAP,
            )
        } != 0
        {
            return Err(format!(
                "Atomic save is unavailable: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (a, b);
        Err("Atomic document saving requires macOS.".into())
    }
}
fn save_at_commit(
    document: &Document,
    text: String,
    recovery_root: &Path,
    before_exchange: impl FnOnce(),
    after_exchange: impl FnOnce(),
) -> Result<Document, String> {
    if text.len() > MAX_BYTES || text.contains('\0') {
        return Err("The edited text exceeds the supported document limit.".into());
    }
    let current = load(&document.path)?;
    if current.device != document.device
        || current.inode != document.inode
        || current.text != document.text
    {
        return Err(
            "File changed on disk. Your edits are preserved. Reload the file before saving.".into(),
        );
    }
    let metadata = fs::symlink_metadata(&document.path).map_err(|e| e.to_string())?;
    if metadata.file_type().is_symlink() || metadata.nlink() != 1 {
        return Err(
            "Saving a linked or replaced file is not supported. Your edits are preserved.".into(),
        );
    }
    if metadata.permissions().readonly() {
        return Err("The file is read only. Your edits are preserved.".into());
    }
    fs::create_dir_all(recovery_root)
        .map_err(|e| format!("Cannot create recovery storage: {e}"))?;
    let recovery_dir = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(recovery_root)
        .map_err(|e| format!("Cannot open recovery storage: {e}"))?;
    if recovery_dir.metadata().map_err(|e| e.to_string())?.dev() != document.device {
        return Err(
            "Safe saving requires recovery storage on the same volume. Your edits are preserved."
                .into(),
        );
    }
    if unsafe { libc::flock(recovery_dir.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("Recovery storage is busy. Try Save again.".into());
    }
    let mut recovery_count = 0usize;
    let mut recovery_bytes = 0u64;
    for entry in fs::read_dir(recovery_root)
        .map_err(|e| e.to_string())?
        .take(257)
    {
        let entry = entry.map_err(|e| e.to_string())?;
        recovery_count += 1;
        recovery_bytes =
            recovery_bytes.saturating_add(entry.metadata().map_err(|e| e.to_string())?.len());
    }
    if recovery_count >= 256
        || recovery_bytes.saturating_add(document.text.len() as u64) > 64 * 1024 * 1024
    {
        return Err("Recovery storage reached its 256 version / 64 MiB limit. Manage previous versions in AgentWatcher's Recovery folder before saving; your draft is preserved.".into());
    }
    let parent = document
        .path
        .parent()
        .ok_or("File has no parent directory.")?;
    let temporary = parent.join(format!(
        ".agentwatcher-save-{}-{}.tmp",
        std::process::id(),
        NEXT_SAVE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut created = false;
    let mut cleanup_safe = true;
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(document.mode & 0o777)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        created = true;
        file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        file.set_permissions(fs::Permissions::from_mode(document.mode & 0o777))
            .map_err(|e| e.to_string())?;
        #[cfg(target_os = "macos")]
        {
            let source = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&document.path)
                .map_err(|e| e.to_string())?;
            if unsafe {
                libc::fcopyfile(
                    source.as_raw_fd(),
                    file.as_raw_fd(),
                    std::ptr::null_mut(),
                    libc::COPYFILE_METADATA,
                )
            } != 0
            {
                return Err(format!(
                    "Cannot preserve file metadata: {}",
                    std::io::Error::last_os_error()
                ));
            }
            file.set_modified(std::time::SystemTime::now())
                .map_err(|e| e.to_string())?;
        }
        file.sync_all().map_err(|e| e.to_string())?;
        let check = load(&document.path)?;
        if check.device != document.device
            || check.inode != document.inode
            || check.text != document.text
        {
            return Err("File changed while saving. Your edits are preserved.".into());
        }
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        let mut saved = Document {
            path: document.path.clone(),
            text,
            device: metadata.dev(),
            inode: metadata.ino(),
            mode: metadata.mode(),
            recovery: None,
        };
        before_exchange();
        exchange(&temporary, &document.path)?;
        cleanup_safe = false;
        // The displaced file remains intact until its exact pre-save version is verified.
        let displaced = load(&temporary);
        if !displaced
            .as_ref()
            .is_ok_and(|old| same_version(old, document))
        {
            let current = load(&document.path);
            if current
                .as_ref()
                .is_ok_and(|current| same_version(current, &saved))
            {
                if exchange(&temporary, &document.path).is_ok() {
                    cleanup_safe = load(&temporary)
                        .as_ref()
                        .is_ok_and(|candidate| same_version(candidate, &saved));
                }
            }
            return Err(if cleanup_safe {
                "A concurrent disk change was detected and restored. Your edits are preserved in the editor.".into()
            } else {
                format!(
                    "A concurrent disk change was detected. Your edits are preserved; recovery data was retained at {}",
                    temporary.display()
                )
            });
        }
        let recovery = recovery_root.join(format!(
            "{}-{}-{}.backup",
            std::process::id(),
            NEXT_SAVE.fetch_add(1, Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        // A hard link retains the displaced inode, including late writes through preopened handles.
        fs::hard_link(&temporary, &recovery).map_err(|e| {
            format!(
                "Saved file, but keep the previous version at {}: {e}",
                temporary.display()
            )
        })?;
        saved.recovery = Some(recovery);
        cleanup_safe = true;
        after_exchange();
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
        // Never adopt an unseen post-save write as the baseline for the next save.
        Ok(saved)
    })();
    // This exact file was created by this save; never remove any source document.
    if created && cleanup_safe && temporary.exists() {
        let _ = fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "aw-document-{}-{}",
                std::process::id(),
                NEXT_SAVE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    #[test]
    fn save_preserves_unicode_newlines_and_mode_and_rejects_external_changes() {
        let fixture = Fixture::new();
        let path = fixture.0.join("AGENTS.md");
        fs::write(&path, "hello\r\n世界\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        let original = load(&path).unwrap();
        let saved = save_at_commit(
            &original,
            "edited 🦀\r\n".into(),
            &fixture.0.join("recovery"),
            || {},
            || {},
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "edited 🦀\r\n");
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o640);
        fs::write(&path, "External").unwrap();
        assert!(
            save_at_commit(
                &saved,
                "Overwrite".into(),
                &fixture.0.join("recovery"),
                || {},
                || {}
            )
            .is_err()
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "External");
    }
    #[test]
    fn binary_large_and_replaced_files_fail_without_losing_source() {
        let fixture = Fixture::new();
        let path = fixture.0.join("file");
        fs::write(&path, "source").unwrap();
        let original = load(&path).unwrap();
        let replacement = fixture.0.join("replacement");
        fs::write(&replacement, "source").unwrap();
        fs::rename(&replacement, &path).unwrap();
        assert!(
            save_at_commit(
                &original,
                "new".into(),
                &fixture.0.join("recovery"),
                || {},
                || {}
            )
            .is_err()
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "source");
        fs::write(&path, [0, 1, 2]).unwrap();
        assert!(load(&path).is_err());
        fs::write(&path, vec![b'a'; MAX_BYTES + 1]).unwrap();
        assert!(load(&path).is_err());
    }
    #[test]
    fn concurrent_write_at_commit_is_restored_and_post_save_write_is_not_adopted() {
        let fixture = Fixture::new();
        let path = fixture.0.join("AGENTS.md");
        fs::write(&path, "Original").unwrap();
        let original = load(&path).unwrap();
        let result = save_at_commit(
            &original,
            "My edit".into(),
            &fixture.0.join("recovery"),
            || {
                fs::write(&path, "Concurrent").unwrap();
            },
            || {},
        );
        assert!(result.is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "Concurrent");
        let baseline = load(&path).unwrap();
        let saved = save_at_commit(
            &baseline,
            "Saved".into(),
            &fixture.0.join("recovery"),
            || {},
            || {
                fs::write(&path, "Later external edit").unwrap();
            },
        )
        .unwrap();
        assert_eq!(saved.text, "Saved");
        assert!(
            save_at_commit(
                &saved,
                "Next edit".into(),
                &fixture.0.join("recovery"),
                || {},
                || {}
            )
            .is_err()
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "Later external edit");
    }
    #[test]
    fn displaced_inode_retains_writes_from_a_preopened_external_writer() {
        let fixture = Fixture::new();
        let path = fixture.0.join("AGENTS.md");
        fs::write(&path, "Original").unwrap();
        let baseline = load(&path).unwrap();
        let mut external = OpenOptions::new().write(true).open(&path).unwrap();
        let saved = save_at_commit(
            &baseline,
            "Saved".into(),
            &fixture.0.join("recovery"),
            || {},
            || {
                external.write_all(b"Changed!").unwrap();
                external.sync_all().unwrap();
            },
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "Saved");
        assert_eq!(
            fs::read_to_string(saved.recovery.unwrap()).unwrap(),
            "Changed!"
        );
    }
}
