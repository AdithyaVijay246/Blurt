// Thin host binary. All wiring lives in `blurt-app`; see
// docs/MODULE_01_ARCHITECTURE.md §3.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    blurt_app::builder()
        .run(tauri::generate_context!())
        .expect("error while running Blurt");
}
