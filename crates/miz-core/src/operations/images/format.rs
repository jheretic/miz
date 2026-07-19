//! Core field extraction for `-Ii`/`-If`: turns a parsed `Describe`/`Feature`
//! into an ordered list of `InfoField`s (the label/value data). This is NOT
//! presentation — the `{:<19}: {}` layout and coloring live in `render`; here
//! we only decide WHICH fields carry WHICH values, defensively (PLAN's weakest
//! assumption about the payload key set). Reuses `common::fmt`
//! (`format_size`/`format_date`) rather than duplicating them, and never
//! imports `render`.

use super::describe::{Describe, Feature};
use crate::common::fmt::{format_date, format_size};
use crate::common::report::InfoField;

fn label(fields: &mut Vec<InfoField>, key: &str, value: &str) {
    fields.push(InfoField::Label {
        key: key.to_string(),
        value: value.to_string(),
    });
}

/// `-Ii` fields. `verbose` (`-ii`) appends changelog/contents. Defensive about
/// which keys the Describe payload actually carried.
pub fn describe_fields(component: &str, d: &Describe, verbose: bool) -> Vec<InfoField> {
    let mut fields = Vec::new();

    label(&mut fields, "Component", component);
    label(
        &mut fields,
        "Version",
        d.version.as_deref().unwrap_or("None"),
    );
    label(&mut fields, "Newest", yesno(d.newest));
    label(&mut fields, "Available", yesno(d.available));
    label(&mut fields, "Installed", yesno(d.installed));
    label(&mut fields, "Obsolete", yesno(d.obsolete));
    label(&mut fields, "Incomplete", yesno(d.incomplete));
    if let Some(t) = d.extra_str("type") {
        label(&mut fields, "Type", &t);
    }

    // Size/timestamp keys are not in the documented typed set; pull them
    // defensively from `extra` and reuse the query.rs formatters when present.
    if let Some(size) = d.extra_i64("size") {
        label(&mut fields, "Download Size", &format_size(size));
    }
    if let Some(ts) = d.extra_i64("timestamp") {
        label(&mut fields, "Build Date", &format_date(ts));
    }

    if verbose {
        if let Some(cl) = d.changelog.as_ref() {
            label(&mut fields, "Changelog", &value_to_text(cl));
        }
        if let Some(c) = d.contents.as_ref() {
            label(&mut fields, "Contents", &value_to_text(c));
        }
    }

    fields
}

/// `-If component/<feature>` fields, same key set as before.
pub fn feature_fields(id: &str, f: &Feature) -> Vec<InfoField> {
    let mut fields = Vec::new();

    label(&mut fields, "Feature", id);
    label(&mut fields, "Name", f.name.as_deref().unwrap_or("None"));
    label(&mut fields, "Enabled", yesno(f.enabled));
    if let Some(d) = f.description.as_deref() {
        label(&mut fields, "Description", d);
    }
    if let Some(u) = f.documentation_url.as_deref() {
        label(&mut fields, "Documentation", u);
    }
    if let Some(u) = f.appstream_url.as_deref() {
        label(&mut fields, "AppStream", u);
    }
    if let Some(t) = f.transfers.as_ref() {
        label(&mut fields, "Transfers", &value_to_text(t));
    }
    fields
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
    use crate::common::report::InfoField;
    use crate::operations::images::describe::{Describe, Feature};

    /// Collapse fields into the `{:<19}: {}` text the render layer produces, so
    /// these unit tests keep asserting the exact wording/layout.
    fn rendered(fields: &[InfoField]) -> String {
        let mut out = String::new();
        for f in fields {
            match f {
                InfoField::Label { key, value } => {
                    out.push_str(&format!("{key:<19}: {value}\n"));
                }
                InfoField::Backup(lines) => {
                    out.push_str("Backup Files       :\n");
                    for l in lines {
                        out.push_str(l);
                        out.push('\n');
                    }
                }
            }
        }
        out
    }

    #[test]
    fn info_block_golden() {
        let d = Describe::parse(
            r#"{"version":"2.3","newest":true,"available":true,"installed":false,"obsolete":false,"incomplete":false}"#,
        )
        .unwrap();
        let got = rendered(&describe_fields("host", &d, false));
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
        let got = rendered(&describe_fields("host", &d, false));
        assert!(got.contains("Newest             : Unknown"));
        assert!(got.contains("Version            : 1.0"));
    }

    #[test]
    fn info_block_verbose_adds_changelog_contents() {
        let d = Describe::parse(r#"{"version":"1.0","changelog":"fixes","contents":["a","b"]}"#)
            .unwrap();
        let got = rendered(&describe_fields("host", &d, true));
        assert!(got.contains("Changelog          : fixes"));
        assert!(got.contains("Contents           : a, b"));
    }

    #[test]
    fn feature_block_golden() {
        let f = Feature::parse(
            r#"{"name":"experimental","enabled":true,"description":"edge","documentationUrl":"https://d"}"#,
        )
        .unwrap();
        let got = rendered(&feature_fields("experimental", &f));
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
        let got = rendered(&feature_fields("x", &f));
        assert!(got.contains("Enabled            : Unknown"));
        assert!(!got.contains("Description"));
    }

    #[test]
    fn info_block_extra_size_uses_format_size() {
        let d = Describe::parse(r#"{"version":"1.0","size":1048576}"#).unwrap();
        let got = rendered(&describe_fields("host", &d, false));
        assert!(got.contains("Download Size      : 1.00 MiB"));
    }
}
