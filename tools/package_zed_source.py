#!/usr/bin/env python3
"""Create a local, matching combined Corresponding Source archive.

AW_OUTPUT_DIR must name an existing absolute directory. This tool never fetches,
uploads, creates output directories, or replaces an existing output. The pinned
checkout is verified by setup_zed_engine.prepare(False) before it is packaged.
Tracked upstream symlinks are materialized only when their complete target chain
stays in that checkout and ends in a tracked regular file; the archive has no
symlink or hardlink entries. All other source symlinks are rejected.

--record-inputs records the verified input identities before compilation.
--check-inputs checks the same record after source packaging, including ctime and
inode changes when a file was edited and restored to identical contents.
"""
from __future__ import annotations

import argparse
from dataclasses import dataclass
import gzip
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import stat
import subprocess
import sys
import tarfile
import uuid

import setup_zed_engine
from zed_engine import PROJECT_ROOT, PreparationError


ARCHIVE_ROOT = "AgentWatcher-source"
MANIFEST_NAME = "SOURCE-MANIFEST.json"
FIXED_FILES = (
    "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "build-rust.sh", "README.md",
    "rust/NOTICE", "rust/Info.plist", "rust/zed-engine.toml",
    "tools/setup_zed_engine.py", "tools/zed_engine.py", "tools/package_zed_source.py",
    "tools/test_package_zed_source.py", "tools/test_setup_zed_engine.py",
    "tools/test_zed_engine.py", "tools/test_zed_bundle_signing.py", ".github/workflows/build.yml",
    ".github/workflows/release.yml",
)
SOURCE_DIRECTORIES = ("rust/src", "rust/resources", "rust/LICENSES", "rust/zed-patches")
EXCLUDED_COMPONENTS = frozenset({
    ".git", "target", "dist", "__pycache__", ".cache", ".pytest_cache",
    ".mypy_cache", ".ruff_cache", "node_modules", ".DS_Store",
})
VENDOR_ROOT = "rust/vendor/zed"
DIRECTORY_FLAGS = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
MAX_INPUT_RECORD_BYTES = 16 * 1024 * 1024


@dataclass(frozen=True)
class Snapshot:
    path: str
    identity: tuple[int, ...]


@dataclass(frozen=True)
class SourceFile:
    path: str
    content_path: str
    size: int
    mode: int
    snapshots: tuple[Snapshot, ...]
    links: tuple[tuple[str, str], ...] = ()


def _relative(value: str) -> PurePosixPath:
    path = PurePosixPath(value)
    if (not value or path.is_absolute() or any(p in ("", ".", "..") for p in value.split("/"))
            or "\\" in value or "\0" in value):
        raise PreparationError(f"Unsafe source path: {value!r}")
    return path


def _open_directory(path: Path) -> int:
    if not path.is_absolute() or ".." in path.parts:
        raise PreparationError("Directories must use absolute paths without parent traversal")
    descriptor = os.open("/", DIRECTORY_FLAGS)
    try:
        for part in path.parts[1:]:
            child = os.open(part, DIRECTORY_FLAGS, dir_fd=descriptor)
            os.close(descriptor)
            descriptor = child
        return descriptor
    except BaseException:
        os.close(descriptor)
        raise


def _parent(root: Path, relative: str) -> tuple[int, str]:
    path = _relative(relative)
    return _open_directory(root.joinpath(*path.parts[:-1])), path.name


def _lstat(root: Path, relative: str) -> os.stat_result:
    descriptor, name = _parent(root, relative)
    try:
        return os.stat(name, dir_fd=descriptor, follow_symlinks=False)
    finally:
        os.close(descriptor)


def _identity(info: os.stat_result) -> tuple[int, ...]:
    return (info.st_dev, info.st_ino, info.st_mode, info.st_size,
            info.st_mtime_ns, info.st_ctime_ns)


def _regular(root: Path, relative: str) -> SourceFile:
    info = _lstat(root, relative)
    if not stat.S_ISREG(info.st_mode):
        raise PreparationError(f"Source input is not a regular file: {relative}")
    return SourceFile(relative, relative, info.st_size,
                      0o755 if info.st_mode & 0o111 else 0o644,
                      (Snapshot(relative, _identity(info)),))


