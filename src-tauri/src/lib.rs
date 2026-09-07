#![forbid(unsafe_code)]

pub mod cache;
pub mod credentials;
pub mod error;
pub mod http;
pub mod model;
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

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_positioner::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .manage(Arc::new(poller::RefreshSignal::default()))
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.handle()
                .set_activation_policy(tauri::ActivationPolicy::Accessory)?;

            tray::build(app.handle())?;
            poller::spawn(app.handle().clone(), poller::PollConfig::default());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
