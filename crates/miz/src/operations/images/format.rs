//! Pacman-style rendering for `-Il`/`-Ii`/`-Ig`. Pure string builders so the
//! shapes can be golden-tested without a bus. Reuses the `pub(crate)`
//! formatters in `query.rs` (`format_size`/`format_date`) rather than
//! duplicating them.

use super::describe::{Describe, Feature};
use crate::operations::query::{format_date, format_size};

/// One `-Il` row. Mirrors `sync::sync_list`'s `{repo} {pkg} {ver}{suffix}`:
/// here `{component} {version}{suffix}` with `[installed]`/`[newest]` markers.
/// `quiet` prints the bare version string (like `-Slq`).
pub fn list_line(
    component: &str,
    version: &str,
    installed: bool,
    newest: bool,
    quiet: bool,
) -> String {
    if quiet {
        return version.to_string();
    }
    let mut suffix = String::new();
    if installed {
        suffix.push_str(" [installed]");
    }
    if newest {
        suffix.push_str(" [newest]");
    }
    format!("{component} {version}{suffix}")
}

/// `-Ii` block using the `{:<19}: {}` label idiom from `query::print_sync_info`.
/// `verbose` (`-ii`) appends changelog/contents. Defensive about which keys the
/// Describe payload actually carried (PLAN's weakest assumption).
pub fn info_block(component: &str, d: &Describe, verbose: bool) -> String {
    let mut out = String::new();
    let mut label = |k: &str, v: &str| out.push_str(&format!("{:<19}: {}\n", k, v));

    label("Component", component);
    label("Version", d.version.as_deref().unwrap_or("None"));
    label("Newest", yesno(d.newest));
    label("Available", yesno(d.available));
    label("Installed", yesno(d.installed));
    label("Obsolete", yesno(d.obsolete));
    label("Incomplete", yesno(d.incomplete));
    if let Some(t) = d.extra_str("type") {
        label("Type", &t);
    }

    // Size/timestamp keys are not in the documented typed set; pull them
    // defensively from `extra` and reuse the query.rs formatters when present.
    if let Some(size) = d.extra_i64("size") {
        label("Download Size", &format_size(size));
    }
    if let Some(ts) = d.extra_i64("timestamp") {
        label("Build Date", &format_date(ts));
    }

    if verbose {
        if let Some(cl) = d.changelog.as_ref() {
            label("Changelog", &value_to_text(cl));
        }
        if let Some(c) = d.contents.as_ref() {
            label("Contents", &value_to_text(c));
        }
    }

    out
}

/// `-If component/<feature>` block, same `{:<19}: {}` label idiom as `-Ii`.
pub fn feature_block(id: &str, f: &Feature) -> String {
    let mut out = String::new();
    let mut label = |k: &str, v: &str| out.push_str(&format!("{:<19}: {}\n", k, v));

    label("Feature", id);
    label("Name", f.name.as_deref().unwrap_or("None"));
    label("Enabled", yesno(f.enabled));
    if let Some(d) = f.description.as_deref() {
        label("Description", d);
    }
    if let Some(u) = f.documentation_url.as_deref() {
        label("Documentation", u);
    }
    if let Some(u) = f.appstream_url.as_deref() {
        label("AppStream", u);
    }
    if let Some(t) = f.transfers.as_ref() {
        label("Transfers", &value_to_text(t));
    }
    out
}

/// One `-Ig` row: `class name` per `Manager.ListTargets` entry.
pub fn component_line(class: &str, name: &str, quiet: bool) -> String {
    if quiet {
        name.to_string()
    } else {
        format!("{class} {name}")
    }
}

fn yesno(b: Option<bool>) -> &'static str {
    match b {
        Some(true) => "Yes",
        Some(false) => "No",
        None => "Unknown",
    }
}

/// Render a raw JSON value (changelog/contents, whose shape varies by systemd
/// version) into a single human-ish string without assuming structure.
fn value_to_text(v: &serde_json::Value) -> String {
    use serde_json::Value;
    match v {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(value_to_text)
            .collect::<Vec<_>>()
            .join(", "),
        Value::Null => "None".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::images::describe::{Describe, Feature};

    #[test]
    fn list_line_markers() {
        assert_eq!(
            list_line("host", "2.3", true, false, false),
            "host 2.3 [installed]"
        );
        assert_eq!(
            list_line("host", "2.4", false, true, false),
            "host 2.4 [newest]"
        );
        assert_eq!(
            list_line("host", "2.4", true, true, false),
            "host 2.4 [installed] [newest]"
        );
        assert_eq!(list_line("host", "2.4", false, false, false), "host 2.4");
    }

    #[test]
    fn list_line_quiet_is_bare_version() {
        assert_eq!(list_line("host", "2.3", true, true, true), "2.3");
    }

    #[test]
    fn component_line_shapes() {
        assert_eq!(component_line("url-file", "host", false), "url-file host");
        assert_eq!(component_line("url-file", "host", true), "host");
    }

    #[test]
    fn info_block_golden() {
        let d = Describe::parse(
            r#"{"version":"2.3","newest":true,"available":true,"installed":false,"obsolete":false,"incomplete":false}"#,
        )
        .unwrap();
        let got = info_block("host", &d, false);
        let expected = "\
Component          : host
Version            : 2.3
Newest             : Yes
Available          : Yes
Installed          : No
Obsolete           : No
Incomplete         : No
";
        assert_eq!(got, expected);
    }

    #[test]
    fn info_block_unknown_fields_render_unknown() {
        let d = Describe::parse(r#"{"version":"1.0"}"#).unwrap();
        let got = info_block("host", &d, false);
        assert!(got.contains("Newest             : Unknown"));
        assert!(got.contains("Version            : 1.0"));
    }

    #[test]
    fn info_block_verbose_adds_changelog_contents() {
        let d = Describe::parse(r#"{"version":"1.0","changelog":"fixes","contents":["a","b"]}"#)
            .unwrap();
        let got = info_block("host", &d, true);
        assert!(got.contains("Changelog          : fixes"));
        assert!(got.contains("Contents           : a, b"));
    }

    #[test]
    fn feature_block_golden() {
        let f = Feature::parse(
            r#"{"name":"experimental","enabled":true,"description":"edge","documentationUrl":"https://d"}"#,
        )
        .unwrap();
        let got = feature_block("experimental", &f);
        let expected = "\
Feature            : experimental
Name               : experimental
Enabled            : Yes
Description        : edge
Documentation      : https://d
";
        assert_eq!(got, expected);
    }

    #[test]
    fn feature_block_minimal() {
        let f = Feature::parse(r#"{"name":"x"}"#).unwrap();
        let got = feature_block("x", &f);
        assert!(got.contains("Enabled            : Unknown"));
        assert!(!got.contains("Description"));
    }

    #[test]
    fn info_block_extra_size_uses_format_size() {
        let d = Describe::parse(r#"{"version":"1.0","size":1048576}"#).unwrap();
        let got = info_block("host", &d, false);
        assert!(got.contains("Download Size      : 1.00 MiB"));
    }
}
