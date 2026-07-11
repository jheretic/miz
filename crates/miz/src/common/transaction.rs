use crate::error::{MizError, Result};
use alpm::{Alpm, CommitData, CommitError, Package, PrepareData, PrepareError, TransFlag};
use std::io::{IsTerminal, Write};
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering};

/// Raw alpm handle pointer for use by the signal handler.
///
/// `TransGuard::new` stores the active handle here; `TransGuard::release` and
/// `Drop` clear it. The signal handler reads this pointer to call
/// `alpm_trans_interrupt` (and, on failure, `alpm_trans_release` + `alpm_unlock`)
/// so that mid-transaction Ctrl-C / SIGTERM does not leak `/var/lib/pacman/db.lck`.
///
/// Safety: `alpm_handle_t` is opaque (typedef'd to `u8` in alpm-sys); the raw
/// pointer is just an address, which is `Send`. libalpm's `alpm_trans_interrupt`
/// is the C-side primitive specifically designed for signal-handler use and is
/// safe to call concurrently with an in-progress transaction on the main thread.
static ALPM_HANDLE: AtomicPtr<alpm_sys::alpm_handle_t> = AtomicPtr::new(ptr::null_mut());

pub struct TransGuard<'a> {
    alpm: &'a mut Alpm,
    released: bool,
}

impl<'a> TransGuard<'a> {
    pub fn new(alpm: &'a mut Alpm, flags: TransFlag) -> Result<Self> {
        alpm.trans_init(flags)?;
        // Stash the raw handle for the signal handler.
        ALPM_HANDLE.store(alpm.as_alpm_handle_t(), Ordering::SeqCst);
        Ok(TransGuard {
            alpm,
            released: false,
        })
    }

    pub fn alpm(&mut self) -> &mut Alpm {
        self.alpm
    }

    pub fn release(mut self) -> Result<()> {
        ALPM_HANDLE.store(ptr::null_mut(), Ordering::SeqCst);
        match self.alpm.trans_release() {
            Ok(()) => {
                self.released = true;
                Ok(())
            }
            Err(e) => {
                // Leave released = false so Drop retries with the raw FFI
                // path as a last resort. ALPM_HANDLE is already cleared.
                Err(MizError::from(e))
            }
        }
    }
}

impl Drop for TransGuard<'_> {
    fn drop(&mut self) {
        // Clear before releasing so a racing signal handler doesn't try to
        // release a transaction that's already being torn down.
        ALPM_HANDLE.store(ptr::null_mut(), Ordering::SeqCst);
        if !self.released {
            if let Err(e) = self.alpm.trans_release() {
                eprintln!("warning: trans_release failed during cleanup: {e}");
                // SAFETY: alpm handle is owned by Alpm; we have &mut self
                // so no other thread holds a reference. Force-clear the
                // lockfile so a future run is not blocked by db.lck.
                let p = self.alpm.as_alpm_handle_t();
                unsafe {
                    let _ = alpm_sys::alpm_unlock(p);
                }
            }
        }
    }
}

/// Install a SIGINT/SIGTERM/SIGHUP handler that interrupts an in-progress
/// libalpm transaction so the database lockfile is released cleanly.
///
/// Mirrors pacman's `soft_interrupt_handler` (see
/// `pacman/src/pacman/sighandler.c`): on signal, if a transaction is active
/// the handler asks libalpm to interrupt it and lets the main thread unwind
/// normally (which runs `TransGuard::Drop` → `alpm_trans_release`). If
/// interrupt fails or no transaction is active, the handler force-releases
/// the lock via raw FFI and calls `_exit(130)` (128 + SIGINT).
///
/// Returns `Ok(())` on success. Callers should propagate failure; missing a
/// handler is non-fatal (the program still works, just leaks the lock on
/// signal).
pub fn install_signal_handler() -> std::result::Result<(), ctrlc::Error> {
    ctrlc::set_handler(|| {
        let p = ALPM_HANDLE.load(Ordering::SeqCst);
        if !p.is_null() {
            // SAFETY: `p` is a valid alpm handle owned by the main thread for
            // the lifetime of the current TransGuard. `alpm_trans_interrupt`
            // is designed for cross-thread/signal use.
            let r = unsafe { alpm_sys::alpm_trans_interrupt(p) };
            if r == 0 {
                // Interrupt accepted: let the main thread observe the error
                // from its in-progress alpm call and unwind through normal
                // Drop. Do NOT exit here.
                return;
            }
            // Interrupt failed (no in-progress operation, or libalpm refused);
            // tear the lock down ourselves so we don't leak db.lck.
            unsafe {
                let _ = alpm_sys::alpm_trans_release(p);
                let _ = alpm_sys::alpm_unlock(p);
            }
        }
        // SIGINT exit code: 128 + 2 = 130. We don't distinguish SIGTERM/SIGHUP
        // here; v0.1 reports SIGINT for all soft signals (matches pacman's
        // behavior of exiting with whatever the caught signum is, but we don't
        // get the signum back from the ctrlc crate without writing a custom
        // sigaction loop).
        std::process::exit(130);
    })
}

