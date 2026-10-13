#!/bin/bash
# Build AgentWatcher.app (ad-hoc signed) and the distributable DMG.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
APP="$ROOT/dist/AgentWatcher.app"
STAGE="$ROOT/dist/stage"
DMG="$ROOT/dist/AgentWatcher-0.1.0.dmg"
ICONSET="$ROOT/dist/icon.iconset"
MASTER="$ROOT/dist/icon-master.png"

rm -rf "$APP" "$STAGE" "$DMG" "$ICONSET" "$MASTER"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

# Icon (skipped gracefully when Pillow is unavailable, e.g. minimal CI images).
PYTHON_BIN="${PYTHON_BIN:-$(command -v python3 || true)}"
if [ -n "$PYTHON_BIN" ] && "$PYTHON_BIN" -c "import PIL" >/dev/null 2>&1; then
  "$PYTHON_BIN" "$ROOT/tools/make_icon.py" "$MASTER" >/dev/null
  mkdir -p "$ICONSET"
  for spec in "16 16x16" "32 16x16@2x" "32 32x32" "64 32x32@2x" "128 128x128" "256 128x128@2x" \
              "256 256x256" "512 256x256@2x" "512 512x512" "1024 512x512@2x"; do
    set -- $spec
    sips -z "$1" "$1" "$MASTER" --out "$ICONSET/icon_$2.png" >/dev/null
  done
  iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"
else
  echo "icon: skipped (python3 with Pillow not found)"
fi

# Binary + metadata.
swiftc -O -swift-version 5 "$ROOT/Sources/main.swift" -o "$APP/Contents/MacOS/AgentWatcher"
cp "$ROOT/Info.plist" "$APP/Contents/Info.plist"

codesign --force --deep --sign - "$APP"
codesign --verify --deep --strict "$APP" && echo "signature: verified (ad-hoc)"

# DMG with drag-to-Applications layout.
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
hdiutil create -volname "AgentWatcher" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
hdiutil verify "$DMG" >/dev/null && echo "dmg: verified"
shasum -a 256 "$DMG"
echo "built: $DMG"
