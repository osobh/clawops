//! Node error types

use thiserror::Error;

#[derive(Debug, Error)]
pub enum NodeError {
    #[error("config error: {0}")]
    Config(String),

    #[error("connection error: {0}")]
    Connection(String),

    #[error("auth error: {0}")]
    Auth(String),

    #[error("command error: {0}")]
    Command(String),

    #[error("persistence error: {0}")]
    Persist(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type NodeResult<T> = Result<T, NodeError>;
