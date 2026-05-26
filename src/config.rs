use crate::cli::Cli;
use crate::error::Result;
use std::path::PathBuf;

pub struct Context {
    pub alpm: alpm::Alpm,
    pub root: PathBuf,
}

pub fn build(cli: &Cli) -> Result<Context> {
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
    alpm_utils::configure_alpm(&mut alpm, &conf)?;

    Ok(Context { alpm, root })
}
