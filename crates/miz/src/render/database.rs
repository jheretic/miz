use crate::common::report::DbReport;

pub fn render(r: &DbReport) {
    match r {
        DbReport::Check {
            problems,
            count,
            quiet,
        } => {
            for p in problems {
                eprintln!("{p}");
            }
            if *count == 0 && !*quiet {
                println!("No database errors have been found!");
            }
        }
        DbReport::SetReason { confirmations, .. } => {
            for line in confirmations {
                eprintln!("{line}");
            }
        }
    }
}
