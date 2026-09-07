#!/usr/bin/env bash
# Build and replace the installed copy. macOS only; Linux installs from a .deb.
set -euo pipefail

APP="Claude Usage.app"
BUILT="src-tauri/target/release/bundle/macos/$APP"
DEST="/Applications/$APP"

# The DMG step can fail while the .app is already built, so tolerate a non-zero
# exit here and check for the bundle itself rather than trusting the status.
pnpm tauri build || true
[ -d "$BUILT" ] || { echo "build produced no $APP — see the output above"; exit 1; }

pkill -f claude-usage-menubar 2>/dev/null || true
rm -rf "$DEST"
cp -R "$BUILT" "$DEST"
open -a "$APP"

/usr/libexec/PlistBuddy -c "Print :CFBundleShortVersionString" "$DEST/Contents/Info.plist" \
  | xargs echo "installed version:"
