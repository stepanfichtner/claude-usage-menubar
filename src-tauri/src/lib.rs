#![forbid(unsafe_code)]

pub mod credentials;
pub mod error;
pub mod http;
pub mod model;
pub mod profile;
pub mod usage;

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
        .setup(|_app| {
            #[cfg(target_os = "macos")]
            _app.handle()
                .set_activation_policy(tauri::ActivationPolicy::Accessory)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