def _git(source: Path, *arguments: str) -> bytes:
    environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    environment.update({"GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
                        "GIT_TERMINAL_PROMPT": "0", "GIT_OPTIONAL_LOCKS": "0"})
    result = subprocess.run(
        ["git", "-c", "core.fsmonitor=false", "-C", str(source), *arguments],
        capture_output=True, timeout=60, env=environment,
    )
    if result.returncode or len(result.stdout) > 8 * 1024 * 1024:
        raise PreparationError("Could not completely inventory the pinned source Git index")
    return result.stdout


def _tracked(source: Path) -> dict[str, tuple[str, str]]:
    tracked = {}
    for record in _git(source, "ls-files", "--stage", "-z").split(b"\0"):
        if not record:
            continue
        header, raw_path = record.split(b"\t", 1)
        mode, digest, stage = header.decode("ascii").split()
        relative = raw_path.decode("utf-8")
        _relative(relative)
        if stage != "0" or mode not in {"100644", "100755", "120000"}:
            raise PreparationError(f"Unsupported or conflicted tracked input: {relative}")
        tracked[relative] = (mode, digest)
    if not tracked:
        raise PreparationError("Pinned source Git index is empty")
    return tracked


def _link_target(path: str, target: str) -> str:
    if not target or target.startswith("/") or "\\" in target:
        raise PreparationError(f"Unsafe tracked symlink target: {path}")
    parts = list(PurePosixPath(path).parent.parts)
    for part in target.split("/"):
        if part in ("", "."):
            continue
        if part == "..":
            if not parts:
                raise PreparationError(f"Tracked symlink escapes the pinned source: {path}")
            parts.pop()
        else:
            parts.append(part)
    relative = "/".join(parts)
    _relative(relative)
    return relative


def _upstream_file(root: Path, relative: str, tracked: dict) -> SourceFile:
    original = relative
    snapshots = []
    links = []
    seen = set()
    while True:
        if relative in seen or len(seen) >= 40:
            raise PreparationError(f"Tracked symlink cycle or excessive depth: {original}")
        seen.add(relative)
        if relative not in tracked:
            raise PreparationError(f"Tracked symlink does not end in tracked source: {original}")
        archive_path = f"{VENDOR_ROOT}/{relative}"
        info = _lstat(root, archive_path)
        snapshots.append(Snapshot(archive_path, _identity(info)))
        mode = tracked[relative][0]
        if mode != "120000":
            if not stat.S_ISREG(info.st_mode):
                raise PreparationError(f"Tracked source changed type: {relative}")
            return SourceFile(f"{VENDOR_ROOT}/{original}", archive_path, info.st_size,
                              0o755 if mode == "100755" else 0o644,
                              tuple(snapshots), tuple(links))
        if not stat.S_ISLNK(info.st_mode):
            raise PreparationError(f"Tracked symlink changed type: {relative}")
        descriptor, name = _parent(root, archive_path)
        try:
            target = os.readlink(name, dir_fd=descriptor)
            if _identity(os.stat(name, dir_fd=descriptor, follow_symlinks=False)) != snapshots[-1].identity:
                raise PreparationError(f"Tracked symlink changed while inspected: {relative}")
        finally:
            os.close(descriptor)
        links.append((relative, target))
        relative = _link_target(relative, target)


def _excluded(relative: str) -> bool:
    return bool(set(PurePosixPath(relative).parts) & EXCLUDED_COMPONENTS)


def _walk(root: Path, relative: str):
    descriptor = _open_directory(root / relative)
    try:
        names = sorted(os.listdir(descriptor))
        entries = [(name, os.stat(name, dir_fd=descriptor, follow_symlinks=False)) for name in names]
    finally:
        os.close(descriptor)
    for name, info in entries:
        child = f"{relative}/{name}"
        if _excluded(child) or name == "AGENTS.md":
            continue
        if stat.S_ISDIR(info.st_mode):
            yield from _walk(root, child)
        else:
            yield _regular(root, child)


