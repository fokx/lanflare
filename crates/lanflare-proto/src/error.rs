//! Protocol error types.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("Serialization or deserialization failed: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Invalid protocol message: {0}")]
    InvalidMessage(String),
}
