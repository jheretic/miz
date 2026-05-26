// Test-only helpers for miz integration tests. See tests/fixtures/README.md
// for the on-disk format and host requirements.
//
// Not every integration test consumes every helper; suppress dead-code
// warnings rather than littering attributes per-symbol.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use tempfile::TempDir;

/// Spawn the `miz` binary built by cargo. Every integration test
/// invokes this; keep behavior identical to the historical per-file
/// helper so each test file just imports `miz` from here.
pub fn miz() -> Command {
    Command::cargo_bin("miz").expect("binary built")
}

/// Pinned timestamp embedded in `%BUILDDATE%` / `%INSTALLDATE%`. Keeping
/// it constant makes `install_fake_pkg` byte-for-byte deterministic.
const FAKE_TIMESTAMP: i64 = 1_700_000_000;

/// Source of truth for the per-test pacman.conf. Substituted at runtime
/// by [`make_test_root`].
const PACMAN_CONF_TEMPLATE: &str = include_str!("../fixtures/root/etc/pacman.conf");

/// A throwaway pacman root suitable for `miz --root <path>`.
///
/// Drop releases the underlying tempdir; keep the `TestRoot` alive for
/// the lifetime of the test. The `path` field is the absolute root path
/// callers should pass to `--root`.
pub struct TestRoot {
    pub path: PathBuf,
    _tmp: TempDir,
}

impl TestRoot {
    /// `<root>/etc/pacman.conf`. Pass to `miz --config`.
    pub fn config_path(&self) -> PathBuf {
        self.path.join("etc/pacman.conf")
    }

    /// `<root>/var/lib/pacman/`. Pass to `miz --dbpath` when needed.
    pub fn dbpath(&self) -> PathBuf {
        self.path.join("var/lib/pacman")
    }

    /// `<root>/var/cache/pacman/pkg/`.
    pub fn cachedir(&self) -> PathBuf {
        self.path.join("var/cache/pacman/pkg")
    }

    /// `<root>/var/lib/pacman/local/<name>-<version>/`.
    pub fn local_pkg_dir(&self, name: &str, version: &str) -> PathBuf {
        self.dbpath()
            .join("local")
            .join(format!("{name}-{version}"))
    }
}

/// Build a fresh tempdir mirroring `tests/fixtures/root/`.
///
/// Empty directories are recreated; the pacman.conf template is
/// rewritten with substituted absolute paths and written to
/// `<root>/etc/pacman.conf`.
pub fn make_test_root() -> TestRoot {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path().to_path_buf();

    for sub in [
        "etc",
        "etc/pacman.d/gnupg",
        "var/lib/pacman/local",
        "var/lib/pacman/sync",
        "var/cache/pacman/pkg",
        "var/log",
    ] {
        fs::create_dir_all(root.join(sub)).expect("mkdir -p");
    }

    let conf = render_pacman_conf(&root);
    fs::write(root.join("etc/pacman.conf"), conf).expect("write pacman.conf");

    TestRoot {
        path: root,
        _tmp: tmp,
    }
}

fn render_pacman_conf(root: &Path) -> String {
    let dbpath = root.join("var/lib/pacman/");
    let cachedir = root.join("var/cache/pacman/pkg/");
    let logfile = root.join("var/log/pacman.log");
    let gpgdir = root.join("etc/pacman.d/gnupg/");
    let mut s = String::from(PACMAN_CONF_TEMPLATE);
    s = s.replace("@ROOTDIR@", &with_trailing_slash(root));
    s = s.replace("@DBPATH@", &path_string(&dbpath));
    s = s.replace("@CACHEDIR@", &path_string(&cachedir));
    s = s.replace("@LOGFILE@", &path_string(&logfile));
    s = s.replace("@GPGDIR@", &path_string(&gpgdir));
    s
}

fn path_string(p: &Path) -> String {
    p.to_str()
        .expect("test root path must be utf-8")
        .to_string()
}

fn with_trailing_slash(p: &Path) -> String {
    let mut s = path_string(p);
    if !s.ends_with('/') {
        s.push('/');
    }
    s
}

