use crate::cli::Cli;
use clap::CommandFactory;
use clap_complete::Shell;
use miz_core::error::Result;

pub fn run(shell: Shell) -> Result<()> {
    let mut cmd = Cli::command();
    clap_complete::generate(shell, &mut cmd, "miz", &mut std::io::stdout());
    Ok(())
}
