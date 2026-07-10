use crate::error::Result;
use std::collections::HashSet;
use std::path::Path;

// Scans the read-only image db (`/usr/lib/miz/db/{weight}-{id}/name-version/`)
// and returns provision strings for alpm `assume_installed`. Each package yields
// `name=version` (an EQ-mod entry is the only kind libalpm consults for a
// versioned dep) plus every %PROVIDES% token verbatim.
// Wired into config.rs (seed_assume_installed).
pub fn provisions(image_db_root: &Path) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    let group_iter = match std::fs::read_dir(image_db_root) {
        Ok(it) => it,
        Err(e) => {
            eprintln!("imagedb: cannot read {}: {e}", image_db_root.display());
            return Ok(out);
        }
    };

    for group in group_iter {
        let group = match group {
            Ok(g) => g.path(),
            Err(_) => continue,
        };
        if !group.is_dir() {
            continue;
        }
        let pkgs = match std::fs::read_dir(&group) {
            Ok(it) => it,
            Err(e) => {
                eprintln!("imagedb: skipping {}: {e}", group.display());
                continue;
            }
        };
        for pkg in pkgs {
            let pkg = match pkg {
                Ok(p) => p.path(),
                Err(_) => continue,
            };
            if !pkg.is_dir() {
                continue;
            }
            let desc = pkg.join("desc");
            if !desc.is_file() {
                continue;
            }
            let text = match std::fs::read_to_string(&desc) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("imagedb: skipping {}: {e}", desc.display());
                    continue;
                }
            };
            let Some((name, version, provides)) = parse_desc(&text) else {
                eprintln!(
                    "imagedb: skipping {}: missing %NAME%/%VERSION%",
                    desc.display()
                );
                continue;
            };
            for tok in std::iter::once(format!("{name}={version}")).chain(provides) {
                if seen.insert(tok.clone()) {
                    out.push(tok);
                }
            }
        }
    }

    Ok(out)
}

/// The subset of package metadata the image-db `desc` carries, for rendering
/// `-Qi` on a baked-in /usr package. The grouped image db is not an alpm
/// localdb, so miz can't produce a full alpm `Pkg` -- fields the desc doesn't
/// store (reverse-deps, install date/reason, size) are simply absent, which is
/// still far better than "package not found".
#[derive(Debug, Default, PartialEq, Eq, Clone)]
pub struct ImagePackage {
    pub name: String,
    pub version: String,
    pub desc: Option<String>,
    pub url: Option<String>,
    pub arch: Option<String>,
    pub licenses: Vec<String>,
    pub groups: Vec<String>,
    pub provides: Vec<String>,
    pub depends: Vec<String>,
    pub optdepends: Vec<String>,
    pub conflicts: Vec<String>,
    pub replaces: Vec<String>,
    pub packager: Option<String>,
    pub build_date: Option<i64>,
    pub install_date: Option<i64>,
    /// The pacman install reason: 1 == installed as a dependency, 0/absent ==
    /// explicit. Mirrors alpm's PackageReason for the -Qd/-Qe filters.
    pub explicit: bool,
    pub isize: Option<i64>,
    /// Owned file paths (from the package's `files` db entry, `%FILES%`), each
    /// relative to the root (no leading `/`), directories keeping a trailing `/`.
    pub files: Vec<String>,
    /// Backup files as (path, md5) from `%BACKUP%`.
    pub backup: Vec<(String, String)>,
}

