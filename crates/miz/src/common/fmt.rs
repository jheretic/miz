//! Pure formatters (no I/O, no color) usable by BOTH the render layer and the
//! operations that still format inline during the transition (sync, images).
//! Kept in `common/` so operations never import `render`; `render/fmt.rs`
//! re-exports these for the render layer.

use alpm::Pkg;

pub fn join_list_str(list: alpm::AlpmList<&str>, none: &str) -> String {
    let items: Vec<&str> = list.iter().collect();
    if items.is_empty() {
        none.to_string()
    } else {
        items.join("  ")
    }
}

pub fn join_dep_list(list: alpm::AlpmList<&alpm::Dep>, none: &str) -> String {
    let items: Vec<String> = list.iter().map(|d| d.to_string()).collect();
    if items.is_empty() {
        none.to_string()
    } else {
        items.join("  ")
    }
}

pub fn join_string_list<I: IntoIterator<Item = String>>(it: I, none: &str) -> String {
    let items: Vec<String> = it.into_iter().collect();
    if items.is_empty() {
        none.to_string()
    } else {
        items.join("  ")
    }
}

pub fn join_optdeps(pkg: &Pkg, none: &str) -> String {
    let items: Vec<String> = pkg.optdepends().iter().map(|d| d.to_string()).collect();
    if items.is_empty() {
        none.to_string()
    } else {
        items.join("\n                     ")
    }
}

pub fn format_size(bytes: i64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u + 1 < UNITS.len() {
        v /= 1024.0;
        u += 1;
    }
    format!("{:.2} {}", v, UNITS[u])
}

pub fn format_date(secs: i64) -> String {
    chrono_like(secs)
}

fn chrono_like(secs: i64) -> String {
    let days_per_month = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut secs = secs;
    // Reject out-of-range values (negative, or beyond year 9999) rather than
    // looping ~292 billion times. Image-db timestamps are parsed from untrusted
    // text, so a bogus value must not hang -Qi.
    if !(0..=253_402_300_799).contains(&secs) {
        return secs.to_string();
    }
    let h = ((secs / 3600) % 24) as u32;
    let m = ((secs / 60) % 60) as u32;
    let s = (secs % 60) as u32;
    let mut days = secs / 86400;
    secs %= 86400;
    let _ = secs;
    let mut year: i64 = 1970;
    loop {
        let leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
        let yd = if leap { 366 } else { 365 };
        if days >= yd {
            days -= yd;
            year += 1;
        } else {
            break;
        }
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
    let mut month = 0usize;
    while month < 12 {
        let mut dm = days_per_month[month] as i64;
        if month == 1 && leap {
            dm = 29;
        }
        if days >= dm {
            days -= dm;
            month += 1;
        } else {
            break;
        }
    }
    let day = days + 1;
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        year,
        month + 1,
        day,
        h,
        m,
        s
    )
}

pub fn format_validation(v: alpm::PackageValidation) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if v.contains(alpm::PackageValidation::NONE) {
        parts.push("None");
    }
    if v.contains(alpm::PackageValidation::MD5SUM) {
        parts.push("MD5 Sum");
    }
    if v.contains(alpm::PackageValidation::SHA256SUM) {
        parts.push("SHA-256 Sum");
    }
    if v.contains(alpm::PackageValidation::SIGNATURE) {
        parts.push("Signature");
    }
    if parts.is_empty() {
        "Unknown".to_string()
    } else {
        parts.join("  ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrono_like_rejects_out_of_range() {
        // negative and absurd values are echoed verbatim, not looped over
        assert_eq!(chrono_like(-5), "-5");
        assert_eq!(chrono_like(i64::MAX), i64::MAX.to_string());
        // a normal value formats
        assert!(chrono_like(0).starts_with("1970-01-01"));
    }
}
