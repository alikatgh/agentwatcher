"""Failure-path coverage for offline Zed preparation, with no network/builds."""

import contextlib
import importlib.util
import io
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import types
import unittest


SPEC = importlib.util.spec_from_file_location("zed_engine", Path(__file__).with_name("zed_engine.py"))
engine = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = engine
SPEC.loader.exec_module(engine)


class OfflineFixture(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="agentwatcher-zed-preparation-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source = self.root / "source" / "zed"
        self.source.mkdir(parents=True)
        self.config = engine.read_config(engine.DEFAULT_CONFIG)
        self.compiler = self.root / "existing-toolchain" / "bin" / "rustc"
        self.compiler.parent.mkdir(parents=True)
        self.compiler.touch()
        self.head = self.config["upstream"]["revision"]
        self.origin = self.config["upstream"]["repository"]
        self.dirty = ""
        self.installed = self.config["upstream"]["toolchain"] + "-aarch64-apple-darwin"
        self.commands = []
        self.free = 16 * 2**30
        self.reserve = 8 * 2**30
        upstream = self.config["upstream"]
        (self.source / "rust-toolchain.toml").write_text(
            f'[toolchain]\nchannel = "{upstream["toolchain"]}"\n')
        patches = []
        for name, patch in self.config["patches"].items():
            patches.append(f'[patch.crates-io.{name}]\ngit = "{patch["git"]}"\nrev = "{patch["rev"]}"\n')
        (self.source / "Cargo.toml").write_text("\n".join(patches))
        for dependency in self.config["dependencies"].values():
            package = dependency["package"]
            directory = self.source / "crates" / package
            directory.mkdir(parents=True)
            features = "\n".join(f'"{name}" = []' for name in dependency.get("features", []))
            (directory / "Cargo.toml").write_text(f'[package]\nname = "{package}"\n[features]\n{features}\n')
        for relative in ("assets/settings/default.json", "crates/gpui_apple/build.rs",
                         "crates/gpui_apple/src/shaders.metal", "crates/gpui/src/scene.rs"):
            path = self.source / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.touch()

    def runner(self, command, env=None):
        self.commands.append((command, env))
        tail = command[command.index(str(self.source)) + 1:] if str(self.source) in command else command[1:]
        if command[0] == "git":
            outputs = {
                ("rev-parse", "--show-toplevel"): str(self.source),
                ("rev-parse", "HEAD"): self.head,
                ("config", "--get", "remote.origin.url"): self.origin,
                ("status", "--porcelain", "--untracked-files=normal"): self.dirty,
                ("ls-files", "-v", "-z"): "H crates/editor/Cargo.toml\0",
            }
            value = outputs[tuple(tail)]
        elif tail == ["toolchain", "list"]:
            value = self.installed
        elif tail[:1] == ["which"]:
            value = str(self.compiler)
        elif command == [str(self.compiler), "--version"]:
            value = f'rustc {self.config["upstream"]["toolchain"]} (fake offline test)'
        else:
            self.fail(f"unexpected command (could allocate or access network): {command}")
        return subprocess.CompletedProcess(command, 0, value + "\n", "")

    def inspect(self, **overrides):
        arguments = dict(config=self.config, build_root=self.root, source=self.source,
                         reserve_bytes=self.reserve, rustup="existing-rustup",
                         toolchain_root=None, runner=self.runner,
                         disk_usage=lambda path: types.SimpleNamespace(free=self.free))
        arguments.update(overrides)
        return engine.check(**arguments)


class CheckTests(OfflineFixture):
    def test_valid_prerequisites_do_not_allocate_or_claim_integration(self):
        before = set(self.root.rglob("*"))
        result = self.inspect()
        self.assertEqual(result.blockers, [])
        self.assertEqual(set(self.root.rglob("*")), before)
        self.assertTrue(any("minimum is unknown" in message for message in result.observations))
        self.assertTrue(self.commands)
        self.assertFalse(any("install" in command for command, _ in self.commands))

    def test_missing_toolchain_never_requests_missing_version_or_install(self):
        self.installed = "1.94.0-aarch64-apple-darwin (default)"
        result = self.inspect()
        self.assertTrue(any("not installed" in message for message in result.blockers))
        self.assertFalse(any("install" in command for command, _ in self.commands))
        selections = [command[command.index("--toolchain") + 1] for command, _ in self.commands if "--toolchain" in command]
        self.assertEqual(selections, ["1.94.0-aarch64-apple-darwin"])

    def test_wrong_pin_blocks_before_source_file_reads(self):
        self.head = "b" * 40
        (self.source / "rust-toolchain.toml").unlink()
        result = self.inspect()
        self.assertTrue(any("HEAD does not match" in message for message in result.blockers))
        self.assertFalse(any("validation incomplete" in message for message in result.blockers))

    def test_foreign_source_origin_and_dirty_checkout_block(self):
        self.origin = "https://example.invalid/foreign"
        self.dirty = " M crates/editor/Cargo.toml"
        result = self.inspect()
        self.assertTrue(any("origin does not identify" in message for message in result.blockers))
        self.assertTrue(any("checkout has changes" in message for message in result.blockers))

    def test_source_package_identity_is_checked(self):
        (self.source / "crates/editor/Cargo.toml").write_text('[package]\nname = "fake-editor"\n')
        result = self.inspect()
        self.assertTrue(any("package identity differs for zed_editor" in message for message in result.blockers))

    def test_source_patch_drift_blocks(self):
        manifest = self.source / "Cargo.toml"
        contents = manifest.read_text().replace(self.config["patches"]["async-task"]["rev"], "c" * 40)
        manifest.write_text(contents)
        result = self.inspect()
        self.assertTrue(any("root patch differs for async-task" in message for message in result.blockers))

    def test_missing_debug_embed_feature_blocks(self):
        (self.source / "crates/util/Cargo.toml").write_text('[package]\nname = "util"\n')
        result = self.inspect()
        self.assertTrue(any("zed_util/debug-embed" in message for message in result.blockers))

    def test_insufficient_free_space_and_unknown_reserve_cannot_pass(self):
        self.free = self.reserve - 1
        self.assertTrue(any("below" in message for message in self.inspect().blockers))
        self.assertTrue(any("unassessed" in message for message in self.inspect(reserve_bytes=None).blockers))

    def test_source_root_must_be_actual_checkout_root(self):
        original_runner = self.runner

        def different_root(command, env=None):
            if command[-2:] == ["rev-parse", "--show-toplevel"]:
                return subprocess.CompletedProcess(command, 0, str(self.source.parent), "")
            return original_runner(command, env)

        result = self.inspect(runner=different_root)
        self.assertTrue(any("not the checkout root" in message for message in result.blockers))

    def test_custom_toolchain_root_is_scoped_without_home_override(self):
        toolchain_root = self.root / "existing-toolchain"
        result = self.inspect(toolchain_root=toolchain_root)
        self.assertEqual(result.blockers, [])
        rust_commands = [(command, env) for command, env in self.commands if command[0] != "git"]
        self.assertTrue(rust_commands)
        for _, environment in rust_commands:
            self.assertEqual(environment, {"RUSTUP_HOME": str(toolchain_root)})


class SourceIndexFlagTests(OfflineFixture):
    def git(self, *arguments):
        # All mutation is confined to this test-owned temporary repository.
        environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
        environment.update({"GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
                            "GIT_TERMINAL_PROMPT": "0"})
        command = ["git", "-c", f"core.hooksPath={os.devnull}", "-c", "core.fsmonitor=false",
                   "-c", "commit.gpgsign=false", "-c", "user.name=Offline Test",
                   "-c", "user.email=offline-test@example.invalid", "-C", str(self.source),
                   *arguments]
        return subprocess.run(command, check=True, capture_output=True, text=True,
                              env=environment, timeout=15).stdout

    def assert_hidden_shader_change_rejected(self, flag):
        self.git("init", "--quiet")
        self.git("remote", "add", "origin", self.origin)
        self.git("add", ".")
        self.git("commit", "--quiet", "-m", "Offline source identity fixture")
        revision = self.git("rev-parse", "HEAD").strip()
        self.config["upstream"]["revision"] = revision
        relative = "crates/gpui_apple/src/shaders.metal"
        self.git("update-index", flag, relative)
        shader = self.source / relative
        shader.write_text("modified shader hidden by index flag\n")
        self.assertEqual(self.git("status", "--porcelain", "--untracked-files=normal"), "")
        original_flags = self.git("ls-files", "-v", "--", relative)

        errors = engine.source_identity(self.source, self.config)

        self.assertTrue(any("index hides tracked changes" in message for message in errors), errors)
        self.assertEqual(self.git("ls-files", "-v", "--", relative), original_flags)
        self.assertEqual(shader.read_text(), "modified shader hidden by index flag\n")
        self.assertEqual(self.git("rev-parse", "HEAD").strip(), revision)

    def test_assume_unchanged_cannot_hide_modified_tracked_shader(self):
        self.assert_hidden_shader_change_rejected("--assume-unchanged")

    def test_skip_worktree_cannot_hide_modified_tracked_shader(self):
        self.assert_hidden_shader_change_rejected("--skip-worktree")


class ManifestTests(OfflineFixture):
    def test_manifest_is_proposed_pinned_git_source_with_root_patches(self):
        manifest = engine.tomllib.loads(engine.proposed_manifest(self.config))
        sources = {(dep["git"], dep["rev"]) for dep in manifest["dependencies"].values()}
        self.assertEqual(sources, {(self.origin, self.head)})
        self.assertEqual(manifest["patch"]["crates-io"], self.config["patches"])
        self.assertIn("debug-embed", manifest["dependencies"]["zed_util"]["features"])

    def test_existing_output_is_never_overwritten(self):
        output = self.root / "proposal.toml"
        output.write_bytes(b"user-owned existing contents\n")
        with self.assertRaises(engine.PreparationError):
            engine.write_new_output(output, self.root, "replacement")
        self.assertEqual(output.read_bytes(), b"user-owned existing contents\n")

    def test_output_requires_explicit_build_root_and_containment(self):
        output = self.root / "proposal.toml"
        with self.assertRaises(engine.PreparationError):
            engine.write_new_output(output, None, "contents")
        with self.assertRaises(engine.PreparationError):
            engine.write_new_output(output, self.source, "contents")
        self.assertFalse(output.exists())

    def test_output_symlink_is_rejected_and_target_preserved(self):
        target = self.root / "original.toml"
        target.write_text("original")
        link = self.root / "proposal.toml"
        link.symlink_to(target)
        with self.assertRaises(engine.PreparationError):
            engine.write_new_output(link, self.root, "replacement")
        self.assertEqual(target.read_text(), "original")

    def test_stdout_manifest_needs_no_build_root_or_source_and_does_not_allocate(self):
        before = set(self.root.rglob("*"))
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            code = engine.main(["manifest"])
        self.assertEqual(code, 0)
        self.assertIn("[patch.crates-io]", output.getvalue())
        self.assertEqual(set(self.root.rglob("*")), before)

    def test_invalid_config_pin_and_missing_asset_feature_reject_emission(self):
        for old, new in ((self.head, "main"), ('features = ["debug-embed"]', 'features = []')):
            path = self.root / "bad-config.toml"
            path.write_text(engine.DEFAULT_CONFIG.read_text().replace(old, new))
            with self.assertRaises(engine.PreparationError):
                engine.read_config(path)


class ReserveTests(unittest.TestCase):
    def test_fractional_gib_and_invalid_reserves(self):
        self.assertEqual(engine.gib_bytes("0.5"), 2**29)
        for value in ("0", "-1", "nan", "inf", "not-a-number"):
            with self.assertRaises(engine.argparse.ArgumentTypeError):
                engine.gib_bytes(value)
        for value in ("0", "-1", "1.5"):
            with self.assertRaises(engine.argparse.ArgumentTypeError):
                engine.positive_bytes(value)


if __name__ == "__main__":
    unittest.main()
