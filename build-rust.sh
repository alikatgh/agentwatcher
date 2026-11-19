#!/bin/bash
# Build the Rust/GPUI application. Existing app bundles and Swift files stay intact.
set -euo pipefail
AW_ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$AW_ROOT"
PYTHONDONTWRITEBYTECODE=1 python3 tools/setup_zed_engine.py
AW_CARGO="${AW_CARGO:-${CARGO_HOME:-$HOME/.cargo}/bin/cargo}"
if [ ! -x "$AW_CARGO" ]; then AW_CARGO="$(command -v cargo)"; fi
AW_PROFILE="${AW_PROFILE:-release}"
AW_OUTPUT_DIR="${AW_OUTPUT_DIR:-$AW_ROOT/dist/rust-$(date -u +%Y%m%d-%H%M%S)}"
if [[ "$AW_OUTPUT_DIR" != /* ]]; then AW_OUTPUT_DIR="$AW_ROOT/$AW_OUTPUT_DIR"; fi
AW_TARGET_DIR="${CARGO_TARGET_DIR:-$AW_ROOT/target}"
AW_APP="$AW_OUTPUT_DIR/AgentWatcher.app"
if [ -e "$AW_OUTPUT_DIR" ]; then
  echo "Output already exists; choose a new AW_OUTPUT_DIR: $AW_OUTPUT_DIR" >&2
  exit 1
fi
mkdir -p "$(dirname "$AW_OUTPUT_DIR")"
mkdir "$AW_OUTPUT_DIR"
AW_OUTPUT_DIR="$AW_OUTPUT_DIR" PYTHONDONTWRITEBYTECODE=1 python3 tools/package_zed_source.py \
  --record-inputs "$AW_OUTPUT_DIR/source-inputs.json"
"$AW_CARGO" build --locked --profile "$AW_PROFILE" -j "${AW_BUILD_JOBS:-2}"
AW_BINARY_PROFILE="$AW_PROFILE"
if [ "$AW_PROFILE" = dev ]; then AW_BINARY_PROFILE=debug; fi
# The output directory was claimed before recording build inputs. Existing
# output paths cannot be overwritten by concurrent builds or relative aliases.
mkdir -p "$AW_APP/Contents/MacOS" "$AW_APP/Contents/Resources"
cp "$AW_TARGET_DIR/$AW_BINARY_PROFILE/agentwatcher" "$AW_APP/Contents/MacOS/agentwatcher"
cp rust/Info.plist "$AW_APP/Contents/Info.plist"
cp rust/resources/AppIcon.icns "$AW_APP/Contents/Resources/AppIcon.icns"
cp rust/NOTICE "$AW_APP/Contents/Resources/NOTICE"
cp -R rust/LICENSES "$AW_APP/Contents/Resources/LICENSES"
cp -R rust/resources/themes "$AW_APP/Contents/Resources/themes"
cp rust/resources/fonts/manifest.json "$AW_APP/Contents/Resources/font-manifest.json"
# FileProvider can add Finder metadata when materializing the new bundle.
# Clear metadata only from this generated app, preserving all source inputs.
xattr -cr "$AW_APP"
codesign --force --deep --sign - "$AW_APP"
codesign --verify --deep --strict "$AW_APP"
ditto -c -k --sequesterRsrc --keepParent "$AW_APP" "$AW_OUTPUT_DIR/AgentWatcher-0.2.0-macos.zip"
AW_OUTPUT_DIR="$AW_OUTPUT_DIR" PYTHONDONTWRITEBYTECODE=1 python3 tools/package_zed_source.py \
  --output "$AW_OUTPUT_DIR/AgentWatcher-0.2.0-source.tar.gz"
AW_OUTPUT_DIR="$AW_OUTPUT_DIR" PYTHONDONTWRITEBYTECODE=1 python3 tools/package_zed_source.py \
  --check-inputs "$AW_OUTPUT_DIR/source-inputs.json"
shasum -a 256 "$AW_APP/Contents/MacOS/agentwatcher" "$AW_OUTPUT_DIR/AgentWatcher-0.2.0-macos.zip" "$AW_OUTPUT_DIR/AgentWatcher-0.2.0-source.tar.gz"
echo "Built: $AW_APP"
