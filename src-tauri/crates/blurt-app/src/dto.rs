//! IPC-facing mirrors of `blurt_schema`'s domain structs.
//!
//! Deliberately separate types, not `serde`/`specta` derives bolted onto
//! `blurt_schema` itself — that crate stays storage-primitives-only per its
//! own `CLAUDE.md`. UUIDs cross the boundary as strings.

use serde::{Deserialize, Serialize};

use blurt_schema::repository::{Destination, DestinationKind, Edit, Item};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DestinationKindDto {
    List,
    Note,
}

impl From<DestinationKind> for DestinationKindDto {
    fn from(kind: DestinationKind) -> Self {
        match kind {
            DestinationKind::List => DestinationKindDto::List,
            DestinationKind::Note => DestinationKindDto::Note,
        }
    }
}

impl From<DestinationKindDto> for DestinationKind {
    fn from(kind: DestinationKindDto) -> Self {
        match kind {
            DestinationKindDto::List => DestinationKind::List,
            DestinationKindDto::Note => DestinationKind::Note,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DestinationDto {
    pub id: String,
    pub parent_id: Option<String>,
    pub name: String,
    pub trigger: String,
    pub kind: DestinationKindDto,
    pub is_system: bool,
    pub is_sensitive: bool,
    pub sort_order: i64,
    pub created_at: i64,
    pub deleted_at: Option<i64>,
}

impl From<Destination> for DestinationDto {
    fn from(d: Destination) -> Self {
        DestinationDto {
            id: d.id.to_string(),
            parent_id: d.parent_id.map(|p| p.to_string()),
            name: d.name,
            trigger: d.trigger,
            kind: d.kind.into(),
            is_system: d.is_system,
            is_sensitive: d.is_sensitive,
            sort_order: d.sort_order,
            created_at: d.created_at,
            deleted_at: d.deleted_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemDto {
    pub id: String,
    pub destination_id: String,
    pub original_text: String,
    pub current_text: String,
    pub checked: Option<bool>,
    pub created_at: i64,
    pub deleted_at: Option<i64>,
}

impl From<Item> for ItemDto {
    fn from(i: Item) -> Self {
        ItemDto {
            id: i.id.to_string(),
            destination_id: i.destination_id.to_string(),
            original_text: i.original_text,
            current_text: i.current_text,
            checked: i.checked,
            created_at: i.created_at,
            deleted_at: i.deleted_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditDto {
    pub id: String,
    pub item_id: String,
    pub text: String,
    pub edited_at: i64,
}

impl From<Edit> for EditDto {
    fn from(e: Edit) -> Self {
        EditDto {
            id: e.id.to_string(),
            item_id: e.item_id.to_string(),
            text: e.text,
            edited_at: e.edited_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn destination_kind_maps_list_and_note() {
        assert_eq!(DestinationKindDto::from(DestinationKind::List), DestinationKindDto::List);
        assert_eq!(DestinationKindDto::from(DestinationKind::Note), DestinationKindDto::Note);
    }

    #[test]
    fn destination_kind_dto_maps_back_to_domain_kind() {
        assert_eq!(DestinationKind::from(DestinationKindDto::List), DestinationKind::List);
        assert_eq!(DestinationKind::from(DestinationKindDto::Note), DestinationKind::Note);
    }

    #[test]
    fn destination_converts_with_stringified_ids() {
        let id = Uuid::new_v4();
        let parent_id = Uuid::new_v4();
        let d = Destination {
            id,
            parent_id: Some(parent_id),
            name: "Shopping".to_string(),
            trigger: "shop".to_string(),
            kind: DestinationKind::List,
            is_system: false,
            is_sensitive: false,
            sort_order: 0,
            created_at: 123,
            deleted_at: None,
        };
        let dto: DestinationDto = d.into();
        assert_eq!(dto.id, id.to_string());
        assert_eq!(dto.parent_id, Some(parent_id.to_string()));
        assert_eq!(dto.name, "Shopping");
        assert_eq!(dto.kind, DestinationKindDto::List);
    }

    #[test]
    fn item_converts_with_stringified_ids() {
        let id = Uuid::new_v4();
        let destination_id = Uuid::new_v4();
        let i = Item {
            id,
            destination_id,
            original_text: "buy milk".to_string(),
            current_text: "buy milk".to_string(),
            checked: Some(false),
            created_at: 123,
            deleted_at: None,
        };
        let dto: ItemDto = i.into();
        assert_eq!(dto.id, id.to_string());
        assert_eq!(dto.destination_id, destination_id.to_string());
        assert_eq!(dto.checked, Some(false));
    }

    #[test]
    fn edit_converts_with_stringified_ids() {
        let id = Uuid::new_v4();
        let item_id = Uuid::new_v4();
        let e = Edit { id, item_id, text: "oat milk".to_string(), edited_at: 456 };
        let dto: EditDto = e.into();
        assert_eq!(dto.id, id.to_string());
        assert_eq!(dto.item_id, item_id.to_string());
        assert_eq!(dto.text, "oat milk");
    }
}