/// A staged installed-package description. Pass to [`install_fake_pkg`].
///
/// `name` and `version` are required; everything else has a sensible
/// default. `files` is `(path, bytes)`; the helper writes the bytes into
/// `<root>/<path>` and adds the path to the package's `%FILES%` block.
#[derive(Debug, Clone)]
pub struct FakePkg<'a> {
    pub name: &'a str,
    pub version: &'a str,
    pub desc: &'a str,
    pub arch: &'a str,
    pub url: Option<&'a str>,
    pub packager: &'a str,
    pub licenses: &'a [&'a str],
    pub groups: &'a [&'a str],
    pub depends: &'a [&'a str],
    pub provides: &'a [&'a str],
    pub optdepends: &'a [&'a str],
    pub conflicts: &'a [&'a str],
    pub replaces: &'a [&'a str],
    pub size: u64,
    /// `0` = explicitly installed, `1` = installed as dependency.
    pub reason: u8,
    pub files: &'a [(&'a str, &'a [u8])],
}

impl<'a> FakePkg<'a> {
    /// Bare-minimum package: just a name and version, no owned files,
    /// installed explicitly, arch `any`.
    pub fn minimal(name: &'a str, version: &'a str) -> Self {
        Self {
            name,
            version,
            desc: "test package",
            arch: "any",
            url: None,
            packager: "miz tests <none@none>",
            licenses: &[],
            groups: &[],
            depends: &[],
            provides: &[],
            optdepends: &[],
            conflicts: &[],
            replaces: &[],
            size: 0,
            reason: 0,
            files: &[],
        }
    }
}

/// Stage a package into the test root's localdb without going through
/// a libalpm transaction. Writes `desc`, `files`, and any actual file
/// payloads under `<root>/`.
pub fn install_fake_pkg(root: &TestRoot, pkg: &FakePkg<'_>) {
    let dir = root.local_pkg_dir(pkg.name, pkg.version);
    fs::create_dir_all(&dir).expect("mkdir local pkg dir");

    fs::write(dir.join("desc"), render_desc(pkg)).expect("write desc");
    fs::write(dir.join("files"), render_files_block(pkg)).expect("write files");

    for (rel, bytes) in pkg.files {
        let full = root.path.join(rel.trim_start_matches('/'));
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).expect("mkdir owned file parent");
        }
        // Directory entries (trailing slash) are recorded in `files` but
        // have no payload to write.
        if !rel.ends_with('/') {
            fs::write(&full, bytes).expect("write owned file");
        }
    }
}

fn render_desc(pkg: &FakePkg<'_>) -> String {
    let mut out = String::new();
    push_block(&mut out, "NAME", &[pkg.name]);
    push_block(&mut out, "VERSION", &[pkg.version]);
    push_block(&mut out, "DESC", &[pkg.desc]);
    if let Some(url) = pkg.url {
        push_block(&mut out, "URL", &[url]);
    }
    push_block(&mut out, "ARCH", &[pkg.arch]);
    push_block(&mut out, "BUILDDATE", &[&FAKE_TIMESTAMP.to_string()]);
    push_block(&mut out, "INSTALLDATE", &[&FAKE_TIMESTAMP.to_string()]);
    push_block(&mut out, "PACKAGER", &[pkg.packager]);
    push_block(&mut out, "SIZE", &[&pkg.size.to_string()]);
    push_block(&mut out, "REASON", &[&pkg.reason.to_string()]);
    push_block(&mut out, "VALIDATION", &["none"]);
    if !pkg.licenses.is_empty() {
        push_block(&mut out, "LICENSE", pkg.licenses);
    }
    if !pkg.groups.is_empty() {
        push_block(&mut out, "GROUPS", pkg.groups);
    }
    if !pkg.depends.is_empty() {
        push_block(&mut out, "DEPENDS", pkg.depends);
    }
    if !pkg.provides.is_empty() {
        push_block(&mut out, "PROVIDES", pkg.provides);
    }
    if !pkg.optdepends.is_empty() {
        push_block(&mut out, "OPTDEPENDS", pkg.optdepends);
    }
    if !pkg.conflicts.is_empty() {
        push_block(&mut out, "CONFLICTS", pkg.conflicts);
    }
    if !pkg.replaces.is_empty() {
        push_block(&mut out, "REPLACES", pkg.replaces);
    }
    out
}

fn render_files_block(pkg: &FakePkg<'_>) -> String {
    let mut out = String::from("%FILES%\n");
    for (rel, _) in pkg.files {
        out.push_str(rel.trim_start_matches('/'));
        out.push('\n');
    }
    out.push('\n');
    out
}

fn push_block(out: &mut String, key: &str, values: &[&str]) {
    out.push('%');
    out.push_str(key);
    out.push_str("%\n");
    for v in values {
        out.push_str(v);
        out.push('\n');
    }
    out.push('\n');
}
