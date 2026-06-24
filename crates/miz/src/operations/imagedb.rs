use crate::error::Result;
use std::collections::HashSet;
use std::path::Path;

// Scans the read-only image db (`/usr/lib/miz/db/{weight}-{id}/name-version/`)
// and returns provision strings for alpm `assume_installed`. Each package yields
// `name=version` (an EQ-mod entry is the only kind libalpm consults for a
// versioned dep) plus every %PROVIDES% token verbatim.
// Wired into config.rs in Phase 3; unused until then.
#[allow(dead_code)]
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
                eprintln!("imagedb: skipping {}: missing %NAME%/%VERSION%", desc.display());
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
    fn parse_desc_requires_name_and_version() {
        assert!(parse_desc("%VERSION%\n1-1\n\n").is_none());
        assert!(parse_desc("%NAME%\nfoo\n\n").is_none());
        let (n, v, p) = parse_desc("%NAME%\nfoo\n\n%VERSION%\n1-1\n\n").unwrap();
        assert_eq!((n.as_str(), v.as_str()), ("foo", "1-1"));
        assert!(p.is_empty());
    }
}