def _inventory(root: Path, source_manifest: dict | None = None) -> tuple[tuple[SourceFile, ...], dict]:
    if source_manifest is not None:
        files = []
        tracked = {}
        for record in source_manifest["files"]:
            relative = record["path"]
            _relative(relative)
            if (_excluded(relative) or (PurePosixPath(relative).name == "AGENTS.md"
                    and not relative.startswith(VENDOR_ROOT + "/")) or not (relative in FIXED_FILES or any(
                    relative.startswith(directory + "/") for directory in (*SOURCE_DIRECTORIES, VENDOR_ROOT)))):
                raise PreparationError(f"Extracted source manifest input is outside the allowlist: {relative}")
            entry = _regular(root, relative)
            if entry.size != record["size"] or f"{entry.mode:04o}" != record["mode"]:
                raise PreparationError(f"Extracted source input changed after validation: {relative}")
            files.append(entry)
            if relative.startswith(VENDOR_ROOT + "/"):
                mode = "100755" if entry.mode == 0o755 else "100644"
                tracked[relative.removeprefix(VENDOR_ROOT + "/")] = (mode, record["sha256"])
        paths = {entry.path for entry in files}
        if not set(FIXED_FILES) <= paths:
            raise PreparationError("Extracted source manifest omits required build inputs")
        if not tracked or len(files) != len(paths):
            raise PreparationError("Extracted source manifest is empty or contains duplicate paths")
        return tuple(sorted(files, key=lambda entry: entry.path)), tracked
    files = [_regular(root, relative) for relative in FIXED_FILES]
    for relative in SOURCE_DIRECTORIES:
        files.extend(_walk(root, relative))
    tracked = _tracked(root / VENDOR_ROOT)
    for relative in sorted(tracked):
        if not _excluded(relative):
            files.append(_upstream_file(root, relative, tracked))
    paths = [entry.path for entry in files]
    if len(paths) != len(set(paths)):
        raise PreparationError("Duplicate source archive path")
    return tuple(sorted(files, key=lambda entry: entry.path)), tracked


def _archive_manifest(root: Path, verified: dict) -> dict | None:
    if not verified.get("archive"):
        return None
    config = setup_zed_engine.read_config(root / "rust/zed-engine.toml")
    manifest = setup_zed_engine.archive_manifest(config)
    if (not manifest or manifest["upstream"]["revision"] != verified["revision"]
            or manifest["upstream"]["patches"] != verified["patches"]):
        raise PreparationError("Extracted source manifest does not match setup verification")
    return manifest


class _HashingReader:
    def __init__(self, stream):
        self.stream = stream
        self.digest = hashlib.sha256()
        self.size = 0

    def read(self, size=-1):
        data = self.stream.read(size)
        self.digest.update(data)
        self.size += len(data)
        return data


class _HashingWriter:
    def __init__(self, stream):
        self.stream = stream
        self.digest = hashlib.sha256()

    def write(self, data):
        written = self.stream.write(data)
        self.digest.update(data[:written])
        return written

    def flush(self):
        self.stream.flush()


def _assert_unchanged(root: Path, entry: SourceFile) -> None:
    for snapshot in entry.snapshots:
        if _identity(_lstat(root, snapshot.path)) != snapshot.identity:
            raise PreparationError(f"Source changed during packaging: {entry.path}")


def _write_entry(archive: tarfile.TarFile, root: Path, entry: SourceFile) -> dict:
    _assert_unchanged(root, entry)
    descriptor, name = _parent(root, entry.content_path)
    try:
        file_descriptor = os.open(name, os.O_RDONLY | os.O_NOFOLLOW, dir_fd=descriptor)
    finally:
        os.close(descriptor)
    with os.fdopen(file_descriptor, "rb") as stream:
        if _identity(os.fstat(stream.fileno())) != entry.snapshots[-1].identity:
            raise PreparationError(f"Source changed before reading: {entry.path}")
        reader = _HashingReader(stream)
        info = tarfile.TarInfo(f"{ARCHIVE_ROOT}/{entry.path}")
        info.size, info.mode, info.mtime = entry.size, entry.mode, 0
        archive.addfile(info, reader)
        if reader.size != entry.size or _identity(os.fstat(stream.fileno())) != entry.snapshots[-1].identity:
            raise PreparationError(f"Source changed while reading: {entry.path}")
        _assert_unchanged(root, entry)
    return {"path": entry.path, "size": entry.size, "mode": f"{entry.mode:04o}",
            "sha256": reader.digest.hexdigest()}


