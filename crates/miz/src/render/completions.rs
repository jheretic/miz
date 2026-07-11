use crate::cli::Cli;
use crate::error::Result;
use clap::CommandFactory;
use clap_complete::Shell;

pub fn run(shell: Shell) -> Result<()> {
    let mut cmd = Cli::command();
    clap_complete::generate(shell, &mut cmd, "miz", &mut std::io::stdout());
    Ok(())
}
