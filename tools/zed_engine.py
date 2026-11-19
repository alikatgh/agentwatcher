#!/usr/bin/env python3
"""Offline preparation for the actual Zed engine; never clone, install or build.

  python3 tools/zed_engine.py manifest
  python3 tools/zed_engine.py check --build-root /Volumes/Build/agentwatcher-zed \
      --reserve-gib CALLER_PLANNING_RESERVE

The reserve is a caller-selected planning allowance, not a measured minimum.
The full engine build's storage requirement has not yet been measured.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass, field
from decimal import Decimal, InvalidOperation
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

try:
    import tomllib
except ImportError:
    sys.exit("zed_engine.py requires Python 3.11 or newer (tomllib); no tools were installed.")


PROJECT_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CONFIG = PROJECT_ROOT / "rust" / "zed-engine.toml"
SHA = re.compile(r"[0-9a-f]{40}\Z")
NAME = re.compile(r"[A-Za-z0-9_-]+\Z")


class PreparationError(Exception):
    pass


def read_config(path: Path) -> dict:
    try:
        with path.open("rb") as stream:
            config = tomllib.load(stream)
        upstream = config["upstream"]
        dependencies = config["dependencies"]
        patches = config["patches"]
        if config["schema-version"] != 1:
            raise ValueError("unsupported schema-version")
        if upstream["repository"] != "https://github.com/zed-industries/zed":
            raise ValueError("upstream repository must identify zed-industries/zed")
        if not SHA.fullmatch(upstream["revision"]):
            raise ValueError("upstream revision must be a full lowercase Git commit")
        if not re.fullmatch(r"\d+\.\d+\.\d+", upstream["toolchain"]):
            raise ValueError("toolchain must be an exact release")
        if not re.fullmatch(r"v[0-9A-Za-z.+-]+", upstream["tag"]):
            raise ValueError("invalid upstream tag")
        if not isinstance(upstream["mandatory-macos-workspace-crates"], int):
            raise ValueError("mandatory macOS crate count must be an integer")
        required = {
            "gpui": "gpui", "gpui_platform": "gpui_platform", "zed_editor": "editor",
            "zed_language": "language", "zed_multi_buffer": "multi_buffer",
            "zed_settings": "settings", "zed_theme": "theme",
            "zed_theme_settings": "theme_settings", "zed_release_channel": "release_channel",
            "zed_util": "util",
        }
        for alias, package in required.items():
            if dependencies[alias]["package"] != package:
                raise ValueError(f"{alias} must name upstream package {package}")
        for alias, dependency in dependencies.items():
            if not NAME.fullmatch(alias) or not NAME.fullmatch(dependency["package"]):
                raise ValueError("invalid dependency alias/package")
            if dependency["default-features"] is not False:
                raise ValueError(f"{alias} must explicitly disable default features")
            if set(dependency) - {"package", "default-features", "features"}:
                raise ValueError(f"unexpected dependency fields for {alias}")
            features = dependency.get("features", [])
            if not isinstance(features, list) or not all(isinstance(f, str) for f in features):
                raise ValueError(f"invalid features for {alias}")
        if not {"font-kit", "runtime_shaders"} <= set(dependencies["gpui_platform"].get("features", [])):
            raise ValueError("gpui_platform requires font-kit and runtime_shaders")
        if "debug-embed" not in dependencies["zed_util"].get("features", []):
            raise ValueError("zed_util/debug-embed is required for pinned debug assets")
        if set(patches) != {"tree-sitter-language", "async-task", "async-process", "notify", "notify-types"}:
            raise ValueError("expected the five researched macOS root patches")
        for name, patch in patches.items():
            if set(patch) != {"git", "rev"} or not SHA.fullmatch(patch["rev"]):
                raise ValueError(f"{name} requires an exact Git revision")
            if not re.fullmatch(r"https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", patch["git"]):
                raise ValueError(f"invalid public Git source for {name}")
        return config
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise PreparationError(f"invalid engine configuration: {error}") from error


def proposed_manifest(config: dict) -> str:
    upstream = config["upstream"]
    lines = [
        "# Proposed ROOT Cargo.toml fragment; merge dependencies rather than duplicate tables.",
        "# Source-supported pins only: Cargo resolution, compilation and runtime remain unverified.",
        f"# Zed {upstream['tag']} at {upstream['revision']}; Rust {upstream['toolchain']}.",
        "# All engine crates use one Cargo source. Do not retain the crates.io GPUI dependency.",
        "[dependencies]",
    ]
    for alias, dependency in config["dependencies"].items():
        values = {"package": dependency["package"], "git": upstream["repository"],
                  "rev": upstream["revision"], "default-features": False}
        if dependency.get("features"):
            values["features"] = dependency["features"]
        fields = ", ".join(f"{key} = {json.dumps(value)}" for key, value in values.items())
        lines.append(f"{alias} = {{ {fields} }}")
    lines.extend(["", "[patch.crates-io]"])
    for name, patch in config["patches"].items():
        fields = ", ".join(f"{key} = {json.dumps(value)}" for key, value in patch.items())
        lines.append(f"{name} = {{ {fields} }}")
    result = "\n".join(lines) + "\n"
    # Keep emitted TOML executable even if quoting/feature generation changes.
    parsed = tomllib.loads(result)
    if set(parsed["dependencies"]) != set(config["dependencies"]):
        raise PreparationError("generated dependency fragment failed validation")
    return result


def write_new_output(output: Path, build_root: Path | None, content: str) -> None:
    if build_root is None:
        raise PreparationError("--build-root is required to write output; stdout needs no allocation")
    if not build_root.is_absolute() or not output.is_absolute():
        raise PreparationError("--build-root and --output must be explicit absolute paths")
    root = build_root.resolve()
    if not root.is_dir():
        raise PreparationError("build root must already exist; no directories are created")
    destination = output.resolve()
    if destination == root or not destination.is_relative_to(root):
        raise PreparationError("output must be inside the explicitly selected build root")
    if destination.is_relative_to(PROJECT_ROOT):
        raise PreparationError("proposed output must be outside the existing project")
    if output.is_symlink():
        raise PreparationError("output cannot be a symlink")
    try:
        # Exclusive creation also protects existing files from a check/write race.
        with output.open("x", encoding="utf-8") as stream:
            stream.write(content)
    except OSError as error:
        raise PreparationError(f"could not create new output (existing files are protected): {error}") from error


def run_readonly(command: list[str], env: dict | None = None) -> subprocess.CompletedProcess:
    environment = os.environ.copy()
    environment.update({"RUSTUP_AUTO_INSTALL": "0", "GIT_TERMINAL_PROMPT": "0", "GIT_OPTIONAL_LOCKS": "0"})
    if env:
        environment.update(env)
    try:
        result = subprocess.run(command, text=True, capture_output=True, env=environment, timeout=15)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise PreparationError(f"could not run local {Path(command[0]).name} probe: {type(error).__name__}") from error
    # The complete pinned source index is larger than a short status response.
    # Bound the read while allowing a full checkout's tracked-path inventory.
    if len(result.stdout) > 8 * 1024 * 1024:
        raise PreparationError("local probe returned too much output; inspection incomplete")
    return result


def source_identity(source: Path, config: dict, runner=run_readonly) -> list[str]:
    errors = []
    if not source.is_dir():
        return [f"source checkout is missing: {source}"]
    # Disable user fsmonitor helpers; these probes never contact a remote.
    prefix = ["git", "-c", "core.fsmonitor=false", "-C", str(source)]

    def git(*args):
        result = runner(prefix + list(args))
        if result.returncode:
            raise PreparationError("source Git identity probe failed")
        return result.stdout.strip()

    try:
        if Path(git("rev-parse", "--show-toplevel")).resolve() != source.resolve():
            return ["source path is not the checkout root"]
        if git("rev-parse", "HEAD") != config["upstream"]["revision"]:
            errors.append("source HEAD does not match the exact configured Zed revision")
        origin = git("config", "--get", "remote.origin.url").removesuffix(".git").rstrip("/")
        if origin not in {config["upstream"]["repository"], "git@github.com:zed-industries/zed"}:
            errors.append("source origin does not identify zed-industries/zed")
        if git("status", "--porcelain", "--untracked-files=normal"):
            errors.append("source checkout has changes; pin identity alone does not describe its contents")
        if errors:
            return errors
        # `status` and `diff` trust these index flags and can report clean after
        # tracked files change. Reject them without altering the user's index.
        # With `ls-files -v`, lowercase tags mean assume-unchanged; S/s means
        # skip-worktree. NUL delimiters also handle paths containing newlines.
        index_entries = git("ls-files", "-v", "-z").split("\0")
        if any(entry and (entry[0].islower() or entry[0].upper() == "S") for entry in index_entries):
            return ["source index hides tracked changes (assume-unchanged or skip-worktree); pin contents cannot be verified"]
        toolchain = tomllib.loads((source / "rust-toolchain.toml").read_text())
        if toolchain["toolchain"]["channel"] != config["upstream"]["toolchain"]:
            errors.append("source-declared toolchain does not match configuration")
        workspace = tomllib.loads((source / "Cargo.toml").read_text())
        upstream_patches = workspace.get("patch", {}).get("crates-io", {})
        for name, expected in config["patches"].items():
            actual = upstream_patches.get(name, {})
            if any(actual.get(key) != value for key, value in expected.items()):
                errors.append(f"source root patch differs for {name}")
        for alias, dependency in config["dependencies"].items():
            manifest_path = source / "crates" / dependency["package"] / "Cargo.toml"
            manifest = tomllib.loads(manifest_path.read_text())
            if manifest["package"]["name"] != dependency["package"]:
                errors.append(f"source package identity differs for {alias}")
            for feature in dependency.get("features", []):
                if feature not in manifest.get("features", {}):
                    errors.append(f"source feature missing: {alias}/{feature}")
        # Native shaders read sibling GPUI files; embedded settings must retain
        # their upstream root layout. Do not accept a hand-extracted editor tree.
        for relative in ("assets/settings/default.json", "crates/gpui_apple/build.rs",
                         "crates/gpui_apple/src/shaders.metal", "crates/gpui/src/scene.rs"):
            if not (source / relative).is_file():
                errors.append(f"required source layout file is missing: {relative}")
    except (PreparationError, OSError, ValueError, KeyError, TypeError) as error:
        errors.append(f"source validation incomplete: {error}")
    return errors


def installed_toolchain(config: dict, rustup: str | None, toolchain_root: Path | None,
                        runner=run_readonly) -> tuple[list[str], list[str]]:
    if not rustup:
        return [], ["rustup was not found; use --rustup with an existing executable"]
    environment = {"RUSTUP_HOME": str(toolchain_root)} if toolchain_root else None
    try:
        listing = runner([rustup, "toolchain", "list"], environment)
        if listing.returncode:
            return [], ["installed toolchain listing failed"]
        names = [line.split()[0] for line in listing.stdout.splitlines() if line.strip()]
        required = config["upstream"]["toolchain"]
        selected = next((name for name in names if name == required or name.startswith(required + "-")), None)
        # Report the actual compiler behind a moving alias such as "stable".
        # Only probe names explicitly returned as installed; never invoke a
        # missing version through a rustup proxy or trigger its auto-install.
        if selected is None:
            if names:
                location = runner([rustup, "which", "--toolchain", names[0], "rustc"], environment)
                compiler = Path(location.stdout.strip())
                if not location.returncode and compiler.is_absolute() and compiler.is_file():
                    version = runner([str(compiler), "--version"], environment)
                    match = re.match(r"rustc ([0-9.]+) ", version.stdout)
                    if not version.returncode and match:
                        names[0] += f" (rustc {match.group(1)})"
            return names, [f"required named Rust {required} toolchain is not installed (no installation attempted)"]
        location = runner([rustup, "which", "--toolchain", selected, "rustc"], environment)
        compiler = Path(location.stdout.strip())
        if location.returncode or not compiler.is_absolute() or not compiler.is_file():
            return names, ["installed toolchain compiler path is unavailable"]
        version = runner([str(compiler), "--version"], environment)
        if version.returncode or not version.stdout.startswith(f"rustc {required} "):
            return names, ["installed compiler version does not match the configured toolchain"]
        return names, []
    except PreparationError as error:
        return [], [str(error)]


@dataclass
class CheckResult:
    observations: list[str] = field(default_factory=list)
    blockers: list[str] = field(default_factory=list)


def check(config: dict, build_root: Path | None, source: Path | None,
          reserve_bytes: int | None, rustup: str | None, toolchain_root: Path | None,
          runner=run_readonly, disk_usage=shutil.disk_usage) -> CheckResult:
    result = CheckResult()
    upstream = config["upstream"]
    result.observations.append(f"Configured Zed: {upstream['tag']} at {upstream['revision']}")
    if build_root is None:
        disk_path = PROJECT_ROOT
        result.blockers.append("no custom --build-root selected for future source/cache/target allocations")
    else:
        disk_path = build_root.resolve()
        if not build_root.is_absolute() or not disk_path.is_dir():
            result.blockers.append("build root must be an existing absolute directory")
        if disk_path == PROJECT_ROOT or disk_path.is_relative_to(PROJECT_ROOT):
            result.blockers.append("select a build root outside the existing project")
        result.observations.extend([
            f"Build root: {disk_path}", f"Source path: {source or disk_path / 'source' / 'zed'}",
            f"Future CARGO_HOME: {disk_path / 'cargo-home'}",
            f"Future CARGO_TARGET_DIR: {disk_path / 'target'}",
            f"Future RUSTUP_HOME (if installing an isolated toolchain): {disk_path / 'rustup-home'}",
        ])
        source = source or disk_path / "source" / "zed"
    if source is None:
        result.blockers.append("no Zed source checkout selected (--source or --build-root)")
    else:
        source_errors = source_identity(source, config, runner)
        result.blockers.extend(source_errors)
        if not source_errors:
            result.observations.append("Source identity, direct packages, features, layout and root patches: verified")
    names, toolchain_errors = installed_toolchain(config, rustup, toolchain_root, runner)
    result.observations.append("Installed toolchains: " + (", ".join(names) or "none found"))
    result.blockers.extend(toolchain_errors)
    probe_path = disk_path
    while not probe_path.exists() and probe_path != probe_path.parent:
        probe_path = probe_path.parent
    try:
        available = disk_usage(probe_path).free
        result.observations.append(f"Available on {probe_path}: {available} bytes ({available / 2**30:.2f} GiB)")
        if reserve_bytes is None:
            result.blockers.append("no --reserve-bytes/--reserve-gib supplied; storage readiness is unassessed")
        elif reserve_bytes <= 0:
            result.blockers.append("planning reserve must be positive")
        else:
            result.observations.append(f"Caller planning reserve: {reserve_bytes} bytes ({reserve_bytes / 2**30:.2f} GiB)")
            if available < reserve_bytes:
                result.blockers.append("available storage is below the caller's planning reserve")
    except OSError:
        result.blockers.append("disk headroom could not be measured")
    result.observations.append("Engine build minimum is unknown; reserve is not a measured minimum or a build guarantee.")
    return result


def positive_bytes(value: str) -> int:
    try:
        number = int(value)
        if number <= 0:
            raise ValueError
        return number
    except ValueError:
        raise argparse.ArgumentTypeError("expected a positive byte count")


def gib_bytes(value: str) -> int:
    try:
        number = Decimal(value)
        if not number.is_finite() or number <= 0:
            raise ValueError
        result = int(number * (2**30))
        if result <= 0:
            raise ValueError
        return result
    except (InvalidOperation, ValueError, OverflowError):
        raise argparse.ArgumentTypeError("expected a positive finite GiB amount")


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
    subcommands = parser.add_subparsers(dest="command", required=True)
    inspect = subcommands.add_parser("check", help="read-only local prerequisite checks; no downloads/builds")
    inspect.add_argument("--build-root", type=Path)
    inspect.add_argument("--source", type=Path)
    inspect.add_argument("--toolchain-root", type=Path, help="existing RUSTUP_HOME to inspect")
    default_rustup = shutil.which("rustup")
    if not default_rustup and (Path.home() / ".cargo/bin/rustup").is_file():
        default_rustup = str(Path.home() / ".cargo/bin/rustup")
    inspect.add_argument("--rustup", default=default_rustup, help="existing rustup executable")
    reserve = inspect.add_mutually_exclusive_group()
    reserve.add_argument("--reserve-bytes", type=positive_bytes)
    reserve.add_argument("--reserve-gib", type=gib_bytes)
    emit = subcommands.add_parser("manifest", help="emit a proposed root Cargo fragment without activating it")
    emit.add_argument("--build-root", type=Path, help="required for output files; existing custom directory")
    emit.add_argument("--output", type=Path, help="new absolute file inside build root; default stdout")
    args = parser.parse_args(argv)
    try:
        config = read_config(args.config)
        if args.command == "manifest":
            content = proposed_manifest(config)
            if args.output:
                write_new_output(args.output, args.build_root, content)
                print(f"Created proposed fragment: {args.output}")
            else:
                print(content, end="")
            return 0
        result = check(config, args.build_root, args.source,
                       args.reserve_bytes if args.reserve_bytes is not None else args.reserve_gib,
                       args.rustup, args.toolchain_root)
        for observation in result.observations:
            print(observation)
        for blocker in result.blockers:
            print(f"BLOCKED: {blocker}")
        if not result.blockers:
            print("Local prerequisites passed only; Cargo resolution/build/runtime integration remain pending.")
        return 1 if result.blockers else 0
    except PreparationError as error:
        print(f"BLOCKED: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
