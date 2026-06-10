use crate::cli::Cli;
use crate::error::Result;
use std::path::PathBuf;

pub struct Context {
    pub alpm: alpm::Alpm,
    pub root: PathBuf,
}

/// Build a Context with an optional sync-db file extension override.
///
/// libalpm parses `%FILES%` blocks while reading the syncdb archive, and
/// `configure_alpm` (alpm-utils) registers + loads every repo. The dbext
/// must therefore be set BEFORE `configure_alpm` runs, otherwise repos
/// load from `.db` and `pkg.files()` returns an empty filelist forever
/// (alpm-utils-5.0.0 conf.rs::configure_alpm doc-comment calls this out
/// explicitly: "set the db ext" before registering repos).
///
/// `-F` operations pass `Some(".files")`; everything else passes `None`.
pub fn build_with_dbext(cli: &Cli, dbext: Option<&str>) -> Result<Context> {
    let config_path = cli.config.as_deref().and_then(|p| p.to_str());
    let root_arg = cli.root.as_deref().and_then(|p| p.to_str());
    let conf = alpm_utils::config::Config::with_opts(None, config_path, root_arg)?;

    let root = cli
        .root
        .clone()
        .unwrap_or_else(|| PathBuf::from(&conf.root_dir));
    let dbpath = cli
        .dbpath
        .clone()
        .unwrap_or_else(|| PathBuf::from(&conf.db_path));

    let mut alpm = alpm::Alpm::new(
        root.as_os_str().as_encoded_bytes().to_vec(),
        dbpath.as_os_str().as_encoded_bytes().to_vec(),
    )?;
    if let Some(ext) = dbext {
        alpm.set_dbext(ext);
    }
    alpm_utils::configure_alpm(&mut alpm, &conf)?;

    Ok(Context { alpm, root })
}
