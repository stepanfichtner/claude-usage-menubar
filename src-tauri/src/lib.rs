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
        let mut guard = state
            .inner
            .lock()
            .map_err(|_| "analytics state unavailable".to_string())?;
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
    settings::save(&app, &settings)?;

    let manager = app.autolaunch();
    let autostart_result = if settings.launch_at_login {
        manager.enable()
    } else {
        manager.disable()
    };
    // A failure here means the store now says `launch_at_login` but the
    // LaunchAgent/.desktop file does not match it — that divergence has to
    // reach the caller rather than being swallowed, so the settings UI can
    // show it instead of silently lying about what took effect.
    autostart_result.map_err(|e| e.to_string())?;

    if let Some(signal) = {
        use tauri::Manager;
        app.try_state::<Arc<poller::RefreshSignal>>()
    } {
        signal.request();
    }
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_positioner::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(Arc::new(poller::RefreshSignal::default()))
        .manage(Arc::new(updater::UpdateCheckStatus::default()))
        .manage(Arc::new(tray::LastQuotaLines::default()))
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
