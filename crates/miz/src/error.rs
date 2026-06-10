use crate::exit;

#[derive(thiserror::Error, Debug)]
pub enum MizError {
    #[error("alpm: {0}")]
    Alpm(#[from] alpm::Error),
    #[error("pacman.conf: {0}")]
    Conf(#[from] alpm_utils::config::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("regex: {0}")]
    Regex(#[from] regex::Error),
    #[error("package '{0}' was not found")]
    PackageNotFound(String),
    #[error("not implemented")]
    NotImplemented,
    #[error("dependency check failed")]
    Deptest,
    #[error("{0} database error(s) found")]
    DatabaseErrors(usize),
    #[error("operation conflict: {0}")]
    BadArgs(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, MizError>;

impl MizError {
    pub fn exit_code(&self) -> i32 {
        match self {
            MizError::Alpm(_) => exit::ALPM,
            MizError::Deptest => exit::DEPTEST,
            MizError::Conf(_)
            | MizError::Io(_)
            | MizError::Regex(_)
            | MizError::PackageNotFound(_)
            | MizError::DatabaseErrors(_)
            | MizError::NotImplemented
            | MizError::BadArgs(_)
            | MizError::Other(_) => exit::GENERIC,
        }
    }
}
