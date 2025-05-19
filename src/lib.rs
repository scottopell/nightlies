#![warn(clippy::pedantic)]

use thiserror::Error;
use tokio::task::JoinError;

#[derive(Error, Debug)]
pub enum NightlyError {
    #[error("Error while fetching tags from docker registry: {0}")]
    FetchError(#[from] reqwest::Error),

    #[error("Error while interacting with tag cache file: {0}")]
    FileError(#[from] std::io::Error),

    #[error("Json error: {0}")]
    JSONError(#[from] serde_json::Error),

    #[error("Join error: {0}")]
    JoinError(#[from] JoinError),

    #[error("Parse Error: {0}")]
    DateParseError(String),

    #[error("Generic Error: {0}")]
    GenericError(String),

    #[error("Git Error: {0}")]
    GitError(String),
}

pub mod diff;
pub mod nightly;
pub mod repo;
