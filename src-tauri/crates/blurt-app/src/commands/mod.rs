//! Tauri command handlers. Thin wrappers over `blurt_schema::repository` —
//! see `docs/MODULE_01_ARCHITECTURE.md` §3 and this crate's `CLAUDE.md`.

pub mod destinations;
pub mod edits;
pub mod items;
pub mod router;
pub mod search;
pub mod vault;

use uuid::Uuid;

use crate::error::{CommandError, CommandResult};

/// Parses an IPC-supplied id string. IDs cross the boundary as strings since
/// `Uuid` isn't itself a JSON primitive; a parse failure means the frontend
/// sent something that was never a valid id, not a "not found" case.
pub(crate) fn parse_uuid(s: &str) -> CommandResult<Uuid> {
    Uuid::parse_str(s).map_err(|_| CommandError::InvalidId(s.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_valid_uuid() {
        let id = Uuid::new_v4();
        assert_eq!(parse_uuid(&id.to_string()).unwrap(), id);
    }

    #[test]
    fn rejects_a_non_uuid_string() {
        let err = parse_uuid("not-a-uuid").unwrap_err();
        assert_eq!(err, CommandError::InvalidId("not-a-uuid".to_string()));
    }
}
