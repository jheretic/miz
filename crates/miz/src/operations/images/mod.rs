//! `miz -I` / `--images` — manage OS image updates via systemd-sysupdated
//! over D-Bus. Dispatch is priority-ordered (read-only verbs first, mutating
//! verbs last — deliberately NOT a mirror of `sync::run`, which checks clean
//! first). Context-less (no alpm handle).
//!
//! Phase 1: scaffold only. Every mode returns `NotImplemented`.

#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod describe;
#[allow(dead_code)]
mod format;
#[allow(dead_code)]
mod job;

use crate::error::{MizError, Result};

pub use crate::cli::args::images::Args;

pub fn run(args: Args) -> Result<()> {
    if args.list {
        return images_list(&args);
    }
    if args.info > 0 {
        return images_info(&args);
    }
    if args.check_new {
        return images_check_new(&args);
    }
    if args.components {
        return images_components(&args);
    }
    if args.pending {
        return images_pending(&args);
    }
    if args.features {
        return images_features(&args);
    }
    if args.upgrade > 0 {
        return images_upgrade(&args);
    }
    if args.clean > 0 {
        return images_vacuum(&args);
    }
    if args.reboot {
        return images_reboot(&args);
    }

    eprintln!("miz: -I/--images is not yet implemented");
    Err(MizError::NotImplemented)
}

/// Split a positional target into `(component, Option<version>)`, mirroring
/// `sync::split_repo_target`'s `repo/pkg` idiom. Defaults to component `"host"`.
/// Wired into the read-only verbs in phase 2; only tests exercise it now.
#[allow(dead_code)]
fn split_component(target: Option<&str>) -> (&str, Option<&str>) {
    match target {
        Some(t) => match t.split_once('/') {
            Some((comp, ver)) => (comp, Some(ver)),
            None => (t, None),
        },
        None => ("host", None),
    }
}

fn images_list(_args: &Args) -> Result<()> {
    Err(MizError::NotImplemented)
}

fn images_info(_args: &Args) -> Result<()> {
    Err(MizError::NotImplemented)
}

fn images_check_new(_args: &Args) -> Result<()> {
    Err(MizError::NotImplemented)
}

fn images_components(_args: &Args) -> Result<()> {
    Err(MizError::NotImplemented)
}

fn images_pending(_args: &Args) -> Result<()> {
    Err(MizError::NotImplemented)
}

fn images_features(_args: &Args) -> Result<()> {
    Err(MizError::NotImplemented)
}

fn images_upgrade(_args: &Args) -> Result<()> {
    Err(MizError::NotImplemented)
}

fn images_vacuum(_args: &Args) -> Result<()> {
    Err(MizError::NotImplemented)
}

fn images_reboot(_args: &Args) -> Result<()> {
    Err(MizError::NotImplemented)
}

#[cfg(test)]
mod tests {
    use super::split_component;

    #[test]
    fn split_defaults_to_host() {
        assert_eq!(split_component(None), ("host", None));
    }

    #[test]
    fn split_bare_component() {
        assert_eq!(split_component(Some("foo")), ("foo", None));
    }

    #[test]
    fn split_component_version() {
        assert_eq!(split_component(Some("host/2.3")), ("host", Some("2.3")));
    }
}
