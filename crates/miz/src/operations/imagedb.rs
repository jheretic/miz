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

// Lists every package in the read-only image db as (name, version) pairs, for
// query operations (-Q/-Qs) that union the baked-in /usr packages with the
// mutable /var localdb. Same grouped-tree traversal as `provisions`, but keeps
// name+version instead of emitting provision tokens. Missing root -> empty
// (non-fatal). Duplicate names across groups keep the first seen.
pub fn installed_packages(image_db_root: &Path) -> Result<Vec<(String, String)>> {
    let mut out: Vec<(String, String)> = Vec::new();
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
            Err(_) => continue,
        };
        for pkg in pkgs {
            let pkg = match pkg {
                Ok(p) => p.path(),
                Err(_) => continue,
            };
            let desc = pkg.join("desc");
            if !desc.is_file() {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&desc) else {
                continue;
            };
            if let Some((name, version, _)) = parse_desc(&text) {
                if seen.insert(name.clone()) {
                    out.push((name, version));
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
#[derive(Debug, Default, PartialEq, Eq)]
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
    pub isize: Option<i64>,
}

/// Look up a single package by name in the image db and return its `desc`
/// fields, or None if not present. Traverses the same grouped tree as
/// [`installed_packages`]; the first group with a matching NAME wins (mirroring
/// the query shadow order). Missing/unreadable root -> None (non-fatal).
pub fn package_info(image_db_root: &Path, want: &str) -> Option<ImagePackage> {
    let groups = std::fs::read_dir(image_db_root).ok()?;
    for group in groups.flatten() {
        let gpath = group.path();
        if !gpath.is_dir() {
            continue;
        }
        let Ok(pkgs) = std::fs::read_dir(&gpath) else {
            continue;
        };
        for pkg in pkgs.flatten() {
            let desc = pkg.path().join("desc");
            if !desc.is_file() {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&desc) else {
                continue;
            };
            let fields = parse_desc_fields(&text);
            let name = fields.get("NAME").and_then(|v| v.first());
            if name.map(String::as_str) != Some(want) {
                continue;
            }
            let one = |k: &str| fields.get(k).and_then(|v| v.first()).cloned();
            let many = |k: &str| fields.get(k).cloned().unwrap_or_default();
            return Some(ImagePackage {
                name: want.to_string(),
                version: one("VERSION").unwrap_or_default(),
                desc: one("DESC"),
                url: one("URL"),
                arch: one("ARCH"),
                licenses: many("LICENSE"),
                groups: many("GROUPS"),
                provides: many("PROVIDES"),
                depends: many("DEPENDS"),
                isize: one("SIZE").and_then(|s| s.parse().ok()),
            });
        }
    }
    None
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
    fn installed_packages_lists_name_version() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/image_db");
        let got: HashSet<(String, String)> =
            installed_packages(&root).unwrap().into_iter().collect();
        let want: HashSet<(String, String)> = [("foo", "1.2.3-1"), ("baz", "2.0-3")]
            .iter()
            .map(|(n, v)| (n.to_string(), v.to_string()))
            .collect();
        assert_eq!(got, want);
    }

    #[test]
    fn installed_packages_missing_root_is_empty() {
        assert!(installed_packages(Path::new("/nonexistent/miz/db"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn package_info_reads_desc_fields() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/image_db");
        let foo = package_info(&root, "foo").expect("foo present");
        assert_eq!(foo.name, "foo");
        assert_eq!(foo.version, "1.2.3-1");
        assert_eq!(foo.desc.as_deref(), Some("A foo package"));
        assert_eq!(foo.depends, vec!["glibc".to_string()]);
        assert_eq!(
            foo.provides,
            vec!["libfoo.so=1-64".to_string(), "bar".to_string()]
        );
        assert!(package_info(&root, "nonexistent").is_none());
    }

    #[test]
    fn package_info_missing_root_is_none() {
        assert!(package_info(Path::new("/nonexistent/miz/db"), "foo").is_none());
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
