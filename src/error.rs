//! The one error type both codecs return.

/// Why a stream could not be decoded, or an encoder not built.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The data breaks the format: a bad CRC, a reserved value, a field out
    /// of range, a frame that runs past the end of its packet. The message
    /// names the codec (`flac: …`, `alac: …`).
    #[error("invalid stream: {0}")]
    Invalid(String),
    /// Well-formed input this crate does not take — an ALAC cookie of
    /// another version or bit depth, a coupling channel element — or an
    /// encoder asked for a channel count, depth or rate it cannot code.
    #[error("unsupported: {0}")]
    Unsupported(String),
}

impl Error {
    /// The message, without the variant's prefix.
    pub fn message(&self) -> &str {
        match self {
            Error::Invalid(m) | Error::Unsupported(m) => m,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
