use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not an XWA archive")]
    BadMagic,
    #[error("corrupt XWA archive: {0}")]
    Corrupt(String),
    #[error("unsupported XWA archive: {0}")]
    Unsupported(String),
    #[error("invalid input: {0}")]
    Invalid(String),
}

pub(crate) fn corrupt(msg: impl Into<String>) -> Error {
    Error::Corrupt(msg.into())
}
