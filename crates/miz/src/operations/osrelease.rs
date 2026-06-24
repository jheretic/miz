//! Shared os-release parsing + archive-snapshot date/URL derivation.
//!
//! Both `-I` (images) and the split-db config path need to read
//! `IMAGE_VERSION` from os-release, so the parser lives here rather than in
//! `operations::images`. Kept free of `miz_config` types (pure str/Path
//! inputs) so it stays a testable leaf module; the caller threads config
//! values in.

// Several functions here are derivation helpers consumed by the split-db
// wiring in later phases (config/relay); allow until those land.
#![allow(dead_code)]

use std::path::Path;

/// Default archive snapshot root used when `[archetype].archive_base` is None.
pub const DEFAULT_ARCHIVE_BASE: &str = "https://archive.archlinux.org/repos";

/// Extract `IMAGE_VERSION=` from os-release text. None if the key is absent.
fn parse_image_version(text: &str) -> Option<String> {
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("IMAGE_VERSION=") {
            return Some(v.trim().trim_matches('"').to_string());
        }
    }
    None
}

/// Read `IMAGE_VERSION=` from a specific os-release file (e.g. a staged
/// image's `<root>/usr/lib/os-release`). None if the file or key is absent.
pub fn image_version_from(path: &Path) -> Option<String> {
    parse_image_version(&std::fs::read_to_string(path).ok()?)
}

/// Booted system's `IMAGE_VERSION`: prefer `/etc/os-release`, fall back to
/// `/usr/lib/os-release`. Relocated from `operations::images` (the unused
/// `targets` param was dropped — it was dead).
pub fn booted_image_version() -> Option<String> {
    let text = std::fs::read_to_string("/etc/os-release")
        .or_else(|_| std::fs::read_to_string("/usr/lib/os-release"))
        .ok()?;
    parse_image_version(&text)
}

/// Map an `IMAGE_VERSION` to the `YYYY/MM/DD` archive snapshot date.
///
/// Port of `repo_date()` in `archetype-build/mkosi.postinst`: split on `-`,
/// take the first field, replace `.` with `/`. e.g. `2026.06.17-2` ->
/// `2026/06/17`. The format contract MUST stay in sync across that language
/// boundary (NOT consolidated — separate repos).
pub fn image_date(version: &str) -> String {
    version
        .split('-')
        .next()
        .unwrap_or(version)
        .replace('.', "/")
}

/// Resolve the archive snapshot date with precedence: an explicit
/// `[archetype].archive_date` override wins; otherwise derive from the
/// os-release `IMAGE_VERSION`.
pub fn resolve_archive_date(explicit: Option<&str>, version: Option<&str>) -> Option<String> {
    match explicit {
        Some(d) => Some(d.to_string()),
        None => version.map(image_date),
    }
}

/// Assemble the archive repo base URL `{archive_base}/{date}/$repo/os/$arch`.
/// `$repo`/`$arch` are left literal — libalpm substitutes them at fetch time.
/// `archive_base` defaults to [`DEFAULT_ARCHIVE_BASE`] when None.
pub fn archive_url(archive_base: Option<&str>, date: &str) -> String {
    let base = archive_base
        .unwrap_or(DEFAULT_ARCHIVE_BASE)
        .trim_end_matches('/');
    format!("{base}/{date}/$repo/os/$arch")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn image_date_strips_release_and_slashes_date() {
        assert_eq!(image_date("2026.06.17-2"), "2026/06/17");
    }

    #[test]
    fn image_date_no_release_suffix() {
        assert_eq!(image_date("2026.06.17"), "2026/06/17");
    }

    #[test]
    fn resolve_archive_date_override_wins() {
        assert_eq!(
            resolve_archive_date(Some("2020/01/01"), Some("2026.06.17-2")),
            Some("2020/01/01".to_string())
        );
    }

    #[test]
    fn resolve_archive_date_derives_when_no_override() {
        assert_eq!(
            resolve_archive_date(None, Some("2026.06.17-2")),
            Some("2026/06/17".to_string())
        );
    }

    #[test]
    fn resolve_archive_date_none_without_version() {
        assert_eq!(resolve_archive_date(None, None), None);
    }

    #[test]
    fn archive_url_default_base() {
        assert_eq!(
            archive_url(None, "2026/06/17"),
            "https://archive.archlinux.org/repos/2026/06/17/$repo/os/$arch"
        );
    }

    #[test]
    fn archive_url_custom_base_trims_trailing_slash() {
        assert_eq!(
            archive_url(Some("https://example.test/repos/"), "2026/06/17"),
            "https://example.test/repos/2026/06/17/$repo/os/$arch"
        );
    }

    #[test]
    fn parse_image_version_extracts_quoted_value() {
        let text = "ID=archetype\nIMAGE_VERSION=\"2026.06.17-2\"\nNAME=foo\n";
        assert_eq!(parse_image_version(text), Some("2026.06.17-2".to_string()));
    }

    #[test]
    fn parse_image_version_absent_key() {
        assert_eq!(parse_image_version("ID=archetype\n"), None);
    }

    #[test]
    fn image_version_from_fixture_file() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/os-release/usr/lib/os-release");
        assert_eq!(image_version_from(&path), Some("2026.06.17-2".to_string()));
    }

    #[test]
    fn image_version_from_missing_file_is_none() {
        assert_eq!(
            image_version_from(Path::new("/nonexistent/os-release")),
            None
        );
    }
}
