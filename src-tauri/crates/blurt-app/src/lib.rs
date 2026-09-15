//! Orchestration layer — Tauri commands calling into the domain crates.
//!
//! Deliberately thin: this is the IPC seam, not where business logic lives.
//! See `docs/MODULE_01_ARCHITECTURE.md` §3 for the intended wiring (Tokio
//! async runtime, the `blurt-rag` model-lifecycle manager, and event-driven
//! frontend updates). Typed TS bindings via `tauri-specta` are deferred
//! until Module 6 UI work actually consumes them — see `docs/PROGRESS.md`.

pub mod commands;
pub mod dto;
pub mod error;
pub mod state;

/// Builds the Tauri application. The host binary (`src-tauri/src/main.rs`)
/// supplies the generated context and runs it.
///
/// Command handlers get registered here as each module lands.
pub fn builder() -> tauri::Builder<tauri::Wry> {
    tauri::Builder::default()
        .manage(state::AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::vault::initialize_vault,
            commands::vault::unlock,
            commands::vault::lock,
            commands::vault::is_unlocked,
            commands::destinations::create_destination,
            commands::destinations::get_destination_path,
            commands::destinations::rename_destination,
            commands::destinations::reparent_destination,
            commands::destinations::tombstone_destination,
            commands::search::classify_input,
            commands::search::search,
            commands::search::ask,
            commands::router::capture_item_via_router,
            commands::router::normalize_voice_transcript,
            commands::items::capture_item,
            commands::items::move_item,
            commands::items::set_item_checked,
            commands::items::tombstone_item,
            commands::items::list_items_for_destination,
            commands::edits::append_edit,
            commands::edits::item_edit_history,
        ])
}
