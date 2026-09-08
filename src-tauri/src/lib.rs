#![forbid(unsafe_code)]

pub mod analytics;
pub mod cache;
pub mod credentials;
pub mod error;
pub mod http;
pub mod model;
pub mod notifier;
pub mod poller;
pub mod profile;
pub mod settings;
pub mod tray;
pub mod updater;
pub mod usage;

use std::sync::Arc;

/// One fetch, printed as JSON, no UI. The first thing to run when the endpoint
/// changes (spec §15).
pub fn debug_once() {
    let runtime = tokio::runtime::Runtime::new().expect("cannot start tokio runtime");
    runtime.block_on(async {
        let token = match credentials::read_token() {
            Ok(token) => token,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        };
        let client = reqwest::Client::new();
        let quotas = usage::fetch_usage(&client, usage::API_BASE, &token).await;
        let profile = profile::fetch_profile(&client, usage::API_BASE, &token).await;

        match quotas {
            Ok(quotas) => println!("{}", serde_json::to_string_pretty(&quotas).unwrap()),
            Err(e) => eprintln!("usage: {e}"),
        }
        match profile {
            Ok(profile) => println!("{}", serde_json::to_string_pretty(&profile).unwrap()),
            Err(e) => eprintln!("profile: {e}"),
        }
    });
}

#[tauri::command]
fn refresh_now(app: tauri::AppHandle) {
    use tauri::Manager;
    if let Some(signal) = app.try_state::<Arc<poller::RefreshSignal>>() {
        signal.request();
    }
}

