fn main() {
    // Declare the app commands so the capability file can allow exactly
    // these (anything not granted in capabilities/ is denied).
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "poll",
            "alert_action",
            "update_geometry",
            "move_by",
            "set_locked",
            "get_state",
            "save_state",
            "open_dashboard",
            "quit",
            "ui_log",
        ]),
    ))
    .expect("failed to run tauri-build");
}
