//! Schema for `miz.toml`.
//!
//! This is the eventual replacement for `pacman.conf` parsing. The schema
//! covers every directive `pacman-conf`/`pacmanconf::Config` exposes, mapped
//! to TOML idioms. A converter (pacman.conf -> miz.toml) lives in a future
//! commit; for now this module just defines + deserializes the schema and
//! is not yet wired into `config::build`.
//!
//! Layout (matches future `[images]`, `[hooks]`, etc. namespaces):
//!
//! ```toml
//! [options]
//! # everything from pacman.conf's [options] section
//!
//! [[repos]]
//! name = "core"
//! servers = ["https://..."]
//! ```
//!
//! Defaults: where pacman ships meaningful defaults in /etc/pacman.conf
//! (parallel_downloads = 5, check_space = true, default sig_level, etc.)
//! this schema reproduces them via `Default` impls. A bare `miz.toml`
//! file with just `[options]` and one repo should behave the same as
//! the stock pacman.conf.
//!
//! `deny_unknown_fields` is intentionally NOT set: future miz-specific
//! sections (e.g. `[images]`) need to land in user configs before this
//! module learns about them, and we want forward compat to be the default.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct MizConfig {
    #[serde(default)]
    pub options: Options,

    #[serde(default, rename = "repos")]
    pub repos: Vec<Repository>,

    /// miz-specific split-db settings (NOT a pacman.conf mirror). Absent =>
    /// classic single-db pacman behaviour.
    #[serde(default)]
    pub archetype: Option<Archetype>,
}

