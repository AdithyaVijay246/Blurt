//! Destination commands — thin wraps over `blurt_schema::repository::destinations`.
//!
//! Each command is a one-line `#[tauri::command]` delegating to a plain
//! `<name>_impl(state: &AppState, ...)` function. This split exists because
//! `tauri::test`'s mock-app harness crashes at process startup on this dev
//! machine (`STATUS_ENTRYPOINT_NOT_FOUND`, unrelated to this crate's code —
//! see `docs/PROGRESS.md`) — the `_impl` functions are fully testable
//! against a bare `AppState` with no Tauri runtime involved at all.

use tauri::State;

use blurt_schema::repository::destinations;

use crate::commands::parse_uuid;
use crate::dto::{DestinationDto, DestinationKindDto};
use crate::error::CommandResult;
use crate::state::AppState;

/// `sortOrder` is never frontend-settable (reordering is a Module 6 drag
/// concern layered on later) — new destinations sort to the end by current
/// timestamp, a simple monotonic default.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is before the Unix epoch")
        .as_millis() as i64
}

fn create_destination_impl(
    state: &AppState,
    name: String,
    trigger: String,
    kind: DestinationKindDto,
    parent_id: Option<String>,
    is_sensitive: bool,
) -> CommandResult<DestinationDto> {
    let parent_id = parent_id.map(|p| parse_uuid(&p)).transpose()?;
    let guard = state.db.lock().unwrap();
    let db = guard.as_ref().ok_or(crate::error::CommandError::Locked)?;
    let created = destinations::create(
        db.conn(),
        &name,
        &trigger,
        kind.into(),
        parent_id,
        false,
        is_sensitive,
        now_ms(),
    )?;
    Ok(created.into())
}

#[tauri::command]
pub fn create_destination(
    state: State<AppState>,
    name: String,
    trigger: String,
    kind: DestinationKindDto,
    parent_id: Option<String>,
    is_sensitive: bool,
) -> CommandResult<DestinationDto> {
    create_destination_impl(state.inner(), name, trigger, kind, parent_id, is_sensitive)
}

fn get_destination_path_impl(state: &AppState, id: String) -> CommandResult<Vec<DestinationDto>> {
    let id = parse_uuid(&id)?;
    let guard = state.db.lock().unwrap();
    let db = guard.as_ref().ok_or(crate::error::CommandError::Locked)?;
    let chain = destinations::path(db.conn(), id)?;
    Ok(chain.into_iter().map(Into::into).collect())
}

#[tauri::command]
pub fn get_destination_path(state: State<AppState>, id: String) -> CommandResult<Vec<DestinationDto>> {
    get_destination_path_impl(state.inner(), id)
}

fn rename_destination_impl(state: &AppState, id: String, new_name: String) -> CommandResult<()> {
    let id = parse_uuid(&id)?;
    let guard = state.db.lock().unwrap();
    let db = guard.as_ref().ok_or(crate::error::CommandError::Locked)?;
    destinations::rename(db.conn(), id, &new_name)?;
    Ok(())
}

#[tauri::command]
pub fn rename_destination(state: State<AppState>, id: String, new_name: String) -> CommandResult<()> {
    rename_destination_impl(state.inner(), id, new_name)
}

fn reparent_destination_impl(state: &AppState, id: String, new_parent_id: Option<String>) -> CommandResult<()> {
    let id = parse_uuid(&id)?;
    let new_parent_id = new_parent_id.map(|p| parse_uuid(&p)).transpose()?;
    let guard = state.db.lock().unwrap();
    let db = guard.as_ref().ok_or(crate::error::CommandError::Locked)?;
    destinations::reparent(db.conn(), id, new_parent_id)?;
    Ok(())
}

#[tauri::command]
pub fn reparent_destination(
    state: State<AppState>,
    id: String,
    new_parent_id: Option<String>,
) -> CommandResult<()> {
    reparent_destination_impl(state.inner(), id, new_parent_id)
}

fn tombstone_destination_impl(state: &AppState, id: String) -> CommandResult<()> {
    let id = parse_uuid(&id)?;
    let guard = state.db.lock().unwrap();
    let db = guard.as_ref().ok_or(crate::error::CommandError::Locked)?;
    destinations::tombstone(db.conn(), id)?;
    Ok(())
}

