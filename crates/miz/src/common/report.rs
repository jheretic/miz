//! Structured transaction plan + confirmation inversion, plus the per-operation
//! result types for the read-only verbs (query/files/deptest/database/version).
//! Core `run()` builds these; the render layer turns them into byte-for-byte
//! identical stdout/stderr. Errors that mattered for exit codes are recovered
//! via each report's `outcome()`.

use crate::error::{MizError, Result};

/// What a confirmer needs to render a summary and pick the prompt wording.
/// `targets` is the (name, version) list shown in the package summary (empty
/// for prompt-only confirmations like cache-clean or the images `-Iu` prompt);
/// `prompt` is the exact `[Y/n]` line.
pub struct TransactionPlan {
    pub targets: Vec<(String, String)>,
    /// Machine-readable operation kind, so a daemon can distinguish operations
    /// without parsing the English `prompt`. Not yet read by the TTY confirmer
    /// (which uses `prompt`); consumed by the future mizd daemon.
    #[allow(dead_code)]
    pub kind: TransactionKind,
    pub prompt: String,
}

/// The operation a [`TransactionPlan`] confirms. Mirrors the cases behind the
/// current prompt strings; the `prompt` field keeps the exact English wording.
pub enum TransactionKind {
    Install,
    Remove,
    DownloadOnly,
    CleanCache,
    ImageInstall,
}

impl TransactionPlan {
    /// A plan with a package summary (install/remove/upgrade).
    pub fn with_targets(targets: Vec<(String, String)>, kind: TransactionKind, prompt: &str) -> Self {
        TransactionPlan {
            targets,
            kind,
            prompt: prompt.to_string(),
        }
    }

    /// A prompt-only plan (no summary): cache-clean, images `-Iu`.
    pub fn prompt_only(kind: TransactionKind, prompt: &str) -> Self {
        TransactionPlan {
            targets: Vec::new(),
            kind,
            prompt: prompt.to_string(),
        }
    }
}

/// Confirmation inversion: core builds a [`TransactionPlan`] and asks the
/// confirmer whether to proceed; it never prompts or renders a summary itself.
pub trait Confirmer {
    fn confirm(&mut self, plan: &TransactionPlan) -> bool;
}

// ---------------------------------------------------------------------------
// version
// ---------------------------------------------------------------------------

pub struct VersionReport {
    pub miz: String,
    pub alpm: String,
}

// ---------------------------------------------------------------------------
// deptest
// ---------------------------------------------------------------------------

pub struct DeptestReport {
    /// Unsatisfied dependency tokens, in input order (printed to stdout).
    pub missing: Vec<String>,
}

impl DeptestReport {
    pub fn outcome(&self) -> Result<()> {
        if self.missing.is_empty() {
            Ok(())
        } else {
            Err(MizError::Deptest)
        }
    }
}

// ---------------------------------------------------------------------------
// database
// ---------------------------------------------------------------------------

pub enum DbReport {
    /// `-Dk`/`-Dkk`: per-problem stderr lines already formatted, the total error
    /// count, and whether `-q` suppresses the success line.
    Check {
        problems: Vec<String>,
        count: usize,
        quiet: bool,
    },
    /// `-D --asdeps`/`--asexplicit`: confirmation lines (stderr) for packages
    /// whose reason was set, plus the first not-found name (which aborted).
    SetReason {
        confirmations: Vec<String>,
        not_found: Option<String>,
        /// A `set_reason` ALPM failure on a later package: the confirmations
        /// already gathered are still rendered, then this error propagates,
        /// matching the original which printed each confirmation immediately.
        set_reason_error: Option<alpm::Error>,
    },
}

