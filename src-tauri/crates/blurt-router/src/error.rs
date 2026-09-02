//! Error type for `blurt-router`.

use thiserror::Error;

/// Everything that can go wrong while resolving a capture to a destination.
///
/// Deliberately thin: routing itself has no failure modes of its own — an
/// unmatched chain segment or an unrecognizable sentence is a *decision*
/// ([`crate::resolve::RoutingDecision`]), not an error. The only way routing
/// fails is the storage read underneath it failing.
#[derive(Debug, Error)]
pub enum RouterError {
    #[error("storage error: {0}")]
    Schema(#[from] blurt_schema::SchemaError),
}

pub type Result<T> = std::result::Result<T, RouterError>;
