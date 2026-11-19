"""Offline macOS signing regression; no Rust build or source packaging is run.

Set AW_SOURCE_TEST_TMPDIR to registered session scratch for storage accounting.
"""
from __future__ import annotations

import hashlib
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import unittest


@unittest.skipUnless(sys.platform == "darwin", "macOS codesign and xattr are required")
class BundleSigningTests(unittest.TestCase):
    def test_generated_bundle_metadata_is_removed_without_changing_inputs(self):
        script = (Path(__file__).resolve().parents[1] / "build-rust.sh").read_text()
        start = script.index('mkdir -p "$AW_APP/Contents/MacOS"')
        end = script.index("ditto -c", start)
        bundle_commands = script[start:end]
        with tempfile.TemporaryDirectory(
                prefix="bundle-signing-fixture-", dir=os.environ.get("AW_SOURCE_TEST_TMPDIR")) as temporary:
            root = Path(temporary)
            target = root / "target/debug/agentwatcher"
            target.parent.mkdir(parents=True)
            shutil.copyfile("/usr/bin/true", target)
            target.chmod(0o755)
            resources = {
                "rust/resources/AppIcon.icns": b"offline icon fixture",
                "rust/NOTICE": b"offline notice fixture",
                "rust/LICENSES/LICENSE.txt": b"offline license fixture",
                "rust/resources/themes/theme.json": b"{}",
                "rust/resources/fonts/manifest.json": b"{}",
            }
            for relative, content in resources.items():
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(content)
            info = root / "rust/Info.plist"
            info.write_bytes(plistlib.dumps({
                "CFBundleExecutable": "agentwatcher",
                "CFBundleIdentifier": "invalid.offline.agentwatcher.signing-fixture",
                "CFBundlePackageType": "APPL",
                "CFBundleVersion": "1",
            }))
            finder_info = "0000000000000000200000000000000000000000000000000000000000000000"
            icon = root / "rust/resources/AppIcon.icns"
            notice = root / "rust/NOTICE"
            self.run_command("xattr", "-wx", "com.apple.FinderInfo", finder_info, str(icon))
            self.run_command("xattr", "-w", "com.apple.ResourceFork", "fixture resource fork", str(notice))
            inputs = [target, info, *(root / relative for relative in resources)]
            before = self.input_state(inputs)
            app = root / "artifacts/AgentWatcher.app"
            (app / "Contents/MacOS").mkdir(parents=True)
            shutil.copyfile(target, app / "Contents/MacOS/agentwatcher")
            (app / "Contents/MacOS/agentwatcher").chmod(0o755)
            shutil.copyfile(info, app / "Contents/Info.plist")
            self.run_command("xattr", "-wx", "com.apple.FinderInfo", finder_info, str(app))
            reproduced = subprocess.run(
                ["codesign", "--dryrun", "--force", "--deep", "--sign", "-", str(app)],
                capture_output=True, text=True, timeout=30)
            self.assertNotEqual(reproduced.returncode, 0)
            self.assertIn("detritus not allowed", reproduced.stderr)
            environment = dict(os.environ, AW_APP=str(app), AW_TARGET_DIR=str(root / "target"),
                               AW_BINARY_PROFILE="debug")
            signed = subprocess.run(
                ["bash", "-euo", "pipefail", "-c", bundle_commands], cwd=root,
                env=environment, capture_output=True, text=True, timeout=30)
            self.assertEqual(signed.returncode, 0, signed.stdout + signed.stderr)
            self.run_command("codesign", "--verify", "--deep", "--strict", str(app))
            self.assertEqual(self.input_state(inputs), before)
            self.assertNotIn("com.apple.FinderInfo", self.run_command("xattr", "-r", str(app)))
            self.assertNotIn("com.apple.ResourceFork", self.run_command("xattr", "-r", str(app)))

    @staticmethod
    def run_command(*arguments):
        return subprocess.run(arguments, capture_output=True, text=True, check=True, timeout=30).stdout

    def input_state(self, paths):
        return {str(path): (hashlib.sha256(path.read_bytes()).hexdigest(),
                            self.run_command("xattr", "-lx", str(path))) for path in paths}


if __name__ == "__main__":
    unittest.main()
