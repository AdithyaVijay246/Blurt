//! Orchestration layer — Tauri commands calling into the domain crates.
//!
//! Deliberately thin: this is the IPC seam, not where business logic lives.
//! See `docs/MODULE_01_ARCHITECTURE.md` §3 for the intended wiring (Tokio
//! async runtime, the `blurt-rag` model-lifecycle manager, the `tauri-specta`
//! IPC contract, and event-driven frontend updates).

/// Builds the Tauri application. The host binary (`src-tauri/src/main.rs`)
/// supplies the generated context and runs it.
///
/// Command handlers get registered here as each module lands.
pub fn builder() -> tauri::Builder<tauri::Wry> {
    tauri::Builder::default()
}
