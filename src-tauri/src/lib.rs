#![forbid(unsafe_code)]

pub mod model;
pub mod usage;

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
