//! One error type for the whole library. It crosses the FFI boundary as a typed exception.

#[derive(Debug, Clone, thiserror::Error, uniffi::Error)]
pub enum ShelfError {
    #[error("invalid input: {msg}")]
    InvalidInput { msg: String },
    #[error("backend `{backend}` failed: {msg}")]
    Backend { backend: String, msg: String },
    #[error("storefront error: {msg}")]
    Storefront { msg: String },
    #[error("io error: {msg}")]
    Io { msg: String },
    #[error("engine is shut down")]
    Closed,
    #[error("internal error: {msg}")]
    Internal { msg: String },
}

impl ShelfError {
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::InvalidInput { msg: msg.into() }
    }
    pub fn backend(backend: impl Into<String>, msg: impl ToString) -> Self {
        Self::Backend { backend: backend.into(), msg: msg.to_string() }
    }
    pub fn storefront(msg: impl ToString) -> Self {
        Self::Storefront { msg: msg.to_string() }
    }
    pub fn internal(msg: impl ToString) -> Self {
        Self::Internal { msg: msg.to_string() }
    }
}

impl From<std::io::Error> for ShelfError {
    fn from(e: std::io::Error) -> Self {
        Self::Io { msg: e.to_string() }
    }
}

/// A foreign (Swift/Kotlin/Python) callback threw something that is not a `ShelfError`.
impl From<uniffi::UnexpectedUniFFICallbackError> for ShelfError {
    fn from(e: uniffi::UnexpectedUniFFICallbackError) -> Self {
        Self::Backend { backend: "foreign-callback".into(), msg: e.reason }
    }
}

pub type Result<T> = std::result::Result<T, ShelfError>;
