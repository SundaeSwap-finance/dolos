pub use dolos_core::*;

use dolos_core::config::StorageVersion;

use miette::Diagnostic;
use std::fmt::Display;
use thiserror::Error;

#[derive(Error, Debug, Diagnostic)]
pub enum Error {
    #[error("io error: {0}")]
    IO(#[from] std::io::Error),

    #[error("configuration error: {0}")]
    ConfigError(String),

    #[error("client error: {0}")]
    ClientError(String),

    #[error("parse error: {0}")]
    ParseError(String),

    #[error("server error: {0}")]
    ServerError(String),

    #[error("storage error: {0}")]
    StorageError(String),

    /// The configuration declares a storage version this binary does not read.
    ///
    /// Both versions are carried rather than formatted away, because the
    /// comparison is between two configuration strings and a caller that wants
    /// to report or repair it needs the pair.
    #[error(
        "the configuration declares storage version `{declared}` and this dolos reads \
         `{supported}`. That is a comparison of two configuration strings, and no store was \
         read. If the stores were written by a dolos that reads `{supported}`, set \
         `storage.version` to `{supported}`. Running `dolos init` writes a fresh configuration \
         and bootstraps again, which discards whatever the stores hold. The bootstrap guide is \
         at {guide}"
    )]
    StorageVersionMismatch {
        declared: StorageVersion,
        supported: StorageVersion,
        guide: &'static str,
    },

    #[error("wal error: {0}")]
    WalError(#[from] WalError),

    #[error("chain error: {0}")]
    ArchiveError(#[from] ArchiveError),

    #[error("state error: {0}")]
    StateError(#[from] StateError),

    #[error("mempool error: {0}")]
    MempoolError(#[from] MempoolError),

    #[error("{0}")]
    Message(String),

    #[error("{0}")]
    Custom(String),
}

impl Error {
    pub fn config(text: impl Display) -> Error {
        Error::ConfigError(text.to_string())
    }

    pub fn client(error: impl Display) -> Error {
        Error::ClientError(error.to_string())
    }

    pub fn parse(error: impl Display) -> Error {
        Error::ParseError(error.to_string())
    }

    pub fn server(error: impl Display) -> Error {
        Error::ServerError(error.to_string())
    }

    pub fn message(text: impl Into<String>) -> Error {
        Error::Message(text.into())
    }

    pub fn custom(error: impl Display) -> Error {
        Error::Custom(error.to_string())
    }
}

impl From<Box<dyn std::error::Error>> for Error {
    fn from(err: Box<dyn std::error::Error>) -> Self {
        Error::custom(err)
    }
}

#[derive(Clone, Default)]
pub struct CancelTokenImpl(pub tokio_util::sync::CancellationToken);

impl CancelToken for CancelTokenImpl {
    async fn cancelled(&self) {
        self.0.cancelled().await;
    }
}
