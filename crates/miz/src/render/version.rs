use miz_core::common::report::VersionReport;

pub fn render(r: &VersionReport) {
    let (miz_ver, alpm_ver) = (&r.miz, &r.alpm);
    println!(" .--.                  miz v{miz_ver} - libalpm v{alpm_ver}");
    println!("/ _.-' .-.  .-.  .-.   Archetype Linux package manager");
    println!("\\  '-. '-'  '-'  '-'");
    println!(" '--'");
    println!("                       This program may be freely redistributed under");
    println!("                       the terms of the GNU General Public License.");
}
