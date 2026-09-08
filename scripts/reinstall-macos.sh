#!/usr/bin/env bash
# Build and replace the installed copy. macOS only; Linux installs from a .deb.
set -euo pipefail

APP="Claude Usage.app"
BUILT="src-tauri/target/release/bundle/macos/$APP"
DEST="/Applications/$APP"

# Only the .app — never the DMG. A local reinstall copies the bundle straight to
# /Applications, so the disk image is pure cost: building it mounts a volume,
# which opens a Finder window, and the unmount then fails against the running
# copy with "the item is in use" — two dialogs and a red build error for an
# artifact nothing here consumes. The release DMG is CI's job.
#
# `--bundles app` is why this no longer needs `|| true`: with the DMG gone, a
# non-zero exit means the build actually failed and should stop the script.
pnpm tauri build --bundles app
[ -d "$BUILT" ] || { echo "build produced no $APP — see the output above"; exit 1; }

pkill -f claude-usage-menubar 2>/dev/null || true
rm -rf "$DEST"
cp -R "$BUILT" "$DEST"
open -a "$APP"

/usr/libexec/PlistBuddy -c "Print :CFBundleShortVersionString" "$DEST/Contents/Info.plist" \
  | xargs echo "installed version:"
