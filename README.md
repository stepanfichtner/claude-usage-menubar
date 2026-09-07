# Claude Usage

A menu bar / tray monitor for Claude subscription limits. Shows your 5-hour
session window, your weekly all-models window, and any per-model weekly windows,
with reset countdowns and threshold notifications.

The limits are account-wide, so one glance covers every Claude Code project and
the Claude desktop app at once. That is the whole reason it exists: running several
projects plus the desktop app, there is otherwise nowhere to see the total.

macOS and Ubuntu, one codebase.

## What it shows

In the menu bar: a crab, coloured by how much of your worst quota is gone, and
whichever figures you choose — percentage, countdown, or both, per quota.

| colour | |
|---|---|
| green | under 50% |
| yellow | 50-79% |
| orange | 80-89% |
| red | 90% and above |

In the panel: your account name and plan, each weekly quota as a ring, and the
current 5-hour session as a full-width meter, each with its reset time and a
countdown that ticks locally.

Notifications at 50, 80 and 90% of any quota — once per threshold per window, so
sitting above one does not produce a stream of them. The first sighting of a quota
after launch primes silently rather than firing, so a fresh install does not
announce thresholds you had already crossed before it existed.

Quotas come from the server's own list rather than a hard-coded set, so a new
model's weekly window appears without an update.

## Unofficial

This is not an Anthropic product and is not affiliated with Anthropic. It reads
an **undocumented** endpoint — the same one Claude Code's own `/usage` command
uses — which may change or disappear without notice. If that happens, the app
shows the last known figures and a message rather than crashing.

## What leaves your machine

Three kinds of outbound request, and nothing else:

- `GET /api/oauth/usage` — to `api.anthropic.com`, authenticated with the OAuth
  token Claude Code already stored when you signed in. Your limit percentages
  and reset times.
- `GET /api/oauth/profile` — to `api.anthropic.com`, the same token. Your
  account display name and subscription tier.
- The update check — to GitHub's release assets, unauthenticated. Only when
  you choose **Check for Updates…** from the tray menu; never automatically,
  and never on launch.

No telemetry, no crash reporting, no other third-party service. The app never
writes to `~/.claude/` and never modifies your Keychain.

From the profile response only the display name and the rate-limit tier are read;
the full name, email address and account identifiers in that response are never
deserialized, cached, or displayed.

## Install

### macOS

Download the `.dmg` from [Releases](../../releases) and drag the app to
Applications.

The build is **not code-signed or notarized**, so the first launch is blocked by
Gatekeeper. Either right-click the app and choose *Open*, or run:

```bash
xattr -dr com.apple.quarantine "/Applications/Claude Usage.app"
```

On first run macOS may ask for permission to read the `Claude Code-credentials`
Keychain item. Choose **Always Allow** — that item is the OAuth token, and
reading it is the whole point of the app.

### Ubuntu

Download the `.deb` from [Releases](../../releases):

```bash
sudo apt install ./claude-usage_*_amd64.deb
```

Or use the AppImage. On stock GNOME the tray needs the AppIndicator extension,
which Ubuntu ships and enables by default:

```bash
sudo apt install gnome-shell-extension-appindicator
```

Log out and back in if the icon does not appear.

## Updating

If you installed a release build, choose **Check for Updates…** from the tray
menu. It downloads and installs any newer release and restarts the app. While
it's running, the same menu item's label changes to **Checking for
updates…**, then to **Up to date (vX.Y.Z)**, **Update available —
installing…**, or **Check failed —** followed by a short reason, for about
30 seconds, before returning to normal. That label is the outcome — read it
there, not from a popup.

The app also tries to show a system notification with the same outcome, but
that is a courtesy, not a guarantee: the notification plugin this app uses
cannot report whether a notification was ever actually displayed (notification
permission can be revoked, or a minimal Linux desktop may have no notification
daemon at all, and either way the call still reports success). For the same
reason, a tray-icon tooltip carrying the outcome is set alongside the
notification as a second best-effort echo — worth checking by hovering the
icon if you're not sure you saw a notification — but on Ubuntu specifically
it is a documented no-op in the AppIndicator backend this app uses, so it
never actually appears there. The **menu label is the only channel this
feature actually depends on**, precisely because it is the one this app fully
controls.

