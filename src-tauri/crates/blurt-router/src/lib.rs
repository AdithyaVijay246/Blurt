//! Module 3 — `@` parsing and natural-language routing.
//!
//! See `docs/MODULE_03_ROUTER.md`.
//!
//! This crate **decides**; it never writes. [`resolve::route`] returns a
//! [`resolve::RoutingDecision`] and leaves the actual `items::capture` /
//! `destinations::create` calls — and any event emission — to `blurt-app`.
//! That keeps routing testable against a `destinations` table alone, with no
//! `items` rows in sight, and keeps cross-crate orchestration in the one crate
//! Module 1 designates as the seam.

pub mod candidates;
pub mod chain;
pub mod error;
pub mod nl;
pub mod resolve;
pub mod voice;

pub use error::{Result, RouterError};
pub use resolve::{route, Routing, RoutingDecision};
