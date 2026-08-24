//! Shared application state, held behind `tauri::State`.
//!
//! Both fields start `None`: the app is locked until the (not-yet-built)
//! unlock command populates them, since `MODULE_02_SCHEMA.md`'s repository
//! layer needs a live, keyed `rusqlite::Connection` that only exists once
//! unlocked.

use std::sync::Mutex;

use blurt_schema::{Database, Keyring};

#[derive(Default)]
pub struct AppState {
    pub db: Mutex<Option<Database>>,
    pub keyring: Mutex<Option<Keyring>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pure data definition, no logic — exempt from the TDD Red/Green loop
    // per this project's CLAUDE.md ("pure config/data... no logic"). This
    // test just documents the starting-locked invariant.
    #[test]
    fn starts_locked() {
        let state = AppState::default();
        assert!(state.db.lock().unwrap().is_none());
        assert!(state.keyring.lock().unwrap().is_none());
    }
}
