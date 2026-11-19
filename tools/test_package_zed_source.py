"""Small offline fixtures for Corresponding Source packaging safety.

Set AW_SOURCE_TEST_TMPDIR to an existing registered scratch directory to account
for test storage. No real upstream checkout, network or compiler is used.
"""
from __future__ import annotations

import contextlib
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest import mock

import package_zed_source as packaging
from zed_engine import PreparationError


class PackageTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(
            prefix="source-package-fixture-", dir=os.environ.get("AW_SOURCE_TEST_TMPDIR"))
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve() / "project"
        self.root.mkdir()
        self.output_root = Path(self.temp.name).resolve() / "artifacts"
        self.output_root.mkdir()
        self.output = self.output_root / "source.tar.gz"
        self.source = self.root / packaging.VENDOR_ROOT
        self.source.mkdir(parents=True)
        for relative in packaging.FIXED_FILES:
            self.write(relative, f"fixture: {relative}\n")
        for relative in packaging.SOURCE_DIRECTORIES:
            self.write(f"{relative}/fixture.txt", f"fixture: {relative}\n")
        self.write("rust/src/main.rs", "fn main() { /* Unicode: λ🙂 */ }\n")
        self.write("rust/vendor/zed/Cargo.toml", "upstream original\n")
        self.write("rust/vendor/zed/LICENSE-GPL", "public upstream license\n")
        self.write("rust/vendor/zed/crates/editor/src/lib.rs", "reviewed patched bytes\n")
        self.git("init", "--quiet")
        self.git("add", ".")
        self.git("commit", "--quiet", "-m", "offline fixture")
        self.revision = self.git("rev-parse", "HEAD").strip()
        self.prepare_calls = []

    def write(self, relative, text):
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
        return path

    def git(self, *arguments):
        environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
        environment.update({"GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
                            "GIT_TERMINAL_PROMPT": "0"})
        result = subprocess.run(
            ["git", "-c", f"core.hooksPath={os.devnull}", "-c", "core.fsmonitor=false",
             "-c", "commit.gpgsign=false", "-c", "user.name=Offline Fixture",
             "-c", "user.email=offline-fixture@example.invalid", "-C", str(self.source),
             *arguments], env=environment, capture_output=True, check=True, text=True, timeout=15)
        return result.stdout

    def prepare(self, fetch):
        self.prepare_calls.append(fetch)
        return {"source": str(self.source), "revision": self.revision,
                "patches": [{"name": "reviewed.patch", "sha256": "a" * 64}]}

    def package(self, **overrides):
        arguments = {"output": self.output, "output_root": self.output_root,
                     "project_root": self.root, "prepare_source": self.prepare}
        arguments.update(overrides)
        return packaging.package(**arguments)

    def record_inputs(self, path=None):
        return packaging.record_inputs(path or self.output_root / "inputs.json", self.output_root,
                                       project_root=self.root, prepare_source=self.prepare)

    def check_inputs(self, path=None):
        return packaging.check_inputs(path or self.output_root / "inputs.json", self.output_root,
                                      project_root=self.root, prepare_source=self.prepare)

    def assert_no_partials(self):
        self.assertEqual(list(self.output_root.glob(".*.partial")), [])

    def read_archive(self, path=None):
        with tarfile.open(path or self.output, "r:gz") as archive:
            members = archive.getmembers()
            self.assertTrue(all(member.isfile() for member in members))
            contents = {member.name.removeprefix(packaging.ARCHIVE_ROOT + "/"):
                        archive.extractfile(member).read() for member in members}
        manifest = json.loads(contents.pop(packaging.MANIFEST_NAME))
        return contents, manifest

    def test_allowlist_manifest_and_current_patched_bytes(self):
        self.write("AGENTS.md", "personal instructions")
        self.write("docs/verification/private.json", "private evidence")
        self.write("rust/src/target/compiled-output", "generated")
        self.write("rust/resources/__pycache__/cached.pyc", "generated")
        self.write("rust/vendor/zed/untracked-private.txt", "not source")
        self.write("rust/vendor/zed/AGENTS.md", "public upstream instructions")
        self.git("add", "AGENTS.md")
        result = self.package()
        self.assertEqual(self.prepare_calls, [False, False])
        contents, manifest = self.read_archive()
        self.assertEqual(contents["rust/vendor/zed/crates/editor/src/lib.rs"], b"reviewed patched bytes\n")
        self.assertIn("rust/vendor/zed/AGENTS.md", contents)
        self.assertNotIn("AGENTS.md", contents)
        self.assertFalse(any("private" in path or "target/" in path or "__pycache__" in path for path in contents))
        records = {record["path"]: record for record in manifest["files"]}
        self.assertEqual(set(records), set(contents))
        for path, content in contents.items():
            self.assertEqual(records[path]["sha256"], hashlib.sha256(content).hexdigest())
            self.assertEqual(records[path]["size"], len(content))
        self.assertEqual(manifest["upstream"]["revision"], self.revision)
        self.assertEqual(result["sha256"], hashlib.sha256(self.output.read_bytes()).hexdigest())
        self.assertEqual(result["file-count"], len(contents))
        self.assert_no_partials()

    def test_identical_inputs_produce_identical_archives(self):
        first = self.package()
        second = self.package(output=self.output_root / "second.tar.gz")
        self.assertEqual(first["sha256"], second["sha256"])
        self.assertEqual(self.output.read_bytes(), (self.output_root / "second.tar.gz").read_bytes())

    def test_prebuild_record_checks_after_archive_without_modifying_record(self):
        link = self.source / "crates/editor/LICENSE-GPL"
        link.symlink_to("../../LICENSE-GPL")
        self.git("add", ".")
        result = self.record_inputs()
        record = self.output_root / "inputs.json"
        record.chmod(0o400)
        original = record.read_bytes()
        self.package()
        checked = self.check_inputs()
        self.assertTrue(checked["inputs-unchanged"])
        self.assertEqual(result["state-sha256"], checked["state-sha256"])
        self.assertEqual(record.read_bytes(), original)
        state = json.loads(original)["input-state"]
        link_entry = next(entry for entry in state["files"] if entry["path"].endswith("crates/editor/LICENSE-GPL"))
        self.assertEqual(link_entry["link-chains"], [["crates/editor/LICENSE-GPL", "../../LICENSE-GPL"]])
        self.assertEqual(len(link_entry["identities"]), 2)
        self.assertEqual(len(link_entry["identities"][0]["identity"]), 6)
        self.assertIn("Cargo.toml", state["tracked-index"])
        self.assert_no_partials()

    def test_changed_source_rejects_prebuild_record(self):
        self.record_inputs()
        self.write("rust/src/main.rs", "changed during compilation")
        with self.assertRaisesRegex(PreparationError, "Source inputs changed"):
            self.check_inputs()

    def test_same_content_and_mtime_restored_during_build_is_rejected(self):
        path = self.root / "rust/src/main.rs"
        original = path.read_bytes()
        before = path.stat()
        self.record_inputs()
        path.write_bytes(b"temporary build input changes")
        path.write_bytes(original)
        os.utime(path, ns=(before.st_atime_ns, before.st_mtime_ns))
        self.assertEqual(path.read_bytes(), original)
        self.assertEqual(path.stat().st_mtime_ns, before.st_mtime_ns)
        self.assertNotEqual(path.stat().st_ctime_ns, before.st_ctime_ns)
        with self.assertRaisesRegex(PreparationError, "Source inputs changed"):
            self.check_inputs()

    def test_new_ignored_resource_is_still_a_changed_build_input(self):
        self.write("rust/resources/.gitignore", "generated-font.ttf\n")
        self.record_inputs()
        self.write("rust/resources/generated-font.ttf", "new compile-time asset")
        with self.assertRaisesRegex(PreparationError, "Source inputs changed"):
            self.check_inputs()

    def test_existing_readonly_record_is_never_overwritten(self):
        self.record_inputs()
        record = self.output_root / "inputs.json"
        record.chmod(0o400)
        original = record.read_bytes()
        self.prepare_calls.clear()
        with self.assertRaisesRegex(PreparationError, "already exists"):
            self.record_inputs()
        self.assertEqual(record.read_bytes(), original)
        self.assertEqual(self.prepare_calls, [])
        self.assert_no_partials()

    def test_input_record_digest_tampering_and_symlink_are_rejected(self):
        self.record_inputs()
        record = self.output_root / "inputs.json"
        stored = json.loads(record.read_bytes())
        stored["input-state"]["tracked-index"] = {}
        record.write_text(json.dumps(stored))
        with self.assertRaisesRegex(PreparationError, "digest is invalid"):
            self.check_inputs()
        alias = self.output_root / "alias.json"
        alias.symlink_to(record)
        with self.assertRaisesRegex(PreparationError, "regular file"):
            self.check_inputs(alias)

    def test_input_record_wrong_json_shape_fails_cleanly(self):
        record = self.output_root / "inputs.json"
        record.write_text("[]")
        with self.assertRaisesRegex(PreparationError, "schema or digest is invalid"):
            self.check_inputs()

    def test_input_record_publication_race_protects_other_process_file(self):
        record = self.output_root / "inputs.json"
        original = packaging.os.link
        def race(*args, **kwargs):
            record.write_bytes(b"another process record")
            return original(*args, **kwargs)
        with mock.patch.object(packaging.os, "link", side_effect=race), self.assertRaises(FileExistsError):
            self.record_inputs()
        self.assertEqual(record.read_bytes(), b"another process record")
        self.assert_no_partials()

    def test_input_record_paths_are_scoped_to_existing_output_root(self):
        with self.assertRaises(PreparationError):
            self.record_inputs(self.root / "inputs.json")
        with self.assertRaises(PreparationError):
            self.check_inputs(self.root / "inputs.json")
        self.assertEqual(self.prepare_calls, [])

    def test_cli_modes_are_mutually_exclusive(self):
        with mock.patch("sys.argv", ["package_zed_source.py", "--output", str(self.output),
                                    "--record-inputs", str(self.output_root / "inputs.json")]), \
                contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as raised:
            packaging.main()
        self.assertEqual(raised.exception.code, 2)

    def test_extracted_source_can_repackage_without_git_or_extra_inputs(self):
        link = self.source / "crates/editor/LICENSE-GPL"
        link.symlink_to("../../LICENSE-GPL")
        self.git("add", ".")
        original = self.package()
        contents, manifest = self.read_archive()
        extracted = Path(self.temp.name).resolve() / "extracted"
        for record in manifest["files"]:
            path = extracted / record["path"]
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(contents[record["path"]])
            path.chmod(int(record["mode"], 8))
        (extracted / packaging.MANIFEST_NAME).write_text(json.dumps(manifest))
        # Additional local files must never enter an archive inventory derived
        # from the validated, extracted source manifest.
        (extracted / "rust/src/unmanifested.rs").write_text("local addition")
        (extracted / "rust/vendor/zed/unmanifested.txt").write_text("local addition")
        output = self.output_root / "repackaged.tar.gz"
        def prepared(fetch):
            self.assertFalse(fetch)
            return {"source": str(extracted / packaging.VENDOR_ROOT),
                    "revision": self.revision, "patches": manifest["upstream"]["patches"],
                    "archive": True}
        with mock.patch.object(packaging.setup_zed_engine, "read_config", return_value={}), \
                mock.patch.object(packaging.setup_zed_engine, "archive_manifest", return_value=manifest, create=True), \
                mock.patch.object(packaging, "_git", side_effect=AssertionError("Extracted packaging ran Git")):
            input_record = self.output_root / "extracted-inputs.json"
            packaging.record_inputs(input_record, self.output_root, project_root=extracted,
                                    prepare_source=prepared)
            result = packaging.package(output, self.output_root, project_root=extracted,
                                       prepare_source=prepared)
            checked = packaging.check_inputs(input_record, self.output_root, project_root=extracted,
                                             prepare_source=prepared)
            self.assertTrue(checked["inputs-unchanged"])
        repackaged, repackaged_manifest = self.read_archive(output)
        self.assertFalse((extracted / packaging.VENDOR_ROOT / ".git").exists())
        self.assertEqual(repackaged, contents)
        self.assertEqual(repackaged_manifest, manifest)
        self.assertEqual(result["sha256"], original["sha256"])
        self.assert_no_partials()

    def test_existing_file_is_never_overwritten_or_prepared(self):
        self.output.write_bytes(b"keep existing")
        with self.assertRaises(PreparationError):
            self.package()
        self.assertEqual(self.output.read_bytes(), b"keep existing")
        self.assertEqual(self.prepare_calls, [])
        self.assert_no_partials()

    def test_dangling_output_symlink_is_preserved(self):
        self.output.symlink_to(self.output_root / "missing")
        with self.assertRaises(PreparationError):
            self.package()
        self.assertTrue(self.output.is_symlink())
        self.assertEqual(self.prepare_calls, [])

    def test_relative_escape_or_wrong_extension_output_is_rejected(self):
        for output in (Path("relative.tar.gz"), self.output_root / ".." / "escape.tar.gz",
                       self.root / "escape.tar.gz", self.output_root / "archive.zip"):
            with self.subTest(output=output), self.assertRaises(PreparationError):
                self.package(output=output)
        self.assertEqual(self.prepare_calls, [])

    def test_missing_output_root_is_not_created(self):
        missing = self.output_root / "missing"
        with self.assertRaises(OSError):
            self.package(output=missing / "source.tar.gz", output_root=missing)
        self.assertFalse(missing.exists())

    def test_output_directory_symlink_is_rejected(self):
        linked = Path(self.temp.name).resolve() / "linked-artifacts"
        linked.symlink_to(self.output_root, target_is_directory=True)
        with self.assertRaises(OSError):
            self.package(output=linked / "source.tar.gz", output_root=linked)
        self.assertFalse(self.output.exists())

    def test_output_inside_source_inputs_is_rejected(self):
        output_root = self.root / "rust/resources"
        with self.assertRaises(PreparationError):
            self.package(output=output_root / "source.tar.gz", output_root=output_root)
        self.assertEqual(self.prepare_calls, [])

    def test_failed_verification_never_allocates_archive(self):
        def rejected(fetch):
            self.assertFalse(fetch)
            raise PreparationError("wrong pinned revision")
        with self.assertRaises(PreparationError):
            self.package(prepare_source=rejected)
        self.assertFalse(self.output.exists())
        self.assert_no_partials()

    def test_source_path_must_be_the_verified_expected_checkout(self):
        def foreign(fetch):
            result = self.prepare(fetch)
            result["source"] = str(self.root)
            return result
        with self.assertRaises(PreparationError):
            self.package(prepare_source=foreign)
        self.assertFalse(self.output.exists())

    def test_missing_required_build_input_fails_closed(self):
        (self.root / "Cargo.lock").unlink()
        with self.assertRaises(OSError):
            self.package()
        self.assertFalse(self.output.exists())
        self.assert_no_partials()

    def test_root_source_file_and_directory_symlinks_are_rejected(self):
        source = self.root / "rust/src/main.rs"
        source.unlink()
        source.symlink_to(self.source / "Cargo.toml")
        with self.assertRaises(PreparationError):
            self.package()
        source.unlink()
        self.write("rust/src/main.rs", "restored fixture")
        (self.root / "rust/src/linked-directory").symlink_to(self.source, target_is_directory=True)
        with self.assertRaises(PreparationError):
            self.package()
        self.assertFalse(self.output.exists())

    def test_tracked_interior_symlink_chain_becomes_regular_bytes(self):
        first = self.source / "crates/editor/LICENSE-GPL"
        first.symlink_to("../../LICENSE-GPL")
        second = self.source / "LICENSE-ALIAS"
        second.symlink_to("crates/editor/LICENSE-GPL")
        self.git("add", ".")
        self.package()
        contents, manifest = self.read_archive()
        self.assertEqual(contents["rust/vendor/zed/LICENSE-ALIAS"], b"public upstream license\n")
        aliases = {item["path"]: item for item in manifest["materialized-symlinks"]}
        self.assertEqual(len(aliases["rust/vendor/zed/LICENSE-ALIAS"]["chain"]), 2)
        self.assertEqual(aliases["rust/vendor/zed/LICENSE-ALIAS"]["content_path"],
                         "rust/vendor/zed/LICENSE-GPL")

    def test_external_untracked_and_cyclic_tracked_symlinks_fail(self):
        link = self.source / "LINK"
        for target in ("../../../Cargo.toml", "/etc/passwd", "untracked", "LINK"):
            with self.subTest(target=target):
                if link.is_symlink():
                    link.unlink()
                link.symlink_to(target)
                self.git("add", "LINK")
                with self.assertRaises(PreparationError):
                    self.package()
                self.assertFalse(self.output.exists())
                self.assert_no_partials()

    def test_tracked_regular_file_replaced_with_symlink_fails(self):
        path = self.source / "Cargo.toml"
        path.unlink()
        path.symlink_to("LICENSE-GPL")
        with self.assertRaises(PreparationError):
            self.package()
        self.assertFalse(self.output.exists())

    def test_mutation_after_streaming_aborts_and_removes_only_own_partial(self):
        unrelated = self.output_root / ".unrelated.partial"
        unrelated.write_bytes(b"keep unrelated")
        original = packaging._write_entry
        def mutate(archive, root, entry):
            result = original(archive, root, entry)
            if entry.path == "Cargo.toml":
                (root / "Cargo.toml").write_text("changed concurrently")
            return result
        with mock.patch.object(packaging, "_write_entry", side_effect=mutate), self.assertRaises(PreparationError):
            self.package()
        self.assertFalse(self.output.exists())
        self.assertEqual(unrelated.read_bytes(), b"keep unrelated")
        self.assertEqual(list(self.output_root.glob(".*.partial")), [unrelated])

    def test_added_source_file_after_inventory_aborts(self):
        original = packaging._write_entry
        def add(archive, root, entry):
            result = original(archive, root, entry)
            if entry.path == "Cargo.toml":
                self.write("rust/src/new.rs", "new concurrent source")
            return result
        with mock.patch.object(packaging, "_write_entry", side_effect=add), self.assertRaises(PreparationError):
            self.package()
        self.assertFalse(self.output.exists())
        self.assert_no_partials()

    def test_publication_race_never_overwrites_other_output(self):
        original = packaging.os.link
        def race(*args, **kwargs):
            self.output.write_bytes(b"created by another process")
            return original(*args, **kwargs)
        with mock.patch.object(packaging.os, "link", side_effect=race), self.assertRaises(FileExistsError):
            self.package()
        self.assertEqual(self.output.read_bytes(), b"created by another process")
        self.assert_no_partials()

    def test_archive_write_failure_removes_own_partial(self):
        with mock.patch.object(packaging, "_write_entry", side_effect=OSError("disk full")), self.assertRaises(OSError):
            self.package()
        self.assertFalse(self.output.exists())
        self.assert_no_partials()

    def test_final_pin_or_patch_verification_change_aborts_publication(self):
        def changed(fetch):
            result = self.prepare(fetch)
            if len(self.prepare_calls) > 1:
                result["patches"] = []
            return result
        with self.assertRaises(PreparationError):
            self.package(prepare_source=changed)
        self.assertEqual(self.prepare_calls, [False, False])
        self.assertFalse(self.output.exists())
        self.assert_no_partials()

    def test_cli_requires_output_root_without_allocating(self):
        with mock.patch.dict(os.environ, {}, clear=True), mock.patch("sys.argv", [
                "package_zed_source.py", "--output", str(self.output)]), contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(packaging.main(), 1)
        self.assertFalse(self.output.exists())


if __name__ == "__main__":
    unittest.main()
