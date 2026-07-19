use miz_core::common::report::DeptestReport;

pub fn render(r: &DeptestReport) {
    for dep in &r.missing {
        println!("{dep}");
    }
}