#[tauri::command]
fn open_settings(app: tauri::AppHandle) {
    use tauri::Manager;
    if let Some(window) = app.get_webview_window("settings") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[tauri::command]
fn get_settings(app: tauri::AppHandle) -> settings::Settings {
    settings::load(&app)
}

/// Everything the analytics scan has read so far, and how far into each file
/// it got. Held across calls so the second and later scans read only what has
/// been appended, rather than re-parsing hundreds of megabytes of transcript
/// every time the panel opens.
///
/// It grows for the life of the process and is never trimmed, which is a
/// deliberate acceptance rather than an oversight. The bound is one `Entry`
/// plus one `HashSet` string per request ever seen — around 300 bytes, so
/// roughly 9 MB for the 31,000 requests on the machine this was measured on,
/// and it only grows as new requests are made while the app stays running.
/// Trimming it is not cheap: dropping old entries would lower the very
/// lifetime totals the tab exists to show, and folding entries into running
/// aggregates instead would still leave the `seen` set — the larger half —
/// growing, in exchange for reworking the one code path where a mistake
/// silently changes a dollar figure. Worth revisiting only with a measured
/// reason to.
#[derive(Default)]
pub struct AnalyticsState {
    inner: std::sync::Mutex<AnalyticsInner>,
}

#[derive(Default)]
struct AnalyticsInner {
    offsets: std::collections::HashMap<std::path::PathBuf, u64>,
    /// Request ids already accumulated into `entries`.
    ///
    /// `scan_dir` deduplicates within one call, which is enough for the
    /// repeated lines of a single request. This set is the same guard across
    /// calls, and it is not redundant: a transcript that is rotated or
    /// truncated beneath us is re-read from the start by design, and without
    /// this every request in it would be appended to `entries` a second time
    /// and counted twice in the estimate. Entries with no request id — four
    /// of the 31,000 usage-bearing lines on this machine — have no identity
    /// to deduplicate on and can still be re-counted in that case.
    seen: std::collections::HashSet<String>,
    entries: Vec<analytics::scan::Entry>,
}

impl AnalyticsInner {
    /// Fold one scan's output into the running set, dropping any request
    /// already counted.
    fn accumulate(&mut self, fresh: Vec<analytics::scan::Entry>) {
        for entry in fresh {
            if entry.request_id.is_empty() || self.seen.insert(entry.request_id.clone()) {
                self.entries.push(entry);
            }
        }
    }
}

/// The usage estimate built from Claude Code's local transcripts. Returns an
/// empty summary — not an error — while the feature is switched off, so the
/// tab has something to render and the setting is the only gate.
#[tauri::command]
async fn analytics_summary(app: tauri::AppHandle) -> Result<analytics::Summary, String> {
    summary_for(&app).await
}

/// The body of `analytics_summary`, generic over the Tauri runtime for the
/// same reason `settings::load` is: a `#[tauri::command]` takes a concrete
/// `AppHandle` (= `AppHandle<Wry>`) and needs a real webview, whereas this can
/// be driven against `tauri::test`'s `MockRuntime`. That is what makes the
/// `analytics_enabled` gate — the one thing standing between "off by default"
/// and reading the user's transcripts unasked — testable at all.
async fn summary_for<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<analytics::Summary, String> {
    if !settings::load(app).analytics_enabled {
        return Ok(analytics::Summary::default());
    }
    let root = dirs::home_dir()
        .ok_or("no home directory")?
        .join(".claude")
        .join("projects");

    // Reading and parsing every transcript is blocking, filesystem-bound work
    // measured in hundreds of megabytes on a well-used machine — 371 MB
    // across 214 files here. Left on an async worker, the first scan would
    // hold that thread for its whole duration, and the poller shares this
    // runtime. `spawn_blocking` puts it on the pool meant for exactly this,
    // which is what keeps the panel answering while the scan runs.
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        use tauri::Manager;
        let state = app.state::<AnalyticsState>();
        // A panic anywhere inside the scan poisons this mutex, and treating
        // that as an error made the poisoning permanent: every later call
        // answered "analytics state unavailable" for the rest of the
        // process, so one bad line killed the tab until the app was
        // restarted. Recovering instead is sound here because of what is
        // behind the lock — a `HashMap` of file cursors, a `HashSet` of
        // request ids and a `Vec` of entries, each of which a panic between
        // operations leaves structurally intact. The worst a recovered
        // guard can carry is a half-folded scan, and the very next call
        // re-reads from the cursors that did get written.
        let mut guard = state
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let fresh = analytics::scan::scan_dir(&root, &mut guard.offsets);
        guard.accumulate(fresh);
        Ok(analytics::summarize(&guard.entries))
    })
    .await
    // Deliberately discards the join error rather than rendering it. It is
    // the only place a panic payload from the scan could reach a string the
    // UI shows, and nothing from a transcript may travel that way.
    .map_err(|_| "the usage scan did not finish".to_string())?
}

/// The version the popover footer shows. Read from `CARGO_PKG_VERSION` rather
/// than `package.json`, because `src-tauri/Cargo.toml` is the version the
/// release workflow checks the tag against and the one the updater compares —
/// sourcing the footer anywhere else would let the number a user reads drift
/// from the number that decides whether they get an update.
#[tauri::command]
fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[tauri::command]
fn set_settings(app: tauri::AppHandle, settings: settings::Settings) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    set_settings_for(&app, &settings)?;

    let manager = app.autolaunch();
    let autostart_result = if settings.launch_at_login {
        manager.enable()
    } else {
        manager.disable()
    };
    // A failure here means the store now says `launch_at_login` but the
    // LaunchAgent/.desktop file does not match it — that divergence has to
    // reach the caller rather than being swallowed, so the settings UI can
    // show it instead of silently lying about what took effect. It is
    // reported after the work above rather than before it: the store is
    // written and the menu bar retitled either way, and what the user just
    // saved must not go unshown because a LaunchAgent file could not be.
    autostart_result.map_err(|e| e.to_string())
}