pub(crate) fn prepare(alpm: &mut Alpm) -> Result<()> {
    match alpm.trans_prepare() {
        Ok(()) => Ok(()),
        Err(pe) => {
            report_prepare_error(&pe);
            Err(MizError::Alpm(pe.error()))
        }
    }
}

pub(crate) fn commit(alpm: &mut Alpm) -> Result<()> {
    match alpm.trans_commit() {
        Ok(()) => Ok(()),
        Err(ce) => {
            report_commit_error(&ce);
            Err(MizError::Alpm(ce.error()))
        }
    }
}

pub(crate) fn report_prepare_error(pe: &PrepareError<'_>) {
    match pe.data() {
        Some(PrepareData::UnsatisfiedDeps(deps)) => {
            for d in deps {
                match d.causing_pkg() {
                    Some(cause) => eprintln!(
                        "error: removing '{}' breaks dependency '{}' required by '{}'",
                        cause,
                        d.depend(),
                        d.target()
                    ),
                    None => eprintln!(
                        "error: unsatisfied dependency '{}' required by '{}'",
                        d.depend(),
                        d.target()
                    ),
                }
            }
        }
        Some(PrepareData::ConflictingDeps(conflicts)) => {
            for c in conflicts {
                eprintln!(
                    "error: conflict between '{}' and '{}'",
                    c.package1().name(),
                    c.package2().name()
                );
            }
        }
        Some(PrepareData::PkgInvalidArch(pkgs)) => {
            for p in pkgs {
                eprintln!(
                    "error: package '{}' has an invalid architecture: {}",
                    p.name(),
                    p.arch().unwrap_or("?")
                );
            }
        }
        None => eprintln!("error: failed to prepare transaction: {}", pe.error()),
    }
}

pub(crate) fn report_commit_error(ce: &CommitError) {
    match ce.data() {
        Some(CommitData::FileConflict(_)) => {
            eprintln!("error: file conflicts detected; aborting");
        }
        Some(CommitData::PkgInvalid(names)) => {
            for n in names {
                eprintln!("error: invalid package: {n}");
            }
        }
        None => eprintln!("error: failed to commit transaction: {}", ce.error()),
    }
}

pub(crate) fn confirm(prompt: &str) -> bool {
    let mut stderr = std::io::stderr();
    let _ = write!(stderr, "{prompt}");
    let _ = stderr.flush();
    let mut input = String::new();
    if std::io::stdin().read_line(&mut input).is_err() {
        return false;
    }
    let trimmed = input.trim();
    trimmed.is_empty() || trimmed.eq_ignore_ascii_case("y") || trimmed.eq_ignore_ascii_case("yes")
}

pub(crate) fn should_prompt(noconfirm: bool) -> bool {
    !noconfirm && std::io::stdin().is_terminal()
}

pub(crate) fn render_format(fmt: &str, pkg: &Package) -> String {
    let mut out = String::with_capacity(fmt.len());
    let mut chars = fmt.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push_str(pkg.name()),
            Some('v') => out.push_str(pkg.version().as_str()),
            Some('%') => out.push('%'),
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => out.push('%'),
        }
    }
    out
}

pub(crate) fn format_print_line(pkg: &Package, format: Option<&str>) -> String {
    match format {
        None => format!("{} {}", pkg.name(), pkg.version()),
        Some(fmt) => render_format(fmt, pkg),
    }
}

pub(crate) fn print_summary(targets: &[(String, String)], palette: &crate::render::palette::Palette) {
    let total = targets.len();
    eprintln!();
    eprintln!(
        "{}",
        palette.header.apply_to(format!("Packages ({total}):"))
    );
    let mut buf = String::new();
    for (name, version) in targets {
        if !buf.is_empty() {
            buf.push(' ');
        }
        buf.push_str(&palette.package.apply_to(name).to_string());
        buf.push('-');
        buf.push_str(&palette.version.apply_to(version).to_string());
    }
    eprintln!("{buf}");
    eprintln!();
}

pub(crate) fn collect_pkgs<'a, I: IntoIterator<Item = &'a Package>>(
    it: I,
) -> Vec<(String, String)> {
    it.into_iter()
        .map(|p| (p.name().to_string(), p.version().as_str().to_string()))
        .collect()
}
