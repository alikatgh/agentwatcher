"""Real tiny Git fixtures; no downloads, toolchain changes or live source edits."""
import hashlib
import json
import stat
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import setup_zed_engine as setup


class SourcePreparationTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="zed-setup-test-")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name).resolve()
        self.source = self.root / "rust/vendor/zed"
        self.patches = self.root / "rust/zed-patches"
        self.source.mkdir(parents=True)
        self.patches.mkdir()
        self.file = self.source / "crates/language/src/buffer.rs"
        self.file.parent.mkdir(parents=True)
        self.original = "first\nsecond\nthird\n"
        self.file.write_text(self.original)
        self.git("init", "-q")
        self.git("add", ".")
        self.git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                 "-c", "commit.gpgsign=false", "commit", "-qm", "fixture")
        self.git("remote", "add", "origin", "https://github.com/zed-industries/zed")
        pin = self.git("rev-parse", "HEAD")
        config = (setup.PROJECT_ROOT / "rust/zed-engine.toml").read_text()
        old_pin = setup.read_config(setup.PROJECT_ROOT / "rust/zed-engine.toml")["upstream"]["revision"]
        (self.root / "rust/zed-engine.toml").write_text(config.replace(old_pin, pin))
        for name, value in [("PROJECT_ROOT", self.root), ("SOURCE", self.source), ("PATCH_ROOT", self.patches)]:
            context = patch.object(setup, name, value)
            context.start()
            self.addCleanup(context.stop)

    def git(self, *args, binary=False):
        return setup.git(*args, cwd=self.source, binary=binary)

    def make_patch(self):
        modified = self.original.replace("second", "bounded second")
        self.file.write_text(modified)
        (self.patches / "0001-policy.patch").write_bytes(self.git("diff", "--binary", binary=True))
        (self.patches / "files.sha256").write_text(
            hashlib.sha256(modified.encode()).hexdigest() + "  crates/language/src/buffer.rs\n"
        )
        self.file.write_text(self.original)
        return modified

    def test_exact_clean_checkout_passes_without_network(self):
        self.assertEqual(setup.prepare()["patches"], [])

    def test_reviewed_patch_is_applied_once_and_then_verified(self):
        expected = self.make_patch()
        self.assertEqual(len(setup.prepare()["patches"]), 1)
        self.assertEqual(self.file.read_text(), expected)
        setup.prepare()
        self.assertEqual(self.file.read_text(), expected)

    def test_extra_changes_in_patch_target_are_preserved_without_application(self):
        self.make_patch()
        extra = self.original + "user change\n"
        self.file.write_text(extra)
        with self.assertRaises(setup.PreparationError):
            setup.prepare()
        self.assertEqual(self.file.read_text(), extra)

    def test_modified_already_patched_file_is_preserved(self):
        self.make_patch()
        setup.prepare()
        changed = self.file.read_text() + "extra\n"
        self.file.write_text(changed)
        with self.assertRaises(setup.PreparationError):
            setup.prepare()
        self.assertEqual(self.file.read_text(), changed)

    def test_missing_digests_do_not_modify_source(self):
        self.make_patch()
        (self.patches / "files.sha256").unlink()
        with self.assertRaises(setup.PreparationError):
            setup.prepare()
        self.assertEqual(self.file.read_text(), self.original)

    def test_hidden_index_changes_rejected(self):
        for flag in ["--assume-unchanged", "--skip-worktree"]:
            self.git("update-index", flag, "crates/language/src/buffer.rs")
            with self.assertRaises(setup.PreparationError):
                setup.prepare()
            self.git("update-index", "--no-assume-unchanged", "--no-skip-worktree", "crates/language/src/buffer.rs")

    def test_staged_changes_preserved(self):
        self.file.write_text("staged\n")
        self.git("add", ".")
        with self.assertRaises(setup.PreparationError):
            setup.prepare()
        self.assertEqual(self.file.read_text(), "staged\n")

    def test_untracked_source_preserved(self):
        user_file = self.source / "notes.txt"
        user_file.write_text("user notes")
        with self.assertRaises(setup.PreparationError):
            setup.prepare()
        self.assertEqual(user_file.read_text(), "user notes")

    def test_foreign_origin_preserved(self):
        self.git("remote", "set-url", "origin", "https://example.invalid/other")
        with self.assertRaises(setup.PreparationError):
            setup.prepare()
        self.assertEqual(self.file.read_text(), self.original)

    def test_changed_pin_preserved(self):
        self.file.write_text("other revision\n")
        self.git("add", ".")
        self.git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
                 "-c", "commit.gpgsign=false", "commit", "-qm", "changed")
        with self.assertRaises(setup.PreparationError):
            setup.prepare()
        self.assertEqual(self.file.read_text(), "other revision\n")

    def test_digest_path_escape_rejected(self):
        self.make_patch()
        (self.patches / "files.sha256").write_text("0" * 64 + "  ../../outside.txt\n")
        with self.assertRaises(setup.PreparationError):
            setup.prepare()
        self.assertEqual(self.file.read_text(), self.original)

    def make_archive_manifest(self):
        names = ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "build-rust.sh",
                 "rust/zed-engine.toml", "rust/vendor/zed/Cargo.toml",
                 "rust/vendor/zed/crates/language/src/buffer.rs"]
        for name in names:
            candidate = self.root / name
            if not candidate.exists():
                candidate.write_text("fixture source\n")
        config = setup.read_config(self.root / "rust/zed-engine.toml")
        manifest = {"schema-version": 1, "archive-root": "AgentWatcher-source",
                    "upstream": {"revision": config["upstream"]["revision"], "patches": []},
                    "files": [{"path": name, "size": (self.root/name).stat().st_size,
                               "mode": f"{stat.S_IMODE((self.root/name).stat().st_mode):04o}",
                               "sha256": hashlib.sha256((self.root/name).read_bytes()).hexdigest()}
                              for name in names]}
        (self.root / "SOURCE-MANIFEST.json").write_text(json.dumps(manifest))
        (self.source / ".git").rename(self.root / "fixture-git-metadata")
        return manifest

    def test_extracted_source_build_needs_no_git_checkout(self):
        self.make_archive_manifest()
        self.assertTrue(setup.prepare()["archive"])

    def test_extracted_source_modification_is_rejected_and_preserved(self):
        self.make_archive_manifest()
        self.file.write_text("modified after extraction\n")
        with self.assertRaises(setup.PreparationError):
            setup.prepare()
        self.assertEqual(self.file.read_text(), "modified after extraction\n")

    def test_extracted_source_pin_drift_is_rejected(self):
        manifest = self.make_archive_manifest()
        manifest["upstream"]["revision"] = "0" * 40
        (self.root / "SOURCE-MANIFEST.json").write_text(json.dumps(manifest))
        with self.assertRaises(setup.PreparationError):
            setup.prepare()

    def test_extracted_source_duplicate_inventory_is_rejected(self):
        manifest = self.make_archive_manifest()
        manifest["files"].append(manifest["files"][0])
        (self.root / "SOURCE-MANIFEST.json").write_text(json.dumps(manifest))
        with self.assertRaises(setup.PreparationError):
            setup.prepare()

    def test_unmanifested_asset_cannot_enter_extracted_binary(self):
        self.make_archive_manifest()
        asset = self.source / "assets/prompts/private.md"
        asset.parent.mkdir(parents=True)
        asset.write_text("private content must not be bundled")
        with self.assertRaises(setup.PreparationError):
            setup.prepare()
        self.assertTrue(asset.is_file())

    def test_unmanifested_license_is_rejected_and_preserved(self):
        self.make_archive_manifest()
        license_file = self.root / "rust/LICENSES/private.txt"
        license_file.parent.mkdir()
        private_contents = "private content must not be bundled with licenses"
        license_file.write_text(private_contents)
        with self.assertRaisesRegex(setup.PreparationError, "Undeclared extracted build input: rust/LICENSES/private.txt"):
            setup.prepare()
        self.assertEqual(license_file.read_text(), private_contents)

    def test_ignored_untracked_source_is_still_rejected(self):
        exclude = self.source / ".git/info/exclude"
        with exclude.open("a") as stream:
            stream.write("\nprivate.md\n")
        (self.source / "private.md").write_text("private")
        with self.assertRaises(setup.PreparationError):
            setup.prepare()

    def test_extracted_source_symlink_is_never_followed(self):
        self.make_archive_manifest()
        self.file.unlink()
        self.file.symlink_to(self.root / "Cargo.lock")
        with self.assertRaises(OSError):
            setup.prepare()


if __name__ == "__main__":
    unittest.main()