If you're running from a source checkout instead:

```bash
git pull && pnpm run reinstall
```

Builds, stops any running copy, replaces the one in `/Applications`, relaunches, and
prints the version it installed.

## Build from source

Requires Rust 1.82+, Node 24+, pnpm 10+.

```bash
pnpm install
pnpm tauri dev     # run
pnpm tauri build   # produce installers
```

Ubuntu build dependencies:

```bash
sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev \
  patchelf build-essential curl wget file libxdo-dev libssl-dev
```

The tray icons are generated, not hand-drawn — `python3 scripts/generate-tray-icons.py`
regenerates all five from the colour table at the top of that file. No image library
needed.

### If the DMG step fails

`pnpm tauri build` can occasionally fail partway through, after the `.app` is
built, with a Finder/AppleScript error from the vendored `create-dmg` step
(something like `Can't get disk ... (-1728)`, or the run leaves a stray
`rw.<pid>.*.dmg` mounted under `/Volumes`). This is a known race in that
script's Finder automation — it styles the mounted disk image's window with
`osascript`, and occasionally runs that before Finder has caught up with the
just-attached volume — not something introduced by this project. It reproduces
the same way on a from-scratch checkout of the very first commit in this
repo's history, before any of the app's own code existed, which rules out a
project-specific cause.

The `.app` bundle is unaffected and already usable at that point; re-running
`pnpm tauri build` typically succeeds on the next attempt. `pnpm run reinstall`
already accounts for this: it treats a non-zero exit from `tauri build` as
non-fatal and checks for the `.app` directly rather than trusting the exit
code.

## Releasing

Bump the version in `src-tauri/Cargo.toml` (the single source of truth —
`tauri.conf.json` inherits it), merge to `main`, then tag and push:

```bash
git tag v0.2.0 && git push origin v0.2.0
```

`.github/workflows/release.yml` refuses the tag unless it is on `main` and
matches `Cargo.toml`, then builds macOS (universal `.dmg` + `.app`) and Ubuntu
(`.deb` + AppImage), and publishes a draft GitHub release with `latest.json`
for the in-app updater attached.

**If the macOS build job fails**, it is almost certainly the same `create-dmg`
race described above ("If the DMG step fails"), now hitting the build job's
first-ever real run instead of a local one. It affects only the human-facing
`.dmg` — the updater fetches the `.app.tar.gz` and its signature, never the
`.dmg` — so it can never break an update, and it is not a sign the release
itself is broken. Re-run just that job from the Actions tab; that is the fix,
not a rollback.

**If the signing key is ever lost** — the private half, in the
`TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` repository
secrets, or its password — no future release can be signed with the public
key already shipped in past installs. **Check for Updates…** on those installs
will keep reporting a failure (the downloaded bundle's signature will not
verify against the key they shipped with) rather than ever installing again,
and every existing user has to reinstall by hand from a fresh download. There
is no recovery short of that; treat the private key and its password with the
same care as the OAuth token this app reads.

## Troubleshooting

Run one fetch and print the result, without launching the UI:

```bash
cd src-tauri && cargo run -- --once
```

This is the first thing to try when the numbers look wrong or stop updating. It
exercises the credential read, both HTTP calls, and the response parsing.

**"It starts and immediately quits."** It did not. The app has no Dock icon and no
window of its own — look for the crab in the menu bar. If the bar is full, macOS
hides what does not fit; ⌘-drag icons to make room.

**Two crabs.** A copy left over from `pnpm tauri dev` is still running.
`pkill -f claude-usage-menubar` and start one.

**"showing cached data" that will not clear.** No fetch has succeeded since launch.
Check `cargo run -- --once`. The endpoint rate-limits in bursts — four requests in
ten seconds trips it and it clears after about two minutes.

## Licence

MIT.
