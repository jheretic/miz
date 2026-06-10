use crate::error::Result;

pub fn run() -> Result<()> {
    let miz_ver = env!("CARGO_PKG_VERSION");
    let alpm_ver = alpm::version();
    println!(" .--.                  miz v{miz_ver} - libalpm v{alpm_ver}");
    println!("/ _.-' .-.  .-.  .-.   Archetype Linux package manager");
    println!("\\  '-. '-'  '-'  '-'");
    println!(" '--'");
    println!("                       This program may be freely redistributed under");
    println!("                       the terms of the GNU General Public License.");
    Ok(())
}