/// Every package in the image db as a full [`ImagePackage`] (desc + files), for
/// the query paths that union the baked-in /usr packages with the /var localdb.
/// First occurrence of a name across groups wins (the query shadow order).
/// Missing/unreadable root -> empty (non-fatal).
pub fn all_packages(image_db_root: &Path) -> Vec<ImagePackage> {
    let mut out: Vec<ImagePackage> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    let Ok(groups) = std::fs::read_dir(image_db_root) else {
        return out;
    };
    // Visit groups in sorted order (`{weight}-{id}`, e.g. 10-archetype before
    // 50-extra) so a name present in multiple groups resolves deterministically
    // to the lowest-weight group -- readdir order is otherwise unspecified.
    let mut group_paths: Vec<_> = groups
        .flatten()
        .map(|g| g.path())
        .filter(|p| p.is_dir())
        .collect();
    group_paths.sort();
    for gpath in group_paths {
        let Ok(pkgs) = std::fs::read_dir(&gpath) else {
            continue;
        };
        for pkg in pkgs.flatten() {
            if let Some(p) = read_package(&pkg.path()) {
                if seen.insert(p.name.clone()) {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// Build a full [`ImagePackage`] from one `<name-version>/` db dir (`desc` +
/// optional `files`). None if `desc` is absent/unreadable or lacks %NAME%.
fn read_package(pkg_dir: &Path) -> Option<ImagePackage> {
    let text = std::fs::read_to_string(pkg_dir.join("desc")).ok()?;
    let fields = parse_desc_fields(&text);
    let name = fields.get("NAME").and_then(|v| v.first())?.clone();
    let one = |k: &str| fields.get(k).and_then(|v| v.first()).cloned();
    let many = |k: &str| fields.get(k).cloned().unwrap_or_default();
    // A package with no %VERSION% is malformed; skip it (matching `provisions`,
    // which requires both fields) rather than surface an empty-version entry.
    let version = one("VERSION").filter(|v| !v.is_empty())?;

    // `files` (%FILES% list + %BACKUP% path\tmd5). Absent for a package that
    // owns nothing, so a missing file is not an error.
    let (files, backup) = match std::fs::read_to_string(pkg_dir.join("files")) {
        Ok(t) => parse_files(&t),
        Err(_) => (Vec::new(), Vec::new()),
    };

    Some(ImagePackage {
        name,
        version,
        desc: one("DESC"),
        url: one("URL"),
        arch: one("ARCH"),
        licenses: many("LICENSE"),
        groups: many("GROUPS"),
        provides: many("PROVIDES"),
        depends: many("DEPENDS"),
        optdepends: many("OPTDEPENDS"),
        conflicts: many("CONFLICTS"),
        replaces: many("REPLACES"),
        packager: one("PACKAGER"),
        build_date: one("BUILDDATE").and_then(|s| s.parse().ok()),
        install_date: one("INSTALLDATE").and_then(|s| s.parse().ok()),
        // pacman omits %REASON% for explicitly-installed packages; 1 == dep.
        explicit: one("REASON").as_deref() != Some("1"),
        isize: one("SIZE").and_then(|s| s.parse().ok()),
        files,
        backup,
    })
}

/// Parse a pacman `files` db entry into (owned paths, backup entries). `%FILES%`
/// lists relative paths (dirs end in `/`); `%BACKUP%` lists `path\tmd5`. Pure.
fn parse_files(text: &str) -> (Vec<String>, Vec<(String, String)>) {
    let mut files = Vec::new();
    let mut backup = Vec::new();
    let mut section = "";
    for line in text.lines() {
        let t = line.trim_end();
        if t.starts_with('%') && t.ends_with('%') && t.len() >= 2 {
            section = match &t[1..t.len() - 1] {
                "FILES" => "files",
                "BACKUP" => "backup",
                _ => "",
            };
            continue;
        }
        if t.is_empty() {
            continue;
        }
        match section {
            "files" => files.push(t.to_string()),
            "backup" => {
                if let Some((path, md5)) = t.split_once('\t') {
                    backup.push((path.to_string(), md5.to_string()));
                }
            }
            _ => {}
        }
    }
    (files, backup)
}

/// Parse the `%KEY%\nval...\n\n` block format into a key -> values map. Shared
/// by [`package_info`]; keys have the surrounding `%` stripped.
fn parse_desc_fields(text: &str) -> std::collections::HashMap<String, Vec<String>> {
    let mut map = std::collections::HashMap::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let key = line.trim();
        if !(key.starts_with('%') && key.ends_with('%') && key.len() >= 2) {
            continue;
        }
        let mut values = Vec::new();
        for v in lines.by_ref() {
            if v.trim().is_empty() {
                break;
            }
            values.push(v.trim().to_string());
        }
        map.insert(key[1..key.len() - 1].to_string(), values);
    }
    map
}

// Parses the trivial `%KEY%\nval\n\n` block format, returning name, version, and
// provides tokens. Returns None if NAME or VERSION is absent.
fn parse_desc(text: &str) -> Option<(String, String, Vec<String>)> {
    let mut name: Option<String> = None;
    let mut version: Option<String> = None;
    let mut provides: Vec<String> = Vec::new();

    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let key = line.trim();
        if !(key.starts_with('%') && key.ends_with('%') && key.len() >= 2) {
            continue;
        }
        let mut values: Vec<String> = Vec::new();
        for v in lines.by_ref() {
            if v.trim().is_empty() {
                break;
            }
            values.push(v.trim().to_string());
        }
        match key {
            "%NAME%" => name = values.into_iter().next(),
            "%VERSION%" => version = values.into_iter().next(),
            "%PROVIDES%" => provides = values,
            _ => {}
        }
    }

    Some((name?, version?, provides))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn scans_fixture_tree() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/image_db");
        let got: HashSet<String> = provisions(&root).unwrap().into_iter().collect();
        let want: HashSet<String> = [
            "foo=1.2.3-1",
            "libfoo.so=1-64",
            "bar",
            "baz=2.0-3",
            "libbaz.so=2-64",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(got, want);
    }

    #[test]
    fn missing_root_is_empty_not_error() {
        let got = provisions(Path::new("/nonexistent/miz/db")).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn all_packages_lists_name_version() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/image_db");
        let got: HashSet<(String, String)> = all_packages(&root)
            .into_iter()
            .map(|p| (p.name, p.version))
            .collect();
        let want: HashSet<(String, String)> = [("foo", "1.2.3-1"), ("baz", "2.0-3")]
            .iter()
            .map(|(n, v)| (n.to_string(), v.to_string()))
            .collect();
        assert_eq!(got, want);
    }

    #[test]
    fn all_packages_missing_root_is_empty() {
        assert!(all_packages(Path::new("/nonexistent/miz/db")).is_empty());
    }

    #[test]
    fn all_packages_reads_desc_fields() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/image_db");
        let pkgs = all_packages(&root);
        let foo = pkgs.iter().find(|p| p.name == "foo").expect("foo present");
        assert_eq!(foo.version, "1.2.3-1");
        assert_eq!(foo.desc.as_deref(), Some("A foo package"));
        assert_eq!(foo.depends, vec!["glibc".to_string()]);
        assert_eq!(
            foo.provides,
            vec!["libfoo.so=1-64".to_string(), "bar".to_string()]
        );
        assert!(!pkgs.iter().any(|p| p.name == "nonexistent"));
    }

    #[test]
    fn all_packages_reads_files_and_backup() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/image_db");
        let foo = all_packages(&root)
            .into_iter()
            .find(|p| p.name == "foo")
            .expect("foo present");
        assert_eq!(
            foo.files,
            vec![
                "usr/bin/foo".to_string(),
                "usr/share/doc/foo/".to_string(),
                "usr/share/doc/foo/README".to_string(),
            ]
        );
        assert_eq!(
            foo.backup,
            vec![(
                "etc/foo.conf".to_string(),
                "d41d8cd98f00b204e9800998ecf8427e".to_string()
            )]
        );
        // A package without a `files` db entry yields empty lists, not an error.
        let baz = all_packages(&root)
            .into_iter()
            .find(|p| p.name == "baz")
            .expect("baz present");
        assert!(baz.files.is_empty() && baz.backup.is_empty());
    }

    #[test]
    fn parse_files_splits_sections() {
        let (files, backup) =
            parse_files("%FILES%\nusr/bin/x\nusr/lib/\n\n%BACKUP%\netc/x.conf\tabc123\n");
        assert_eq!(files, vec!["usr/bin/x".to_string(), "usr/lib/".to_string()]);
        assert_eq!(
            backup,
            vec![("etc/x.conf".to_string(), "abc123".to_string())]
        );
    }

    #[test]
    fn parse_desc_requires_name_and_version() {
        assert!(parse_desc("%VERSION%\n1-1\n\n").is_none());
        assert!(parse_desc("%NAME%\nfoo\n\n").is_none());
        let (n, v, p) = parse_desc("%NAME%\nfoo\n\n%VERSION%\n1-1\n\n").unwrap();
        assert_eq!((n.as_str(), v.as_str()), ("foo", "1-1"));
        assert!(p.is_empty());
    }
}
