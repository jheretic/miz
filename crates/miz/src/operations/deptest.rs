use crate::common::report::DeptestReport;
use crate::config::Context;
use crate::error::Result;

pub use crate::cli::args::deptest::Args;

pub fn run(args: Args, ctx: &Context) -> Result<DeptestReport> {
    let mut missing = Vec::new();
    if args.deps.is_empty() {
        return Ok(DeptestReport { missing });
    }

    let pkgs = ctx.alpm.localdb().pkgs();
    for dep in &args.deps {
        if pkgs.find_satisfier(dep.as_str()).is_none() {
            missing.push(dep.clone());
        }
    }
    Ok(DeptestReport { missing })
}
