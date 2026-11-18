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
}
