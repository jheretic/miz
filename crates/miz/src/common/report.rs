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
