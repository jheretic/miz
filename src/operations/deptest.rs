use crate::config::Context;
use crate::error::{MizError, Result};

pub use crate::cli::args::deptest::Args;

pub fn run(args: Args, ctx: &Context) -> Result<()> {
    if args.deps.is_empty() {
        return Ok(());
    }

    let pkgs = ctx.alpm.localdb().pkgs();
    let mut missing = false;
    for dep in &args.deps {
        if pkgs.find_satisfier(dep.as_str()).is_none() {
            println!("{dep}");
            missing = true;
        }
    }

    if missing {
        Err(MizError::Deptest)
    } else {
        Ok(())
    }
}
