#!/usr/bin/env python3
"""Prepare the pinned source used by AgentWatcher's actual embedded editor.

Run with --fetch once to create the checkout. Later invocations validate the
exact pin and apply only the repository's reviewed integration patches. Existing
foreign or modified checkouts are never reset, overwritten, or removed.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import stat
import subprocess
import sys

from zed_engine import PROJECT_ROOT, PreparationError, read_config

SOURCE = PROJECT_ROOT / "rust/vendor/zed"
PATCH_ROOT = PROJECT_ROOT / "rust/zed-patches"


def archive_manifest(config: dict) -> dict:
    """Verify every declared source input in a matching extracted source archive.

    This checks archive integrity, not an independent signature on its declared
    provenance. The source archive SHA-256 is supplied alongside the binary.
    """
    path = PROJECT_ROOT / "SOURCE-MANIFEST.json"
    if path.is_symlink() or not path.is_file() or path.stat().st_size > 8 * 1024 * 1024:
        raise PreparationError("Missing or invalid extracted-source manifest")
    manifest = json.loads(path.read_text())
    if manifest.get("schema-version") != 1 or manifest.get("archive-root") != "AgentWatcher-source":
        raise PreparationError("Unsupported extracted-source manifest")
    if manifest.get("upstream", {}).get("revision") != config["upstream"]["revision"]:
        raise PreparationError("Extracted source declares a different upstream pin")
    files = manifest.get("files")
    if not isinstance(files, list) or not 1 <= len(files) <= 20000:
        raise PreparationError("Invalid extracted-source inventory")
    seen = set()
    for entry in files:
        name = entry.get("path", "")
        relative = PurePosixPath(name)
        if not name or relative.is_absolute() or any(p in ("", ".", "..") for p in name.split("/")) or "\\" in name:
            raise PreparationError("Unsafe extracted-source path")
        if name in seen:
            raise PreparationError("Duplicate extracted-source path")
        seen.add(name)
        parent = os.open(PROJECT_ROOT, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            for component in relative.parts[:-1]:
                child = os.open(component, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=parent)
                os.close(parent)
                parent = child
            descriptor = os.open(relative.name, os.O_RDONLY | os.O_NOFOLLOW, dir_fd=parent)
        finally:
            os.close(parent)
        with os.fdopen(descriptor, "rb") as stream:
            before = os.fstat(stream.fileno())
            if not stat.S_ISREG(before.st_mode) or before.st_size != entry["size"]:
                raise PreparationError(f"Extracted-source size changed: {name}")
            if f"{stat.S_IMODE(before.st_mode):04o}" != entry["mode"]:
                raise PreparationError(f"Extracted-source mode changed: {name}")
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
            after = os.fstat(stream.fileno())
            if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) != (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns):
                raise PreparationError(f"Extracted-source changed during verification: {name}")
        if digest != entry["sha256"]:
            raise PreparationError(f"Extracted-source hash changed: {name}")
    required = {"Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "build-rust.sh",
                "rust/zed-engine.toml", "rust/vendor/zed/Cargo.toml"}
    if not required <= seen:
        raise PreparationError("Incomplete extracted-source build inputs")
    # Cargo/embedded asset globs can consume files absent from the inventory.
    # Never compile such inputs while presenting the archive as matching source.
    for directory in ["rust/src", "rust/resources", "rust/LICENSES", "rust/zed-patches", "rust/vendor/zed", "tools", ".cargo"]:
        start = PROJECT_ROOT / directory
        if not start.exists():
            continue
        if start.is_symlink():
            raise PreparationError("Build input directory cannot be a symlink")
        for parent, dirs, names in os.walk(start, followlinks=False):
            if directory == "tools":
                dirs[:] = [name for name in dirs if name != "__pycache__"]
            for name in dirs:
                if (Path(parent) / name).is_symlink():
                    raise PreparationError("Undeclared build directory symlink")
            for name in names:
                if name == ".DS_Store":
                    continue
                candidate = Path(parent) / name
                relative = candidate.relative_to(PROJECT_ROOT).as_posix()
                if relative not in seen:
                    raise PreparationError(f"Undeclared extracted build input: {relative}")
    if (PROJECT_ROOT / "build.rs").exists() and "build.rs" not in seen:
        raise PreparationError("Undeclared Cargo build script")
    patches = [{"name": p.name, "sha256": hashlib.sha256(p.read_bytes()).hexdigest()}
               for p in sorted(PATCH_ROOT.glob("*.patch"))]
    if manifest["upstream"].get("patches") != patches:
        raise PreparationError("Extracted-source patch identity changed")
    return manifest


def git(*args: str, cwd: Path | None = None, binary: bool = False):
    result = subprocess.run(
        ["git", "-c", "core.fsmonitor=false", "-c", "http.lowSpeedLimit=1024",
         "-c", "http.lowSpeedTime=60", *args],
        cwd=cwd if cwd is not None else SOURCE, capture_output=True, timeout=180,
    )
    if result.returncode:
        raise PreparationError(result.stderr.decode(errors="replace").strip())
    return result.stdout if binary else result.stdout.decode().strip()


def prepare(fetch: bool = False) -> dict:
    config = read_config(PROJECT_ROOT / "rust/zed-engine.toml")
    upstream = config["upstream"]
    if SOURCE.resolve() != SOURCE:
        raise PreparationError("Zed source and its parent directories cannot be symlinks")
    if not SOURCE.exists():
        if not fetch:
            raise PreparationError("Missing pinned source; run python3 tools/setup_zed_engine.py --fetch")
        SOURCE.parent.mkdir(parents=True, exist_ok=True)
        git("clone", "--filter=blob:none", "--depth", "1", "--no-checkout",
            "--branch", upstream["tag"], upstream["repository"], str(SOURCE),
            cwd=PROJECT_ROOT)
        # Check the immutable identity before populating the worktree.
        if git("rev-parse", "HEAD") != upstream["revision"]:
            raise PreparationError("Upstream tag moved; fetched checkout retained for inspection")
        git("checkout", "--detach", upstream["revision"])
    if not (SOURCE / ".git").exists() and (PROJECT_ROOT / "SOURCE-MANIFEST.json").exists():
        manifest = archive_manifest(config)
        return {"source": str(SOURCE), "revision": upstream["revision"],
                "patches": manifest["upstream"]["patches"], "archive": True}
    if Path(git("rev-parse", "--show-toplevel")).resolve() != SOURCE.resolve():
        raise PreparationError("Source is not its own Git checkout")
    if git("rev-parse", "HEAD") != upstream["revision"]:
        raise PreparationError("Existing Zed source is at another revision; it was preserved")
    if git("remote", "get-url", "origin").removesuffix(".git") != upstream["repository"]:
        raise PreparationError("Existing Zed source has a different origin; it was preserved")
    flags = git("ls-files", "-v", "-z", binary=True).split(b"\0")
    if any(entry and (entry[:1].islower() or entry[:1] in (b"S", b"s")) for entry in flags):
        raise PreparationError("Hidden Git index flags prevent source verification")
    if git("diff", "--cached", "--name-only"):
        raise PreparationError("Staged Zed changes exist; they were preserved")
    if git("ls-files", "--others"):
        raise PreparationError("Untracked Zed files exist; they were preserved")
    patches = sorted(PATCH_ROOT.glob("*.patch")) if PATCH_ROOT.exists() else []
    expected_paths: set[str] = set()
    for patch in patches:
        for line in patch.read_text().splitlines():
            if line.startswith("+++ b/"):
                expected_paths.add(line[6:])
    dirty = set(git("diff", "--name-only").splitlines())
    if dirty - expected_paths:
        raise PreparationError("Unrelated Zed changes exist; they were preserved")
    manifest = PATCH_ROOT / "files.sha256"
    if patches and not manifest.is_file():
        raise PreparationError("Missing integration patch file digests")
    digests = {}
    for line in manifest.read_text().splitlines() if manifest.exists() else []:
        digest, relative = line.split("  ", 1)
        candidate = SOURCE / relative
        if candidate.is_symlink() or not candidate.resolve().is_relative_to(SOURCE.resolve()):
            raise PreparationError("Integration patch digest escapes source checkout")
        digests[relative] = digest
    if set(digests) != expected_paths:
        raise PreparationError("Integration patch paths and digest manifest disagree")
    # Do not modify a checkout with extra edits even in a patch's target file.
    if any(hashlib.sha256((SOURCE / name).read_bytes()).hexdigest() != digests[name] for name in dirty):
        raise PreparationError("Modified Zed source does not match the integration patch; preserved")
    applied = []
    for patch in patches:
        try:
            git("apply", "--reverse", "--check", str(patch))
        except PreparationError:
            git("apply", "--check", str(patch))
            git("apply", str(patch))
        applied.append({"name": patch.name, "sha256": hashlib.sha256(patch.read_bytes()).hexdigest()})
    # Reverse checks alone could overlook extra modifications in a patched file.
    # Compare each complete patched file with the retained expected digest.
    for relative, digest in digests.items():
        candidate = SOURCE / relative
        if hashlib.sha256(candidate.read_bytes()).hexdigest() != digest:
            raise PreparationError(f"Extra changes in {relative}; existing file preserved")
    return {"source": str(SOURCE), "revision": upstream["revision"], "patches": applied}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fetch", action="store_true", help="allow initial pinned source download")
    args = parser.parse_args()
    try:
        result = prepare(args.fetch)
        print(f"Embedded Zed source verified: {result['revision']}; {len(result['patches'])} integration patch(es)")
        return 0
    except (PreparationError, OSError, subprocess.TimeoutExpired, ValueError, KeyError, TypeError) as error:
        print(f"Zed setup stopped: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