impl DbReport {
    pub fn outcome(&self) -> Result<()> {
        match self {
            DbReport::Check { count, .. } => {
                if *count == 0 {
                    Ok(())
                } else {
                    Err(MizError::DatabaseErrors(*count))
                }
            }
            DbReport::SetReason {
                not_found,
                set_reason_error,
                ..
            } => {
                if let Some(e) = set_reason_error {
                    return Err(MizError::Alpm(*e));
                }
                match not_found {
                    Some(name) => Err(MizError::PackageNotFound(name.clone())),
                    None => Ok(()),
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// files
// ---------------------------------------------------------------------------

/// One package's matched/owned files. `files` are raw bytes so the machine
/// readable (null-separated) output stays byte-exact.
pub struct FilePkg {
    pub db: String,
    pub pkg: String,
    pub version: String,
    pub files: Vec<Vec<u8>>,
    /// Search only: the target contained a `/`, so print "file is owned by ...".
    pub exact_file: bool,
}

pub enum FilesReport {
    List {
        machine: bool,
        quiet: bool,
        pkgs: Vec<FilePkg>,
        /// `-Fl` not-found targets: full stderr lines, printed after the body.
        diagnostics: Vec<String>,
        /// PackageNotFound argument (joined) if any target was missing.
        error: Option<String>,
    },
    Search {
        machine: bool,
        quiet: bool,
        matches: Vec<FilePkg>,
        /// PackageNotFound argument (joined) if any target found nothing.
        error: Option<String>,
        /// A regex compile failure on a LATER target: the matches gathered so
        /// far are still rendered, then this error propagates (generic exit),
        /// taking priority over the not-found error -- matching the original
        /// which returned on `Regex::new(..)?` before the not-found sweep.
        regex_error: Option<regex::Error>,
    },
}

impl FilesReport {
    pub fn outcome(&self) -> Result<()> {
        match self {
            FilesReport::List { error, .. } => match error {
                Some(joined) => Err(MizError::PackageNotFound(joined.clone())),
                None => Ok(()),
            },
            FilesReport::Search {
                error, regex_error, ..
            } => {
                if let Some(e) = regex_error {
                    return Err(MizError::Regex(e.clone()));
                }
                match error {
                    Some(joined) => Err(MizError::PackageNotFound(joined.clone())),
                    None => Ok(()),
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// query
// ---------------------------------------------------------------------------

/// One name/version listing row (localdb or image package alike).
pub struct PkgLine {
    pub name: String,
    pub version: String,
}

/// A single `-Qi` field. `Label` renders `{:<19}: {}`; `Backup` renders the
/// "Backup Files       :" header followed by one `path\thash` line each.
pub enum InfoField {
    Label { key: String, value: String },
    Backup(Vec<String>),
}

pub struct InfoBlock {
    pub fields: Vec<InfoField>,
}

/// One `-Ql` row: `pkg file` (or bare `file` when quiet).
pub struct FileLine {
    pub pkg: String,
    pub file: String,
}

/// One `-Qk` result: per-file problem lines (stderr) then a summary (stdout).
pub struct CheckResult {
    pub problems: Vec<String>,
    pub summary: String,
}

pub enum SearchSource {
    Local,
    Image,
}

/// One `-Qs` hit. `Local` always prints the description line (even empty);
/// `Image` prints it only when non-empty -- matching the current code.
pub struct SearchHit {
    pub source: SearchSource,
    pub name: String,
    pub version: String,
    pub desc: String,
}

pub enum QueryBody {
    List { quiet: bool, pkgs: Vec<PkgLine> },
    Info(Vec<InfoBlock>),
    Files { quiet: bool, lines: Vec<FileLine> },
    Check(Vec<CheckResult>),
    Changelog(Vec<String>),
    Search { quiet: bool, hits: Vec<SearchHit> },
    /// `-Qo`: "path is owned by name version" lines.
    Owns(Vec<String>),
    /// `-Qg`: pre-built "group pkg" / "group" lines.
    Groups(Vec<String>),
}

pub enum QueryError {
    NotFound(String),
    Other(String),
    /// A changelog `read_to_string` failure: reproduces the original
    /// `MizError::Io` (generic exit) with no "no changelog available" line.
    Io(String),
}

pub struct QueryReport {
    pub body: QueryBody,
    /// Diagnostic stderr lines emitted after the body (not-found targets, etc.).
    pub diagnostics: Vec<String>,
    /// Terminal error to propagate to main after rendering.
    pub error: Option<QueryError>,
}

impl QueryReport {
    pub fn outcome(&self) -> Result<()> {
        match &self.error {
            None => Ok(()),
            Some(QueryError::NotFound(s)) => Err(MizError::PackageNotFound(s.clone())),
            Some(QueryError::Other(s)) => Err(MizError::Other(s.clone())),
            Some(QueryError::Io(s)) => Err(MizError::Io(std::io::Error::other(s.clone()))),
        }
    }
}

// ---------------------------------------------------------------------------
// sync
// ---------------------------------------------------------------------------

/// Deferred output for a `-S` run. The refresh header/footer (`::
/// Synchronizing...` / ` package databases synchronized`) are NOT here: they
/// must interleave with the live download bars, so core emits them through the
/// progress sink. Everything else the old code println!'d becomes a variant.
pub enum SyncReport {
    /// `-Ss`: pre-built stdout lines (name/version + description), in order.
    Search { lines: Vec<String> },
    /// `-Sl`/`-Sg`/`-Si`: stdout body lines, plus colorized "error: {msg}"
    /// diagnostics for missing repos/groups/packages, plus a terminal
    /// PackageNotFound(joined) if any target was missing. Partial-results
    /// idiom: found listings render first, diagnostics next, error last.
    Listing {
        lines: Vec<String>,
        diagnostics: Vec<String>,
        error: Option<String>,
    },
    /// `-Sc`/`-Scc`: number of cache files removed (only when the user
    /// confirmed; a declined prompt yields `Done` with no output).
    Clean { removed: u64 },
    /// `-Sp` / `--print`: print-target lines, plus an optional colorized
    /// "warning: {msg}" if `trans_release` failed after printing.
    Print {
        lines: Vec<String>,
        release_warning: Option<String>,
    },
    /// Committed install, refresh-only, declined confirm, or nothing-to-do:
    /// no further render output (summary/progress/commit-errors already went
    /// through the confirmer/sink/transaction seams).
    Done,
}

impl SyncReport {
    pub fn outcome(&self) -> Result<()> {
        match self {
            SyncReport::Listing {
                error: Some(joined),
                ..
            } => Err(MizError::PackageNotFound(joined.clone())),
            _ => Ok(()),
        }
    }
}

// ---------------------------------------------------------------------------
// remove
// ---------------------------------------------------------------------------

/// Deferred output for a `-R` run. Structurally identical to
/// [`UpgradeReport`] today (both only defer `--print` output); kept distinct so
/// a daemon can diverge without a breaking change.
pub enum RemoveReport {
    /// `-Rp` / `--print`: print lines + optional uncolored release warning.
    Print {
        lines: Vec<String>,
        release_warning: Option<String>,
    },
    /// Committed removal / declined / nothing-to-do: no further render output.
    Done,
}

impl RemoveReport {
    pub fn outcome(&self) -> Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// upgrade
// ---------------------------------------------------------------------------

/// Deferred output for a `-U` run. See [`RemoveReport`] on the shared shape.
pub enum UpgradeReport {
    /// `-Up` / `--print`: print lines + optional uncolored release warning.
    Print {
        lines: Vec<String>,
        release_warning: Option<String>,
    },
    /// Committed install / declined / nothing-to-do: no further render output.
    Done,
}

impl UpgradeReport {
    pub fn outcome(&self) -> Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// images
// ---------------------------------------------------------------------------

/// One `-Il` row: a concrete version plus its `[installed]`/`[newest]` markers.
/// Marker computation (per-version Describe) is core; the render layer turns
/// this into the `{component} {version}{suffix}` line.
pub struct ImageListRow {
    pub version: String,
    pub installed: bool,
    pub newest: bool,
}

/// Outcome of the relay sub-step (standalone `--reinstall-layered` or the
/// automatic post-`-Iu` relay). The `-Syu` transaction + teardown/prune
/// warnings run live during core; only the trailing textual status defers here.
pub enum RelayReport {
    /// `--dry-run`: the plan preview (already newline-terminated, printed verbatim).
    DryRun(String),
    /// Live relay finished; render prints "relayed layered packages onto {subvol}".
    Relayed { subvol: String, quiet: bool },
}

/// Outcome of `-Iu`. Bars (acquire/install/relay `-Syu`) already went through
/// the progress sink and the prompt through the confirmer; only the trailing
/// status line(s) defer to render.
pub enum ImageUpgradeOutcome {
    /// Nothing to do; render emits "{name}: already up to date" to stderr.
    AlreadyUpToDate { name: String, quiet: bool },
    /// The confirmation was declined: no output.
    Declined,
    /// Install completed. `host_changed` picks "updated to" vs the in-place
    /// completion wording; `relay` is present only for the host component when
    /// the version actually advanced. `reboot` requests a post-render reboot
    /// (deferred to main so the upgrade/relay lines print BEFORE reboot, as the
    /// pre-refactor code did). `error` carries a terminal relay failure so
    /// render prints the completed upgrade line first, then it propagates.
    Done {
        name: String,
        version: String,
        host_changed: bool,
        quiet: bool,
        relay: Option<RelayReport>,
        reboot: bool,
        error: Option<String>,
    },
}

/// Per-sub-op result for `miz -I`. Each variant carries exactly the values the
/// old inline print sites emitted; `render::images` reproduces the stdout/stderr
/// byte-for-byte. `Json` is the `--json=short/pretty` passthrough (emitted
/// verbatim before any parse).
pub enum ImagesReport {
    /// `-Il`: rows for a resolved component.
    List {
        component: String,
        quiet: bool,
        rows: Vec<ImageListRow>,
    },
    /// `-Ii`/`-If` `--json` passthrough: the payload string, printed verbatim.
    Json(String),
    /// `-Ii`/`-If`: label fields followed by a trailing blank line.
    Info(Vec<InfoField>),
    /// `-Iy`: `newest` is `None` when nothing newer is available.
    CheckNew {
        name: String,
        quiet: bool,
        newest: Option<String>,
    },
    /// `-Ig`: (class, name) rows.
    Components {
        quiet: bool,
        rows: Vec<(String, String)>,
    },
    /// `-Ip`: reboot-pending status.
    Pending {
        name: String,
        quiet: bool,
        installed: String,
        booted_label: String,
        installed_label: String,
        reboot_due: bool,
    },
    /// `-If` bare: plain feature id lines.
    FeatureList(Vec<String>),
    /// `--enable`/`--disable`: confirmation line to stdout when not quiet.
    FeatureToggle {
        name: String,
        feature: String,
        enabled: bool,
        quiet: bool,
    },
    /// `--appstream`: catalog URLs.
    AppStream(Vec<String>),
    /// `-Iu`.
    Upgrade(ImageUpgradeOutcome),
    /// `-Ic`.
    Vacuum {
        name: String,
        quiet: bool,
        instances: u32,
        disabled: u32,
    },
    /// `--reinstall-layered` (standalone) or the automatic relay.
    Relay(RelayReport),
    /// No output (e.g. `--reboot`).
    Silent,
}

impl ImagesReport {
    pub fn outcome(&self) -> Result<()> {
        // A relay failure after a completed install is carried here so render
        // prints the "updated to" line first; propagate it now with the same
        // generic exit as the pre-refactor `?`.
        if let ImagesReport::Upgrade(ImageUpgradeOutcome::Done {
            error: Some(e), ..
        }) = self
        {
            return Err(MizError::Sysupdate(e.clone()));
        }
        Ok(())
    }

    /// Whether a post-render reboot was requested (`-Iu --reboot`). main
    /// triggers it AFTER rendering so the upgrade/relay lines print first.
    pub fn wants_reboot(&self) -> bool {
        matches!(
            self,
            ImagesReport::Upgrade(ImageUpgradeOutcome::Done { reboot: true, .. })
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scripted confirmer: returns queued answers, recording the prompts it
    /// was asked. Proves the Confirmer seam without a TTY.
    struct ScriptedConfirmer {
        answers: Vec<bool>,
        seen: Vec<String>,
    }

    impl Confirmer for ScriptedConfirmer {
        fn confirm(&mut self, plan: &TransactionPlan) -> bool {
            self.seen.push(plan.prompt.clone());
            if self.answers.is_empty() {
                true
            } else {
                self.answers.remove(0)
            }
        }
    }

    #[test]
    fn scripted_confirmer_returns_queued_answers_and_records_prompts() {
        let mut c = ScriptedConfirmer {
            answers: vec![true, false],
            seen: Vec::new(),
        };
        let plan = TransactionPlan::with_targets(
            vec![("bash".into(), "5.2-1".into())],
            TransactionKind::Install,
            "Proceed with installation? [Y/n] ",
        );
        assert!(c.confirm(&plan));
        assert!(!c.confirm(&TransactionPlan::prompt_only(
            TransactionKind::Remove,
            "Do you want to remove these packages? [Y/n] "
        )));
        assert_eq!(
            c.seen,
            vec![
                "Proceed with installation? [Y/n] ".to_string(),
                "Do you want to remove these packages? [Y/n] ".to_string(),
            ]
        );
    }

    #[test]
    fn sync_report_outcome_propagates_only_listing_error() {
        assert!(SyncReport::Done.outcome().is_ok());
        assert!(SyncReport::Search { lines: vec![] }.outcome().is_ok());
        assert!(SyncReport::Clean { removed: 3 }.outcome().is_ok());
        assert!(SyncReport::Listing {
            lines: vec![],
            diagnostics: vec![],
            error: None,
        }
        .outcome()
        .is_ok());
        let err = SyncReport::Listing {
            lines: vec![],
            diagnostics: vec!["package 'x' was not found".into()],
            error: Some("x".into()),
        }
        .outcome();
        assert!(matches!(err, Err(MizError::PackageNotFound(s)) if s == "x"));
    }

    #[test]
    fn remove_upgrade_reports_never_error() {
        assert!(RemoveReport::Done.outcome().is_ok());
        assert!(UpgradeReport::Done.outcome().is_ok());
    }

    #[test]
    fn transaction_plan_shapes() {
        let p = TransactionPlan::with_targets(
            vec![("a".into(), "1".into())],
            TransactionKind::Install,
            "go? ",
        );
        assert_eq!(p.targets.len(), 1);
        assert_eq!(p.prompt, "go? ");
        let q = TransactionPlan::prompt_only(TransactionKind::CleanCache, "clean? ");
        assert!(q.targets.is_empty());
        assert_eq!(q.prompt, "clean? ");
    }
}