def _destination(output: Path, output_root: Path, project_root: Path, *,
                 suffix: str | None = ".tar.gz", existing: bool = False) -> tuple[int, str]:
    if not output.is_absolute() or not output_root.is_absolute():
        raise PreparationError("Selected path and AW_OUTPUT_DIR must be absolute paths")
    if ".." in output.parts or ".." in output_root.parts or not output.is_relative_to(output_root):
        raise PreparationError("Selected path must stay inside AW_OUTPUT_DIR without parent traversal")
    if output == output_root or (suffix is not None and not output.name.endswith(suffix)):
        raise PreparationError("Selected path must name a suitable file inside AW_OUTPUT_DIR")
    for relative in (*SOURCE_DIRECTORIES, VENDOR_ROOT):
        if output.is_relative_to(project_root / relative):
            raise PreparationError("Source archives cannot be written inside source input directories")
    root_descriptor = _open_directory(output_root)
    os.close(root_descriptor)
    descriptor = _open_directory(output.parent)
    try:
        try:
            info = os.stat(output.name, dir_fd=descriptor, follow_symlinks=False)
        except FileNotFoundError:
            if existing:
                raise PreparationError("Input record does not exist")
            return descriptor, output.name
        if existing:
            if not stat.S_ISREG(info.st_mode):
                raise PreparationError("Input record must be a regular file")
            return descriptor, output.name
        raise PreparationError("Output already exists; existing files are protected")
    except BaseException:
        os.close(descriptor)
        raise


def _canonical(value: dict) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode("utf-8")


def _input_state(project_root: Path, prepare_source=None) -> dict:
    verify = prepare_source or setup_zed_engine.prepare
    verified = verify(False)
    if Path(verified["source"]) != project_root / VENDOR_ROOT:
        raise PreparationError("Verified source is not the expected vendored checkout")
    source_manifest = _archive_manifest(project_root, verified)
    files, tracked = _inventory(project_root, source_manifest)
    manifest_identity = list(_identity(_lstat(project_root, MANIFEST_NAME))) if source_manifest else None
    if verify(False) != verified:
        raise PreparationError("Pinned source verification changed while recording inputs")
    current_manifest = _archive_manifest(project_root, verified)
    if current_manifest != source_manifest or _inventory(project_root, current_manifest) != (files, tracked):
        raise PreparationError("Source inputs changed while recording inputs")
    if source_manifest and list(_identity(_lstat(project_root, MANIFEST_NAME))) != manifest_identity:
        raise PreparationError("Extracted source manifest changed while recording inputs")
    return {
        "project-root": str(project_root), "setup": verified,
        "source-manifest-sha256": hashlib.sha256(_canonical(source_manifest)).hexdigest() if source_manifest else None,
        "source-manifest-identity": manifest_identity,
        "files": [{"path": entry.path, "content-path": entry.content_path,
                   "size": entry.size, "mode": entry.mode,
                   "identities": [{"path": snapshot.path, "identity": list(snapshot.identity)}
                                  for snapshot in entry.snapshots],
                   "link-chains": [list(link) for link in entry.links]} for entry in files],
        "tracked-index": {path: list(identity) for path, identity in sorted(tracked.items())},
    }


