use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("{0}")]
    Network(String),
    #[error("{0}")]
    Parse(String),
    #[error("{0}")]
    Sign(String),
    #[error("{0}")]
    InvalidParameter(String),
    #[error("{0}")]
    Other(String),
    #[error("{0}")]
    UnitConversion(String),
    // #[error("data store disconnected")]
    // Disconnect(#[from] io::Error),
    // #[error("the data for key `{0}` is not available")]
    // Redaction(String),
    // #[error("invalid header (expected {expected:?}, found {found:?})")]
    // InvalidHeader {
    //     expected: String,
    //     found: String,
    // },
    // #[error("unknown data store error")]
    // Unknown,
}
