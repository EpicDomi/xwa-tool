use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a ZIM file (bad magic number)")]
    BadMagic,
    #[error("corrupt ZIM file: {0}")]
    Corrupt(String),
    #[error("unsupported ZIM feature: {0}")]
    Unsupported(String),
}
