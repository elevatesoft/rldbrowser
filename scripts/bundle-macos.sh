#!/usr/bin/env bash
set -euo pipefail

BUILD_DMG="${BUILD_DMG:-0}"

for arg in "$@"; do
  case "$arg" in
    --dmg)
      BUILD_DMG=1
      ;;
    -h|--help)
      echo "Usage: $0 [--dmg]"
      echo "  --dmg  Also build a drag-to-Applications macOS disk image (or set BUILD_DMG=1)"
      exit 0
      ;;
    *)
      echo "Unknown option: $arg" >&2
      echo "Usage: $0 [--dmg]" >&2
      exit 1
      ;;
  esac
done

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

if [[ "$BUILD_DMG" == "1" ]]; then
  DMG_PATH="$DIST_DIR/RLD-Browser-macos-arm64.dmg"
  DMG_STAGE="$(mktemp -d)"
  trap 'rm -rf "$DMG_STAGE"' EXIT

  echo "Creating disk image $DMG_PATH..."
  ditto "$APP_DIR" "$DMG_STAGE/RLD Browser.app"
  ln -s /Applications "$DMG_STAGE/Applications"
  rm -f "$DMG_PATH"
  hdiutil create \
    -volname "RLD Browser" \
    -srcfolder "$DMG_STAGE" \
    -ov \
    -format UDZO \
    "$DMG_PATH"
  rm -rf "$DMG_STAGE"
  trap - EXIT
  echo "Done: $DMG_PATH"
fi