def record_inputs(output: Path, output_root: Path, *, project_root: Path = PROJECT_ROOT,
                  prepare_source=None) -> dict:
    """Exclusively record identities of the verified inputs before a build."""
    parent_descriptor, name = _destination(output, output_root, project_root, suffix=None)
    partial_name = f".{name}.{uuid.uuid4().hex}.partial"
    partial_identity = None
    try:
        state = _input_state(project_root, prepare_source)
        digest = hashlib.sha256(_canonical(state)).hexdigest()
        record = _canonical({"schema-version": 1, "input-state": state, "state-sha256": digest}) + b"\n"
        if len(record) > MAX_INPUT_RECORD_BYTES:
            raise PreparationError("Verified input record exceeds the bounded record size")
        descriptor = os.open(partial_name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                             0o600, dir_fd=parent_descriptor)
        partial_identity = _identity(os.fstat(descriptor))[:2]
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(record)
            stream.flush()
            os.fsync(stream.fileno())
            completed_identity = _identity(os.fstat(stream.fileno()))
        if _identity(os.stat(partial_name, dir_fd=parent_descriptor, follow_symlinks=False)) != completed_identity:
            raise PreparationError("Input record changed before publication")
        os.link(partial_name, name, src_dir_fd=parent_descriptor,
                dst_dir_fd=parent_descriptor, follow_symlinks=False)
        os.fsync(parent_descriptor)
        return {"record": str(output), "state-sha256": digest, "file-count": len(state["files"])}
    finally:
        try:
            if partial_identity is not None:
                try:
                    info = os.stat(partial_name, dir_fd=parent_descriptor, follow_symlinks=False)
                    if (info.st_dev, info.st_ino) == partial_identity:
                        os.unlink(partial_name, dir_fd=parent_descriptor)
                except FileNotFoundError:
                    pass
        finally:
            os.close(parent_descriptor)


def check_inputs(record: Path, output_root: Path, *, project_root: Path = PROJECT_ROOT,
                 prepare_source=None) -> dict:
    """Read a prior record without modifying it, rejecting changed build inputs."""
    parent_descriptor, name = _destination(record, output_root, project_root, suffix=None, existing=True)
    try:
        descriptor = os.open(name, os.O_RDONLY | os.O_NOFOLLOW, dir_fd=parent_descriptor)
        with os.fdopen(descriptor, "rb") as stream:
            before = os.fstat(stream.fileno())
            if not stat.S_ISREG(before.st_mode) or before.st_size > MAX_INPUT_RECORD_BYTES:
                raise PreparationError("Input record is not a bounded regular file")
            content = stream.read(MAX_INPUT_RECORD_BYTES + 1)
            if len(content) > MAX_INPUT_RECORD_BYTES or _identity(os.fstat(stream.fileno())) != _identity(before):
                raise PreparationError("Input record changed while reading")
        stored = json.loads(content)
        if (not isinstance(stored, dict) or stored.get("schema-version") != 1
                or not isinstance(stored.get("input-state"), dict)
                or hashlib.sha256(_canonical(stored["input-state"])).hexdigest() != stored.get("state-sha256")):
            raise PreparationError("Input record schema or digest is invalid")
        current = _input_state(project_root, prepare_source)
        if stored["input-state"] != current:
            raise PreparationError("Source inputs changed since the pre-build record; binary/source matching is unverified")
        if _identity(os.stat(name, dir_fd=parent_descriptor, follow_symlinks=False)) != _identity(before):
            raise PreparationError("Input record changed during verification")
        return {"record": str(record), "state-sha256": stored["state-sha256"],
                "file-count": len(current["files"]), "inputs-unchanged": True}
    finally:
        os.close(parent_descriptor)


