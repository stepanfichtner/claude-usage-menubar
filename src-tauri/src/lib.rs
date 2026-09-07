#![forbid(unsafe_code)]

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.handle()
                .set_activation_policy(tauri::ActivationPolicy::Accessory)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
