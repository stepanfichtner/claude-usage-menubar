#!/usr/bin/env bash
# Build and replace the installed copy. macOS only; Linux installs from a .deb.
set -euo pipefail

APP="Claude Usage.app"
BUILT="src-tauri/target/release/bundle/macos/$APP"
DEST="/Applications/$APP"

# Build the .app and nothing else. Two things have to be turned off for that,
# and each one otherwise ends the build with a failure this script does not
# care about:
#
#   --bundles app   skips the DMG. Building it mounts a volume, which opens a
#                   Finder window, and the unmount then fails against the
#                   running copy with "the item is in use" — two dialogs for an
#                   artifact that goes straight in the bin. CI builds the real
#                   release DMG.
#   createUpdaterArtifacts=false
#                   skips the signed .app.tar.gz. It is on in tauri.conf.json
#                   because the release needs it, and signing it needs
#                   TAURI_SIGNING_PRIVATE_KEY — which lives in CI secrets and
#                   must never be on a developer machine. So locally it can
#                   only ever fail.
#
# With both off the build has no reason to fail except a real failure, which is
# why there is no `|| true` here and why the guard below is a second check
# rather than the only one.
pnpm tauri build --bundles app --config '{"bundle":{"createUpdaterArtifacts":false}}'
[ -d "$BUILT" ] || { echo "build produced no $APP — see the output above"; exit 1; }

pkill -f claude-usage-menubar 2>/dev/null || true
rm -rf "$DEST"
cp -R "$BUILT" "$DEST"
open -a "$APP"

/usr/libexec/PlistBuddy -c "Print :CFBundleShortVersionString" "$DEST/Contents/Info.plist" \
  | xargs echo "installed version:"
