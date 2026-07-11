use crate::common::exit;
use std::path::PathBuf;

#[derive(thiserror::Error, Debug)]
pub enum MizError {
    // libalpm's own messages are already user-facing ("could not find package
    // ...", "unable to lock database", etc.); the top-level handler prints the
    // "error:" prefix, so an extra "alpm:" tag would only add noise.
    #[error("{0}")]
    Alpm(#[from] alpm::Error),
    // std::io::Error's Display is self-descriptive ("No such file or directory
    // (os error 2)"); no "io:" tag needed after the "error:" prefix.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("invalid search pattern: {0}")]
    Regex(#[from] regex::Error),
    #[error("could not parse config {path}: {source}")]
    Toml {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("package '{0}' was not found")]
    PackageNotFound(String),
    #[error("this operation is not implemented")]
    NotImplemented,
    #[error("image update failed: {0}")]
    Sysupdate(String),
    #[error("could not reach the image service over D-Bus: {0}")]
    Dbus(#[from] zbus::Error),
    #[error("dependency check failed")]
    Deptest,
    #[error("{0} database error(s) found")]
    DatabaseErrors(usize),
    #[error("conflicting options: {0}")]
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
            MizError::Io(_)
            | MizError::Regex(_)
            | MizError::Toml { .. }
            | MizError::PackageNotFound(_)
            | MizError::DatabaseErrors(_)
            | MizError::NotImplemented
            | MizError::Sysupdate(_)
            | MizError::Dbus(_)
            | MizError::BadArgs(_)
            | MizError::Other(_) => exit::GENERIC,
        }
    }
}
