//! Failures from reading or rewriting an image container.

/// Anything that stops us identifying, reading or rewriting a container.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The bytes are not an image in a format we handle.
    #[error("unsupported image format")]
    UnsupportedFormat,
    /// The container is damaged, or not shaped the way its format requires.
    #[error("corrupt image container: {0}")]
    Corrupt(String),
    /// The format is understood but this particular encoding is not handled.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// The metadata does not fit the format's own size limits.
    #[error("metadata does not fit: {0} bytes")]
    TooLarge(usize),
}

pub type Result<T> = std::result::Result<T, Error>;
