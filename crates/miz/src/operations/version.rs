use crate::common::report::VersionReport;
use crate::error::Result;

pub fn run() -> Result<VersionReport> {
    Ok(VersionReport {
        miz: env!("CARGO_PKG_VERSION").to_string(),
        alpm: alpm::version().to_string(),
    })
}
