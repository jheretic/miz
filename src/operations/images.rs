use crate::error::{MizError, Result};

pub use crate::cli::args::images::Args;

pub fn run(_args: Args) -> Result<()> {
    eprintln!("miz: -I/--images is not yet implemented");
    Err(MizError::NotImplemented)
}