#[tauri::command]
pub fn tombstone_destination(state: State<AppState>, id: String) -> CommandResult<()> {
    tombstone_destination_impl(state.inner(), id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use blurt_schema::{migrations, Database, MasterKey};

    use crate::error::CommandError;

    fn unlocked_state() -> AppState {
        let db = Database::open_in_memory(&MasterKey::generate()).unwrap();
        migrations::run(db.conn()).unwrap();
        AppState { db: Mutex::new(Some(db)), keyring: Mutex::new(None) }
    }

    #[test]
    fn creates_a_destination() {
        let state = unlocked_state();

        let dto = create_destination_impl(
            &state,
            "Shopping".to_string(),
            "shop".to_string(),
            DestinationKindDto::List,
            None,
            false,
        )
        .unwrap();

        assert_eq!(dto.name, "Shopping");
        assert_eq!(dto.trigger, "shop");
        assert_eq!(dto.kind, DestinationKindDto::List);
        assert!(!dto.is_system, "is_system must never be frontend-settable");
    }

    #[test]
    fn create_destination_fails_when_locked() {
        let state = AppState::default();

        let err = create_destination_impl(
            &state,
            "Shopping".to_string(),
            "shop".to_string(),
            DestinationKindDto::List,
            None,
            false,
        )
        .unwrap_err();

        assert_eq!(err, CommandError::Locked);
    }

    #[test]
    fn get_destination_path_resolves_a_multi_level_chain() {
        let state = unlocked_state();
        let top = {
            let guard = state.db.lock().unwrap();
            destinations::create(
                guard.as_ref().unwrap().conn(),
                "Shopping",
                "shop",
                blurt_schema::repository::DestinationKind::List,
                None,
                false,
                false,
                0,
            )
            .unwrap()
        };

        let path = get_destination_path_impl(&state, top.id.to_string()).unwrap();
        assert_eq!(path.len(), 1);
        assert_eq!(path[0].name, "Shopping");
    }

    #[test]
    fn get_destination_path_rejects_a_malformed_id() {
        let state = unlocked_state();
        let err = get_destination_path_impl(&state, "not-a-uuid".to_string()).unwrap_err();
        assert_eq!(err, CommandError::InvalidId("not-a-uuid".to_string()));
    }

    #[test]
    fn renames_a_destination() {
        let state = unlocked_state();
        let created = create_destination_impl(
            &state,
            "Shopping".to_string(),
            "shop".to_string(),
            DestinationKindDto::List,
            None,
            false,
        )
        .unwrap();

        rename_destination_impl(&state, created.id.clone(), "Groceries".to_string()).unwrap();

        let path = get_destination_path_impl(&state, created.id).unwrap();
        assert_eq!(path[0].name, "Groceries");
    }

    #[test]
    fn reparents_a_destination() {
        let state = unlocked_state();
        let parent = create_destination_impl(
            &state,
            "Errands".to_string(),
            "errands".to_string(),
            DestinationKindDto::List,
            None,
            false,
        )
        .unwrap();
        let child = create_destination_impl(
            &state,
            "Shopping".to_string(),
            "shop".to_string(),
            DestinationKindDto::List,
            None,
            false,
        )
        .unwrap();

        reparent_destination_impl(&state, child.id.clone(), Some(parent.id.clone())).unwrap();

        let path = get_destination_path_impl(&state, child.id).unwrap();
        assert_eq!(path.len(), 2);
        assert_eq!(path[0].name, "Errands");
        assert_eq!(path[1].name, "Shopping");
    }

    #[test]
    fn tombstones_a_destination() {
        let state = unlocked_state();
        let created = create_destination_impl(
            &state,
            "Shopping".to_string(),
            "shop".to_string(),
            DestinationKindDto::List,
            None,
            false,
        )
        .unwrap();

        tombstone_destination_impl(&state, created.id.clone()).unwrap();

        // path() returns an empty chain for a tombstoned/unknown id, per
        // blurt-schema's own get_by_id-returns-None-not-an-error convention.
        let path = get_destination_path_impl(&state, created.id).unwrap();
        assert!(path.is_empty());
    }
}
