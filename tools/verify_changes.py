#!/usr/bin/env python3
"""Run Changes regression checks in a caller-owned disposable scratch folder."""
import argparse
from pathlib import Path
import subprocess


def git(repo, *args):
    return subprocess.run(["/usr/bin/git", "-c", "core.hooksPath=/dev/null", "-c",
                           "commit.gpgsign=false", "-C", str(repo), *args], check=True,
                          capture_output=True, text=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--scratch", type=Path, required=True)
    parser.add_argument("--module-cache", type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    work = args.scratch / "changes-checks"
    work.mkdir()  # Refuse to reuse or overwrite someone else's test data.
    repo = work / "repo"
    repo.mkdir()
    git(repo, "init", "-q")
    git(repo, "config", "user.name", "Changes Check")
    git(repo, "config", "user.email", "changes-check@example.invalid")
    for name in ["tracked.txt", "staged.txt", 'space → "quote".txt', "rename-old.txt"]:
        (repo / name).write_text("original\n", encoding="utf-8")
    git(repo, "add", ".")
    git(repo, "commit", "-qm", "Fixture baseline")
    (repo / "tracked.txt").write_text("original\nunstaged-line\n", encoding="utf-8")
    (repo / "staged.txt").write_text("original\nstaged-line\n", encoding="utf-8")
    git(repo, "add", "staged.txt")
    (repo / 'space → "quote".txt').write_text("original\nunicode-line\n", encoding="utf-8")
    (repo / "untracked\nline.txt").write_text("new-file-line\n", encoding="utf-8")
    (repo / "broken-link").symlink_to("missing-target")
    git(repo, "mv", "rename-old.txt", "rename-new.txt")
    unborn = work / "unborn"
    unborn.mkdir()
    git(unborn, "init", "-q")
    (unborn / "first.txt").write_text("first-line\n", encoding="utf-8")
    git(unborn, "add", ".")
    (unborn / "first.txt").write_text("first-line\nafter-staging\n", encoding="utf-8")

    linked = work / "linked-worktree"
    git(repo, "worktree", "add", "-q", "-b", "fixture-linked", str(linked))
    (linked / "tracked.txt").write_text("original\nlinked-only-line\n", encoding="utf-8")
    git_pointer = linked / ".git"
    target = git_pointer.read_text().removeprefix("gitdir: ").strip()
    import os
    git_pointer.write_text("gitdir: " + os.path.relpath(target, linked) + "\n")

    history = work / "long-history"
    history.mkdir()
    git(history, "init", "-q")
    git(history, "config", "user.name", "History Check")
    git(history, "config", "user.email", "history-check@example.invalid")
    for index in range(45):
        git(history, "commit", "--allow-empty", "-qm", f"Fixture history {index}")

    source = "\n".join(path.read_text() for path in sorted((root / "Sources").glob("*.swift")) if path.name != "main.swift")
    source += "\n" + (root / "Sources/main.swift").read_text()
    source = source[:source.index("let app = NSApplication.shared")]
    checks = "\n".join((root / path).read_text() for path in ["tools/TelemetryChecks.swift", "tools/AppearanceChecks.swift", "tools/DockSizingChecks.swift", "tools/AgentWorkChecks.swift", "tools/WorkspaceChecks.swift", "tools/ChangesChecks.swift"])
    checks += "\nrunAgentWorkChecks()\nrunWorkspaceChecks()\n"
    combined = work / "main.swift"
    combined.write_text(source + "\n" + checks)
    binary = work / "changes-checks"
    subprocess.run(["swiftc", "-O", "-swift-version", "5", "-module-cache-path",
                    str(args.module_cache or args.scratch / "module-cache"), str(combined), "-o", str(binary)], check=True)
    subprocess.run([str(binary), str(repo), str(unborn), str(linked), str(history)], check=True)


if __name__ == "__main__":
    main()