/// Split-database / archive-snapshot settings for an installed Archetype
/// system. All keys optional; deliberately kept out of `[options]` so that
/// section stays a faithful pacman.conf mirror for the miz-convert round-trip.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Archetype {
    /// Read-only image db scanned for assume_installed provisions.
    pub image_db: Option<PathBuf>,

    /// Writable layered localdb. When set, this overrides `options.db_path`
    /// for the alpm localdb (wired in Phase 3).
    pub layered_db: Option<PathBuf>,

    /// Archive snapshot root, e.g. `https://archive.archlinux.org/repos`.
    pub archive_base: Option<String>,

    /// `YYYY/MM/DD` snapshot date; normally derived from os-release
    /// IMAGE_VERSION, override for testing.
    pub archive_date: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Options {
    // ---- paths ----
    #[serde(default = "default_root_dir")]
    pub root_dir: PathBuf,

    #[serde(default = "default_db_path")]
    pub db_path: PathBuf,

    #[serde(default = "default_cache_dir")]
    pub cache_dir: Vec<PathBuf>,

    #[serde(default = "default_hook_dir")]
    pub hook_dir: Vec<PathBuf>,

    #[serde(default = "default_gpg_dir")]
    pub gpg_dir: PathBuf,

    #[serde(default = "default_log_file")]
    pub log_file: PathBuf,

    // ---- behaviour lists ----
    #[serde(default)]
    pub hold_pkg: Vec<String>,

    #[serde(default)]
    pub ignore_pkg: Vec<String>,

    #[serde(default)]
    pub ignore_group: Vec<String>,

    #[serde(default = "default_architecture")]
    pub architecture: Vec<String>,

    #[serde(default)]
    pub no_upgrade: Vec<String>,

    #[serde(default)]
    pub no_extract: Vec<String>,

    #[serde(default = "default_clean_method")]
    pub clean_method: Vec<String>,

    // ---- downloads ----
    #[serde(default)]
    pub xfer_command: Option<String>,

    #[serde(default = "default_parallel_downloads")]
    pub parallel_downloads: u64,

    #[serde(default)]
    pub disable_download_timeout: bool,

    #[serde(default)]
    pub download_user: Option<String>,

    // ---- signature levels ----
    #[serde(default = "default_sig_level")]
    pub sig_level: Vec<String>,

    #[serde(default = "default_local_file_sig_level")]
    pub local_file_sig_level: Vec<String>,

    #[serde(default = "default_remote_file_sig_level")]
    pub remote_file_sig_level: Vec<String>,

    // ---- sandbox ----
    #[serde(default)]
    pub disable_sandbox: bool,

    #[serde(default)]
    pub disable_sandbox_filesystem: bool,

    #[serde(default)]
    pub disable_sandbox_syscalls: bool,

    // ---- UI / misc booleans ----
    #[serde(default)]
    pub use_syslog: bool,

    #[serde(default)]
    pub color: bool,

    #[serde(default)]
    pub total_download: bool,

    #[serde(default = "default_check_space")]
    pub check_space: bool,

    #[serde(default)]
    pub verbose_pkg_lists: bool,

    /// Pacman's `ILoveCandy` easter egg (pac-man progress chomper).
    #[serde(default)]
    pub chomp: bool,

    // ---- deprecated but still parsed by pacman-conf ----
    /// Deprecated; pacman has dropped delta support but the directive is
    /// still parsed. Kept for round-trip fidelity with pacman.conf.
    #[serde(default)]
    pub use_delta: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Repository {
    /// Section name in pacman.conf (e.g. `core`, `extra`, `multilib`).
    pub name: String,

    /// `Server = ...` lines. Order is significant: pacman tries them
    /// in the order given. `$repo` and `$arch` substitutions are the
    /// consumer's responsibility (libalpm does them at fetch time).
    #[serde(default)]
    pub servers: Vec<String>,

    /// Per-repo SigLevel override. Empty = inherit `[options]`.
    #[serde(default)]
    pub sig_level: Vec<String>,

    /// Per-repo Usage. Empty = `All` (libalpm default). Recognised values
    /// per pacman(5): `Sync`, `Search`, `Install`, `Upgrade`, `All`.
    #[serde(default)]
    pub usage: Vec<String>,

    /// `Include = /etc/pacman.d/mirrorlist` style. Resolved at load time
    /// by whatever assembles the final server list; the schema preserves
    /// the directive verbatim so a future converter can decide whether
    /// to inline or keep the reference.
    #[serde(default)]
    pub include: Vec<PathBuf>,
}

// ---- defaults: mirror pacman's bundled /etc/pacman.conf ----

fn default_root_dir() -> PathBuf {
    PathBuf::from("/")
}
fn default_db_path() -> PathBuf {
    PathBuf::from("/var/lib/pacman/")
}
fn default_cache_dir() -> Vec<PathBuf> {
    vec![PathBuf::from("/var/cache/pacman/pkg/")]
}
fn default_hook_dir() -> Vec<PathBuf> {
    vec![PathBuf::from("/etc/pacman.d/hooks/")]
}
fn default_gpg_dir() -> PathBuf {
    PathBuf::from("/etc/pacman.d/gnupg/")
}
fn default_log_file() -> PathBuf {
    PathBuf::from("/var/log/pacman.log")
}
fn default_architecture() -> Vec<String> {
    vec!["auto".into()]
}
fn default_clean_method() -> Vec<String> {
    vec!["KeepInstalled".into()]
}
fn default_parallel_downloads() -> u64 {
    5
}
fn default_check_space() -> bool {
    true
}
fn default_sig_level() -> Vec<String> {
    vec!["Required".into(), "DatabaseOptional".into()]
}
fn default_local_file_sig_level() -> Vec<String> {
    vec!["Optional".into()]
}
fn default_remote_file_sig_level() -> Vec<String> {
    vec!["Required".into()]
}

impl Default for Options {
    fn default() -> Self {
        Options {
            root_dir: default_root_dir(),
            db_path: default_db_path(),
            cache_dir: default_cache_dir(),
            hook_dir: default_hook_dir(),
            gpg_dir: default_gpg_dir(),
            log_file: default_log_file(),
            hold_pkg: Vec::new(),
            ignore_pkg: Vec::new(),
            ignore_group: Vec::new(),
            architecture: default_architecture(),
            no_upgrade: Vec::new(),
            no_extract: Vec::new(),
            clean_method: default_clean_method(),
            xfer_command: None,
            parallel_downloads: default_parallel_downloads(),
            disable_download_timeout: false,
            download_user: None,
            sig_level: default_sig_level(),
            local_file_sig_level: default_local_file_sig_level(),
            remote_file_sig_level: default_remote_file_sig_level(),
            disable_sandbox: false,
            disable_sandbox_filesystem: false,
            disable_sandbox_syscalls: false,
            use_syslog: false,
            color: false,
            total_download: false,
            check_space: default_check_space(),
            verbose_pkg_lists: false,
            chomp: false,
            use_delta: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_table_yields_pacman_defaults() {
        let c: MizConfig = toml::from_str("").unwrap();
        assert_eq!(c.options.root_dir, PathBuf::from("/"));
        assert_eq!(c.options.db_path, PathBuf::from("/var/lib/pacman/"));
        assert_eq!(c.options.parallel_downloads, 5);
        assert!(c.options.check_space);
        assert_eq!(c.options.architecture, vec!["auto".to_string()]);
        assert_eq!(
            c.options.sig_level,
            vec!["Required".to_string(), "DatabaseOptional".to_string()]
        );
        assert!(c.repos.is_empty());
    }

    #[test]
    fn minimal_realistic_config_roundtrips() {
        let src = r#"
            [options]
            architecture = ["x86_64"]
            parallel_downloads = 10
            color = true

            [[repos]]
            name = "core"
            servers = ["https://mirror.example/$repo/os/$arch"]

            [[repos]]
            name = "extra"
            include = ["/etc/pacman.d/mirrorlist"]
            usage = ["Sync", "Search"]
        "#;
        let c: MizConfig = toml::from_str(src).unwrap();
        assert_eq!(c.options.architecture, vec!["x86_64".to_string()]);
        assert_eq!(c.options.parallel_downloads, 10);
        assert!(c.options.color);
        assert_eq!(c.repos.len(), 2);
        assert_eq!(c.repos[0].name, "core");
        assert_eq!(c.repos[0].servers.len(), 1);
        assert_eq!(c.repos[1].name, "extra");
        assert_eq!(
            c.repos[1].include,
            vec![PathBuf::from("/etc/pacman.d/mirrorlist")]
        );
        assert_eq!(
            c.repos[1].usage,
            vec!["Sync".to_string(), "Search".to_string()]
        );

        let serialized = toml::to_string(&c).unwrap();
        let reparsed: MizConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(reparsed.repos.len(), c.repos.len());
        assert_eq!(
            reparsed.options.parallel_downloads,
            c.options.parallel_downloads
        );
    }

    #[test]
    fn unknown_fields_are_silently_ignored() {
        // Forward-compat: future [images] section won't break old miz binaries.
        let src = r#"
            [options]
            color = true

            [images]
            root = "/var/lib/archetype/images"

            [unknown_future_feature]
            arbitrary = "stuff"
        "#;
        let c: MizConfig = toml::from_str(src).unwrap();
        assert!(c.options.color);
    }

    #[test]
    fn bare_config_has_no_archetype_section() {
        let c: MizConfig = toml::from_str("").unwrap();
        assert!(c.archetype.is_none());
        // pacman defaults remain intact alongside the new optional section.
        assert_eq!(c.options.db_path, PathBuf::from("/var/lib/pacman/"));
        assert_eq!(c.options.parallel_downloads, 5);
    }

    #[test]
    fn full_archetype_section_deserializes() {
        let src = r#"
            [archetype]
            image_db = "/usr/lib/miz/db"
            layered_db = "/var/lib/miz"
            archive_base = "https://archive.archlinux.org/repos"
            archive_date = "2026/06/17"
        "#;
        let c: MizConfig = toml::from_str(src).unwrap();
        let a = c.archetype.expect("archetype section present");
        assert_eq!(a.image_db, Some(PathBuf::from("/usr/lib/miz/db")));
        assert_eq!(a.layered_db, Some(PathBuf::from("/var/lib/miz")));
        assert_eq!(
            a.archive_base,
            Some("https://archive.archlinux.org/repos".to_string())
        );
        assert_eq!(a.archive_date, Some("2026/06/17".to_string()));
    }

    #[test]
    fn archetype_section_roundtrips() {
        let src = r#"
            [archetype]
            image_db = "/usr/lib/miz/db"
            layered_db = "/var/lib/miz"
        "#;
        let c: MizConfig = toml::from_str(src).unwrap();
        let serialized = toml::to_string(&c).unwrap();
        let reparsed: MizConfig = toml::from_str(&serialized).unwrap();
        let a = reparsed.archetype.expect("archetype survives round-trip");
        assert_eq!(a.image_db, Some(PathBuf::from("/usr/lib/miz/db")));
        assert_eq!(a.layered_db, Some(PathBuf::from("/var/lib/miz")));
        assert_eq!(a.archive_base, None);
        assert_eq!(a.archive_date, None);
    }

    #[test]
    fn pacman_archive_repo_layout() {
        // A common real-world config: per-repo SigLevel override.
        let src = r#"
            [[repos]]
            name = "archive"
            sig_level = ["Optional", "TrustAll"]
            servers = ["https://archive.example/$repo"]
        "#;
        let c: MizConfig = toml::from_str(src).unwrap();
        assert_eq!(
            c.repos[0].sig_level,
            vec!["Optional".to_string(), "TrustAll".to_string()]
        );
    }
}