/// The body of `set_settings`: everything that follows from the settings
/// having been saved. Generic over the Tauri runtime for the same reason
/// `summary_for` is — a `#[tauri::command]` takes a concrete `AppHandle`
/// (= `AppHandle<Wry>`) and needs a real webview, whereas this can be driven
/// against `tauri::test`'s `MockRuntime`. Only the autostart toggle stays
/// behind in the command, because it needs a plugin the mock app has no
/// reason to register.
///
/// Returns the menu-bar title it re-rendered, which the command discards; see
/// `tray::refresh_title` for why it is returned at all.
fn set_settings_for<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    settings: &settings::Settings,
) -> Result<Option<String>, String> {
    settings::save(app, settings)?;

    // Which figures the menu bar shows is a display setting, and the snapshot
    // it renders is already in hand — so the title is rebuilt here, from that
    // snapshot, the instant the setting changes. This used to be left to the
    // refresh below, which is a network fetch: `request()` is refused for
    // sixty seconds after any refresh (a panel open, or a poll on the default
    // interval), and even when it is granted the title only changes once the
    // fetch comes back, so the change a user just saved took until the next
    // poll to appear — up to ten minutes on the longest interval the settings
    // window offers.
    let title = tray::refresh_title(app);

    // Asking for fresh numbers on a settings change is still right, so it
    // stays. What changed is that it is no longer what makes the title
    // correct: the two are independent now, and this being refused by the
    // throttle costs nothing but the numbers being a poll older.
    if let Some(signal) = {
        use tauri::Manager;
        app.try_state::<Arc<poller::RefreshSignal>>()
    } {
        signal.request();
    }
    Ok(title)
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_positioner::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        // Registered for `updater.rs`'s dialogs, which are the only dialogs
        // this app opens. `DialogExt::dialog()` reads this plugin's managed
        // state, so without the registration `app.dialog()` panics; but the
        // registration alone is the whole requirement — a Rust-side dialog
        // never passes through `invoke_handler`, so it needs no entry in
        // `capabilities/default.json` (reasoning and sources in the
        // `updater` module doc comment).
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(Arc::new(poller::RefreshSignal::default()))
        .manage(Arc::new(updater::UpdateCheckStatus::default()))
        .manage(Arc::new(tray::LastQuotaLines::default()))
        .manage(Arc::new(tray::LastMenu::default()))
        .manage(Arc::new(tray::LastSnapshot::default()))
        .manage(AnalyticsState::default())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.handle()
                .set_activation_policy(tauri::ActivationPolicy::Accessory)?;

            tray::build(app.handle())?;
            poller::spawn(app.handle().clone(), poller::PollConfig::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            refresh_now,
            open_settings,
            get_settings,
            set_settings,
            app_version,
            analytics_summary
        ])
        .on_window_event(|window, event| match window.label() {
            "popover" => {
                if let tauri::WindowEvent::Focused(false) = event {
                    let _ = window.hide();
                }
            }
            // Closing destroys a window, after which open_settings finds nothing
            // and the menu item silently stops working. Hide instead.
            "settings" => {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
            _ => {}
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::Manager;

    fn entry(request_id: &str, input: u64) -> analytics::scan::Entry {
        analytics::scan::Entry {
            request_id: request_id.into(),
            model: "claude-opus-5".into(),
            timestamp: chrono::Utc::now(),
            project: "alpha".into(),
            input,
            output: 0,
            cache_write_5m: 0,
            cache_write_1h: 0,
            cache_read: 0,
        }
    }

    /// `scan_dir` re-reads a transcript from the start when its recorded
    /// offset is past the end — a file rotated or truncated beneath us — and
    /// `analytics_summary` accumulates across calls, so without this guard
    /// that re-read would append every request a second time and double the
    /// estimate. The second scan below is the same content again, which is
    /// exactly what a re-read hands back.
    #[test]
    fn a_re_read_transcript_is_not_counted_twice() {
        let mut inner = AnalyticsInner::default();
        inner.accumulate(vec![entry("req_1", 10), entry("req_2", 20)]);
        inner.accumulate(vec![entry("req_1", 10), entry("req_2", 20)]);

        assert_eq!(inner.entries.len(), 2);
        assert_eq!(analytics::summarize(&inner.entries).total_tokens, 30);
    }

    #[test]
    fn genuinely_new_requests_are_appended() {
        let mut inner = AnalyticsInner::default();
        inner.accumulate(vec![entry("req_1", 10)]);
        inner.accumulate(vec![entry("req_2", 20)]);

        assert_eq!(inner.entries.len(), 2);
        assert_eq!(analytics::summarize(&inner.entries).total_tokens, 30);
    }

    /// An empty request id is an absence, not an identity. Two such entries
    /// are two requests and must both survive — deduplicating on the empty
    /// string would silently collapse every one of them into one.
    #[test]
    fn entries_without_a_request_id_are_each_kept() {
        let mut inner = AnalyticsInner::default();
        inner.accumulate(vec![entry("", 10), entry("", 20)]);

        assert_eq!(inner.entries.len(), 2);
        assert_eq!(analytics::summarize(&inner.entries).total_tokens, 30);
    }

    /// Scoped `HOME` plus a `MockRuntime` app with the real store plugin, the
    /// same pairing `settings::tests` uses. Both helpers come from there
    /// rather than being copied, so there is one definition of what a scoped
    /// home is, and `HomeGuard` serializes every test that overrides `HOME` —
    /// these tests resolve `~/.claude/projects` through it.
    fn scoped_app(
        home: &std::path::Path,
    ) -> (
        settings::tests::HomeGuard,
        tauri::App<tauri::test::MockRuntime>,
    ) {
        let guard = settings::tests::HomeGuard::scoped_to(home);
        let app = settings::tests::mock_app_with_store();
        (guard, app)
    }

    /// Writes one transcript under `<home>/.claude/projects`, exactly where
    /// `summary_for` will look for it.
    fn plant_transcript(home: &std::path::Path) {
        let project = home
            .join(".claude")
            .join("projects")
            .join("-Users-me-Projects-alpha");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            project.join("session.jsonl"),
            concat!(
                r#"{"type":"assistant","requestId":"r1","timestamp":"2026-09-07T12:00:00.000Z","message":{"model":"claude-opus-5","usage":{"input_tokens":1000000,"output_tokens":0}}}"#,
                "\n"
            ),
        )
        .unwrap();
    }

    /// One quota, shaped like what the poller publishes: 42% used, resetting
    /// in three hours and twenty minutes. The extra half-minute keeps the
    /// countdown reading `3h20m` rather than tipping to `3h19m` if the test
    /// takes a moment to get there.
    fn published_snapshot() -> model::UsageSnapshot {
        model::UsageSnapshot {
            quotas: vec![model::Quota {
                id: "session".into(),
                label: "Session".into(),
                percent: 42.0,
                severity: model::Severity::from_percent(42.0),
                resets_at: Some(
                    chrono::Utc::now()
                        + chrono::Duration::minutes(200)
                        + chrono::Duration::seconds(30),
                ),
                is_active: true,
            }],
            fetched_at: chrono::Utc::now(),
            stale: false,
        }
    }

    /// Settings showing the session quota as percentage only, countdown only,
    /// or both — the very choice the settings window offers and the bug is
    /// about.
    fn title_settings(show_percent: bool, show_countdown: bool) -> settings::Settings {
        settings::Settings {
            title_entries: vec![tray::TitleEntry {
                quota_id: "session".into(),
                show_percent,
                show_countdown,
            }],
            ..settings::Settings::default()
        }
    }

    /// The reported bug, exactly: change which figures the menu bar shows,
    /// save, and the menu bar does not change. Observed as the title string
    /// each save renders, because `MockRuntime` registers no tray to read one
    /// back from. Against the code before the fix both saves rendered nothing
    /// at all — asking the poller for a fetch that would eventually redraw the
    /// title was the only thing a save did about it.
    ///
    /// Which leaves one line of the fix that no test here reaches: deleting
    /// the `set_title` push inside `tray::refresh_title` leaves this suite
    /// green. That is the constraint `current_quota_lines` and
    /// `trailing_rows` already live with — `muda` refuses to build a menu off
    /// the main thread, and no `#[test]` runs on it — and `apply`'s own push
    /// has always sat in the same untested position.
    ///
    /// Both land inside the sixty-second manual-refresh window, which is the
    /// ordinary state of this app: opening the panel refreshes, and the
    /// default poll interval is sixty seconds. That the window really is
    /// closed is asserted on both sides of the saves rather than assumed, so
    /// a `RefreshThrottle` loosened to let these through — the fix this bug
    /// does not want — fails here rather than passing.
    ///
    /// Two saves, selecting different figures, because each has to render
    /// under the settings it has just written: re-rendering from anything
    /// else, the settings on disk a moment earlier included, gives a
    /// different string here.
    #[test]
    fn each_save_retitles_the_menu_bar_even_while_the_refresh_throttle_refuses() {
        let dir = tempfile::tempdir().unwrap();
        let (_home, app) = scoped_app(dir.path());
        app.handle().manage(Arc::new(tray::LastSnapshot::default()));

        // A poll has published a snapshot, as one has by the time anyone is
        // looking at the menu bar to complain about it.
        tray::apply(app.handle(), &published_snapshot());

        // And something has refreshed inside the last minute: a panel open,
        // or a poll on the default interval. From here the throttle refuses,
        // which is the state the bug needs.
        let signal = Arc::new(poller::RefreshSignal::default());
        app.handle().manage(signal.clone());
        assert!(
            signal.request(),
            "the first request through is the one that closes the window"
        );
        assert!(!signal.request(), "and it refuses from here on");

        let percent_only = set_settings_for(app.handle(), &title_settings(true, false)).unwrap();
        let countdown_only = set_settings_for(app.handle(), &title_settings(false, true)).unwrap();

        assert_eq!(percent_only.as_deref(), Some("42%"));
        assert_eq!(countdown_only.as_deref(), Some("3h20m"));
        assert!(
            !signal.request(),
            "and nothing here relaxed the throttle: it is still refusing"
        );
    }

    /// Two quotas as percentages only, which is the shortest thing a
    /// separator can sit between. Countdowns are left off on purpose: what
    /// is under test is the join, and a clock in the expected string would
    /// only add a way for it to fail for another reason.
    fn separator_settings(separator: tray::TitleSeparator) -> settings::Settings {
        settings::Settings {
            title_entries: vec![
                tray::TitleEntry {
                    quota_id: "session".into(),
                    show_percent: true,
                    show_countdown: false,
                },
                tray::TitleEntry {
                    quota_id: "weekly_all".into(),
                    show_percent: true,
                    show_countdown: false,
                },
            ],
            title_separator: separator,
            ..settings::Settings::default()
        }
    }

    /// The whole path the choice travels, with nothing between the ends
    /// stubbed: the settings window's value → `save` → the store file →
    /// `load` → `render_title` → the string handed to the menu bar. The
    /// unit tests in `tray::render` pin what each glyph looks like; this is
    /// the one that fails if `title_for` renders with anything other than
    /// the separator that was saved — passing `TitleSeparator::default()`
    /// there, say, which every test in `render.rs` would survive.
    ///
    /// The first save is what makes the second mean something: the same two
    /// quotas, the same call, under today's default join.
    #[test]
    fn a_saved_separator_reaches_the_menu_bar_title() {
        let dir = tempfile::tempdir().unwrap();
        let (_home, app) = scoped_app(dir.path());
        app.handle().manage(Arc::new(tray::LastSnapshot::default()));

        let mut snapshot = published_snapshot();
        snapshot.quotas.push(model::Quota {
            id: "weekly_all".into(),
            label: "Weekly".into(),
            percent: 7.0,
            severity: model::Severity::from_percent(7.0),
            resets_at: None,
            is_active: false,
        });
        tray::apply(app.handle(), &snapshot);

        let by_default = set_settings_for(
            app.handle(),
            &separator_settings(tray::TitleSeparator::Space),
        )
        .unwrap();
        let with_pipe = set_settings_for(
            app.handle(),
            &separator_settings(tray::TitleSeparator::Pipe),
        )
        .unwrap();

        assert_eq!(by_default.as_deref(), Some("42%  7%"));
        assert_eq!(with_pipe.as_deref(), Some("42% | 7%"));
    }

    /// The two halves composed, which neither the store test in `tray` nor
    /// the throttle test above does on its own: sign out, then save. The
    /// signed-out path renders an empty snapshot, so the title a save
    /// re-renders from the stored one has to be empty too — the quotas from
    /// before the sign-out must not come back. Observed as the rendered title
    /// string, for the reason given above.
    ///
    /// The first save is here to make the second mean something: it shows the
    /// same call rendering the real figures a moment earlier, so an empty
    /// answer at the end is the sign-out taking effect and not the harness
    /// rendering nothing all along.
    #[test]
    fn a_save_after_signing_out_does_not_bring_back_the_old_numbers() {
        let dir = tempfile::tempdir().unwrap();
        let (_home, app) = scoped_app(dir.path());
        app.handle().manage(Arc::new(tray::LastSnapshot::default()));

        tray::apply(app.handle(), &published_snapshot());
        let while_signed_in = set_settings_for(app.handle(), &title_settings(true, true)).unwrap();
        assert_eq!(while_signed_in.as_deref(), Some("42% · 3h20m"));

        // What `poller.rs` renders on the signed-out path: no quotas at all.
        tray::apply(
            app.handle(),
            &model::UsageSnapshot {
                quotas: Vec::new(),
                fetched_at: chrono::Utc::now(),
                stale: false,
            },
        );

        let after_signing_out =
            set_settings_for(app.handle(), &title_settings(true, true)).unwrap();
        assert_eq!(
            after_signing_out.as_deref(),
            Some(""),
            "the save must render the signed-out snapshot, not the numbers before it"
        );
    }

    /// The half of the old behaviour that was right and stays: a settings
    /// change is still a good moment to ask for fresh numbers. A
    /// `RefreshSignal` nothing has touched lets exactly one request through,
    /// so if the save made that request the window is closed afterwards, and
    /// if the save stopped making it the window would still be open.
    #[test]
    fn a_save_still_asks_the_poller_for_fresh_numbers() {
        let dir = tempfile::tempdir().unwrap();
        let (_home, app) = scoped_app(dir.path());
        app.handle().manage(Arc::new(tray::LastSnapshot::default()));
        let signal = Arc::new(poller::RefreshSignal::default());
        app.handle().manage(signal.clone());

        set_settings_for(app.handle(), &title_settings(true, true)).unwrap();

        assert!(
            !signal.request(),
            "the save's own request was let through, which is what closed the window"
        );
    }

    /// Saving before the first poll has published anything — the settings
    /// window opens from the menu, which is available immediately — has no
    /// numbers to render, and must answer with no title rather than panicking
    /// on the absent snapshot. That the tray is then left alone rather than
    /// blanked follows from `refresh_title` returning before it pushes
    /// anything; it is not observed here.
    #[test]
    fn a_save_before_the_first_snapshot_renders_no_title() {
        let dir = tempfile::tempdir().unwrap();
        let (_home, app) = scoped_app(dir.path());
        app.handle().manage(Arc::new(tray::LastSnapshot::default()));

        let rendered = set_settings_for(app.handle(), &title_settings(true, true)).unwrap();
        assert_eq!(rendered, None);
    }

    /// The gate, from the side that matters. Analytics is off by default, and
    /// while it is off this app must not read the user's transcripts at all —
    /// so a transcript sitting right where the scan would look must produce
    /// the same empty summary as an empty disk.
    ///
    /// Nothing failed if the gate was deleted before this existed: the enabled
    /// path returns a real summary and the disabled path was never exercised.
    #[test]
    fn no_transcript_is_read_while_analytics_is_switched_off() {
        let dir = tempfile::tempdir().unwrap();
        plant_transcript(dir.path());
        let (_home, app) = scoped_app(dir.path());
        app.handle().manage(AnalyticsState::default());

        settings::save(
            app.handle(),
            &settings::Settings {
                analytics_enabled: false,
                ..settings::Settings::default()
            },
        )
        .unwrap();

        let summary = tauri::async_runtime::block_on(summary_for(app.handle())).unwrap();
        assert_eq!(summary, analytics::Summary::default());
        assert_eq!(summary.total_tokens, 0, "the transcript must not be read");
    }

    /// A panic while the analytics lock is held poisons it, and a poisoned
    /// `Mutex` stays poisoned for the life of the process. Treating that as
    /// an error meant one panic anywhere inside the scan left every later
    /// call answering "analytics state unavailable" until the app was
    /// restarted — the tab dead, with a message that gives no hint that
    /// restarting is the cure.
    ///
    /// The panic is raised here rather than provoked from inside the scan
    /// because `scan_dir` deliberately has no panicking path; what is under
    /// test is the recovery, not any particular way of getting there. The
    /// assertion between the two halves is what makes this mean something:
    /// it confirms the lock really is poisoned before asking `summary_for`
    /// to work through it.
    #[test]
    fn a_poisoned_analytics_lock_recovers_instead_of_failing_for_the_process_lifetime() {
        let dir = tempfile::tempdir().unwrap();
        plant_transcript(dir.path());
        let (_home, app) = scoped_app(dir.path());
        app.handle().manage(AnalyticsState::default());

        settings::save(
            app.handle(),
            &settings::Settings {
                analytics_enabled: true,
                ..settings::Settings::default()
            },
        )
        .unwrap();

        let state = app.handle().state::<AnalyticsState>();
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _held = state.inner.lock().unwrap();
            panic!("as a panic inside the scan would");
        }));
        assert!(panicked.is_err(), "the closure above must have panicked");
        assert!(
            state.inner.lock().is_err(),
            "this test says nothing unless the lock really is poisoned"
        );

        let summary = tauri::async_runtime::block_on(summary_for(app.handle())).unwrap();
        assert_eq!(summary.total_tokens, 1_000_000);
    }

    /// The other side of the same switch, which is what stops the test above
    /// from passing against a command that always returns nothing: the very
    /// same transcript, the very same app, analytics on.
    ///
    /// This is also the only test that drives the whole command path —
    /// settings load, home resolution, `spawn_blocking`, the managed state,
    /// the scan and the summary — rather than its pieces.
    #[test]
    fn switching_analytics_on_reads_the_same_transcript() {
        let dir = tempfile::tempdir().unwrap();
        plant_transcript(dir.path());
        let (_home, app) = scoped_app(dir.path());
        app.handle().manage(AnalyticsState::default());

        settings::save(
            app.handle(),
            &settings::Settings {
                analytics_enabled: true,
                ..settings::Settings::default()
            },
        )
        .unwrap();

        let summary = tauri::async_runtime::block_on(summary_for(app.handle())).unwrap();
        assert_eq!(summary.total_tokens, 1_000_000);
        assert_eq!(summary.by_model.len(), 1);
        assert_eq!(summary.by_model[0].name, "claude-opus-5");
        assert!(
            (summary.total_cost - 5.0).abs() < 1e-9,
            "{}",
            summary.total_cost
        );

        // And the state really is carried across calls: a second run over an
        // unchanged tree reads nothing new and must not double the figures.
        let again = tauri::async_runtime::block_on(summary_for(app.handle())).unwrap();
        assert_eq!(again.total_tokens, 1_000_000);
    }
}
