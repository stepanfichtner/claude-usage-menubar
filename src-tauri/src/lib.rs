#![forbid(unsafe_code)]

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

#[tauri::command]
fn set_settings(app: tauri::AppHandle, settings: settings::Settings) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    settings::save(&app, &settings)?;

    let manager = app.autolaunch();
    let _ = if settings.launch_at_login {
        manager.enable()
    } else {
        manager.disable()
    };

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
        .manage(Arc::new(poller::RefreshSignal::default()))
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
            set_settings
        ])
        .on_window_event(|window, event| {
            if window.label() == "popover" {
                if let tauri::WindowEvent::Focused(false) = event {
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
