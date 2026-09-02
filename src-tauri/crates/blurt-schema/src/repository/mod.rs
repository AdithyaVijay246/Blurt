//! Repository/CRUD layer — `MODULE_02_SCHEMA.md` §1–2.
//!
//! Storage primitives only, per `crate` docs: this module reads and writes
//! `destinations`/`items`/`edits` rows and nothing else. Policy (auth gating,
//! idle timers, which destination captures land in) belongs to `blurt-app`.

pub mod destinations;
pub mod edits;
pub mod indexing;
pub mod items;

pub use destinations::{Destination, DestinationKind, RANDOM_THOUGHTS_ID, UNSORTED_ID};
pub use edits::Edit;
pub use indexing::items_for_indexing;
pub use items::Item;

/// Current time as Unix milliseconds, matching the `INTEGER` timestamp
/// convention in `migrations/0001_initial.sql`.
pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is before the Unix epoch")
        .as_millis() as i64
}