def package(output: Path, output_root: Path, *, project_root: Path = PROJECT_ROOT,
            prepare_source=None) -> dict:
    """Stream verified inputs and atomically publish a new archive with no overwrite."""
    parent_descriptor, name = _destination(output, output_root, project_root)
    partial_name = f".{name}.{uuid.uuid4().hex}.partial"
    partial_identity = None
    try:
        verify = prepare_source or setup_zed_engine.prepare
        verified = verify(False)
        if Path(verified["source"]) != project_root / VENDOR_ROOT:
            raise PreparationError("Verified source is not the expected vendored checkout")
        source_manifest = _archive_manifest(project_root, verified)
        files, tracked = _inventory(project_root, source_manifest)
        if source_manifest is None and _git(project_root / VENDOR_ROOT, "rev-parse", "HEAD").decode().strip() != verified["revision"]:
            raise PreparationError("Pinned source revision changed before packaging")
        descriptor = os.open(partial_name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                             0o600, dir_fd=parent_descriptor)
        partial_identity = _identity(os.fstat(descriptor))[:2]
        with os.fdopen(descriptor, "wb") as stream:
            writer = _HashingWriter(stream)
            with gzip.GzipFile(filename="", mode="wb", fileobj=writer, mtime=0) as compressed:
                with tarfile.open(fileobj=compressed, mode="w|", format=tarfile.PAX_FORMAT) as archive:
                    records = [_write_entry(archive, project_root, entry) for entry in files]
                    if source_manifest is not None and records != sorted(source_manifest["files"], key=lambda record: record["path"]):
                        raise PreparationError("Extracted source content changed after validation")
                    symlinks = [{"path": entry.path, "content_path": entry.content_path,
                                 "chain": [{"path": path, "target": target} for path, target in entry.links]}
                                for entry in files if entry.links]
                    if source_manifest is not None:
                        symlinks = source_manifest["materialized-symlinks"]
                    manifest = json.dumps({
                        "schema-version": 1, "archive-root": ARCHIVE_ROOT,
                        "upstream": {"revision": verified["revision"], "patches": verified["patches"]},
                        "files": records, "materialized-symlinks": symlinks,
                    }, sort_keys=True, indent=2).encode("utf-8") + b"\n"
                    info = tarfile.TarInfo(f"{ARCHIVE_ROOT}/{MANIFEST_NAME}")
                    info.size, info.mode, info.mtime = len(manifest), 0o644, 0
                    archive.addfile(info, io.BytesIO(manifest))
            # Revalidate the pin/controlled changes as well as every source,
            # link chain and index entry. This also closes the interval between
            # the initial setup verification and the first file inventory.
            if verify(False) != verified:
                raise PreparationError("Pinned source verification changed during packaging")
            current_manifest = _archive_manifest(project_root, verified)
            if current_manifest != source_manifest:
                raise PreparationError("Extracted source manifest changed during packaging")
            current_files, current_tracked = _inventory(project_root, current_manifest)
            if current_files != files or current_tracked != tracked:
                raise PreparationError("Source inputs changed during packaging")
            if source_manifest is None and _git(project_root / VENDOR_ROOT, "rev-parse", "HEAD").decode().strip() != verified["revision"]:
                raise PreparationError("Pinned source revision changed during packaging")
            stream.flush()
            os.fsync(stream.fileno())
            digest = writer.digest.hexdigest()
            completed_identity = _identity(os.fstat(stream.fileno()))
        # link() is an atomic, exclusive publication; rename() could overwrite
        # a file created by another process after the initial existence check.
        if _identity(os.stat(partial_name, dir_fd=parent_descriptor, follow_symlinks=False)) != completed_identity:
            raise PreparationError("Completed source archive changed before publication")
        os.link(partial_name, name, src_dir_fd=parent_descriptor,
                dst_dir_fd=parent_descriptor, follow_symlinks=False)
        os.fsync(parent_descriptor)
        return {"output": str(output), "sha256": digest,
                "manifest-sha256": hashlib.sha256(manifest).hexdigest(),
                "file-count": len(files), "upstream-revision": verified["revision"]}
    finally:
        if partial_identity is not None:
            try:
                info = os.stat(partial_name, dir_fd=parent_descriptor, follow_symlinks=False)
                if (info.st_dev, info.st_ino) == partial_identity:
                    os.unlink(partial_name, dir_fd=parent_descriptor)
            except FileNotFoundError:
                pass
        os.close(parent_descriptor)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    action = parser.add_mutually_exclusive_group(required=True)
    action.add_argument("--output", type=Path, help="absolute path of a NEW .tar.gz file")
    action.add_argument("--record-inputs", type=Path, help="absolute path of a NEW pre-build JSON record")
    action.add_argument("--check-inputs", type=Path, help="absolute path of an existing pre-build JSON record")
    args = parser.parse_args()
    try:
        output_root = os.environ.get("AW_OUTPUT_DIR")
        if not output_root:
            raise PreparationError("AW_OUTPUT_DIR must name an already-created output directory")
        if args.record_inputs is not None:
            result = record_inputs(args.record_inputs, Path(output_root))
        elif args.check_inputs is not None:
            result = check_inputs(args.check_inputs, Path(output_root))
        else:
            result = package(args.output, Path(output_root))
        print(json.dumps(result, sort_keys=True))
        return 0
    except (PreparationError, OSError, ValueError, KeyError, UnicodeError, subprocess.TimeoutExpired) as error:
        print(f"Source packaging stopped: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
