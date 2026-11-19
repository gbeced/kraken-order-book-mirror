use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("{0}")]
    Network(String),
    #[error("{0}")]
    Parse(String),
    #[error("{0}")]
    Sign(String),
    #[error("Invalid {name}")]
    InvalidParameter { name: String, msg: String },
    #[error("{0}")]
    Other(String),
    #[error("{0}")]
    UnitConversion(String),
    #[error("{0}")]
    IPCChannelClosed(String),
    #[error("{0}")]
    UpdateFailed(String),
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Parse(e.to_string())
    }
}

impl From<tokio_tungstenite::tungstenite::Error> for Error {
    fn from(error: tokio_tungstenite::tungstenite::Error) -> Self {
        Error::Network(error.to_string())
    }
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        Error::Network(error.to_string())
    }
}
