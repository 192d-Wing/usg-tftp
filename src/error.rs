use thiserror::Error;

#[derive(Error, Debug)]
pub enum TftpError {
    #[error("TFTP error: {0}")]
    Tftp(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, TftpError>;
