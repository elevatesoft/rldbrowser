#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
DIST_DIR="$ROOT_DIR/dist"
APP_DIR="$DIST_DIR/RLD Browser.app"
BIN_PATH="$ROOT_DIR/target/release/rustbrowser"
PROCTOR_BIN_PATH="$ROOT_DIR/target/release/rustbrowser-proctor"
PROCTOR_OUT_PATH="$DIST_DIR/rld-proctor"

echo "Building release binaries (RLD Browser + rld-proctor)..."
cargo build --release --manifest-path "$ROOT_DIR/Cargo.toml" -p rustbrowser -p rustbrowser-proctor

echo "Assembling $APP_DIR..."
rm -rf "$APP_DIR" "$DIST_DIR/rustbrowser.app"
mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources" "$DIST_DIR"
cp "$BIN_PATH" "$APP_DIR/Contents/MacOS/rld-browser"
rm -f "$DIST_DIR/rustbrowser-proctor" "$PROCTOR_OUT_PATH"
cp "$PROCTOR_BIN_PATH" "$PROCTOR_OUT_PATH"
cp "$ROOT_DIR/packaging/macos/Info.plist" "$APP_DIR/Contents/Info.plist"
chmod +x "$APP_DIR/Contents/MacOS/rld-browser" "$PROCTOR_OUT_PATH"
codesign --force --sign - "$PROCTOR_OUT_PATH"

if [[ "${CODESIGN_IDENTITY:-}" != "" ]]; then
  echo "Signing with identity: $CODESIGN_IDENTITY"
  if [[ -f "$ROOT_DIR/packaging/macos/entitlements.plist" ]]; then
    codesign --force --options runtime --entitlements "$ROOT_DIR/packaging/macos/entitlements.plist" --sign "$CODESIGN_IDENTITY" "$APP_DIR"
  else
    codesign --force --options runtime --sign "$CODESIGN_IDENTITY" "$APP_DIR"
  fi
else
  echo "Ad-hoc signing local bundle (set CODESIGN_IDENTITY for Apple Developer signing)..."
  codesign --force --deep --sign - "$APP_DIR" >/dev/null 2>&1 || true
fi

echo "Done: $APP_DIR"
