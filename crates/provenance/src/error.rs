//! Error type, kept deliberately close to the codes the JS layer exposes.

/// Stable machine-readable codes; these cross the wasm boundary and are part of
/// the public API, so renaming one is a breaking change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    UnsupportedFormat,
    MalformedXmp,
    ContainerCorrupt,
    SignerUnauthorized,
    SignerRejected,
    Network,
    InvalidSignerInfo,
    MalformedManifest,
    C2pa,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedFormat => "unsupportedFormat",
            Self::MalformedXmp => "malformedXmp",
            Self::ContainerCorrupt => "containerCorrupt",
            Self::SignerUnauthorized => "signerUnauthorized",
            Self::SignerRejected => "signerRejected",
            Self::Network => "networkError",
            Self::InvalidSignerInfo => "invalidSignerInfo",
            Self::MalformedManifest => "malformedManifest",
            Self::C2pa => "c2paError",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unsupported image format")]
    UnsupportedFormat,

    #[error("malformed XMP: {0}")]
    MalformedXmp(String),

    #[error("corrupt image container: {0}")]
    ContainerCorrupt(String),

    #[error("signing service rejected the credentials")]
    SignerUnauthorized,

    #[error("signing service rejected the request (HTTP {status}): {message}")]
    SignerRejected { status: u16, message: String },

    #[error("network error: {0}")]
    Network(String),

    #[error("invalid signer info from the service: {0}")]
    InvalidSignerInfo(String),

    #[error("malformed manifest definition: {0}")]
    MalformedManifest(String),

    #[error("c2pa error: {0}")]
    C2pa(#[from] c2pa::Error),
}

impl From<xmp::XmpError> for Error {
    fn from(e: xmp::XmpError) -> Self {
        Self::MalformedXmp(e.to_string())
    }
}

impl From<xmp_image::Error> for Error {
    fn from(e: xmp_image::Error) -> Self {
        match e {
            xmp_image::Error::UnsupportedFormat => Self::UnsupportedFormat,
            other => Self::ContainerCorrupt(other.to_string()),
        }
    }
}

impl Error {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::UnsupportedFormat => ErrorCode::UnsupportedFormat,
            Self::MalformedXmp(_) => ErrorCode::MalformedXmp,
            Self::ContainerCorrupt(_) => ErrorCode::ContainerCorrupt,
            Self::SignerUnauthorized => ErrorCode::SignerUnauthorized,
            Self::SignerRejected { .. } => ErrorCode::SignerRejected,
            Self::Network(_) => ErrorCode::Network,
            Self::InvalidSignerInfo(_) => ErrorCode::InvalidSignerInfo,
            Self::MalformedManifest(_) => ErrorCode::MalformedManifest,
            Self::C2pa(_) => ErrorCode::C2pa,
        }
    }
}

/// `c2pa`'s error type is opaque to us, so the original message is folded into
/// `BadParam` rather than dropped — losing it would make signer failures
/// impossible to diagnose from JS.
impl From<Error> for c2pa::Error {
    fn from(e: Error) -> Self {
        c2pa::Error::BadParam(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
