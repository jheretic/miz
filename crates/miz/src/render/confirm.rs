//! TTY confirmation: the bin-side [`Confirmer`] that renders the package
//! summary (moved here from `common/transaction::print_summary`) then reads the
//! `[Y/n]` prompt. Honors `noconfirm` (auto-yes) and non-TTY stdin exactly as
//! the old `should_prompt` + `confirm` did.

use crate::common::report::{Confirmer, TransactionPlan};
use crate::render::palette::Palette;
use std::io::{IsTerminal, Write};

pub struct TtyConfirmer {
    palette: Palette,
    noconfirm: bool,
}

impl TtyConfirmer {
    pub fn new(palette: Palette, noconfirm: bool) -> Self {
        TtyConfirmer { palette, noconfirm }
    }

    fn render_summary(&self, targets: &[(String, String)]) {
        let total = targets.len();
        eprintln!();
        eprintln!(
            "{}",
            self.palette.header.apply_to(format!("Packages ({total}):"))
        );
        let mut buf = String::new();
        for (name, version) in targets {
            if !buf.is_empty() {
                buf.push(' ');
            }
            buf.push_str(&self.palette.package.apply_to(name).to_string());
            buf.push('-');
            buf.push_str(&self.palette.version.apply_to(version).to_string());
        }
        eprintln!("{buf}");
        eprintln!();
    }

    /// Whether to actually prompt: only when not `--noconfirm` and stdin is a
    /// TTY. Otherwise the confirmer auto-yes'es.
    fn should_prompt(&self) -> bool {
        !self.noconfirm && std::io::stdin().is_terminal()
    }

    fn read_yes_no(prompt: &str) -> bool {
        let mut stderr = std::io::stderr();
        let _ = write!(stderr, "{prompt}");
        let _ = stderr.flush();
        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_err() {
            return false;
        }
        let trimmed = input.trim();
        trimmed.is_empty()
            || trimmed.eq_ignore_ascii_case("y")
            || trimmed.eq_ignore_ascii_case("yes")
    }
}

impl Confirmer for TtyConfirmer {
    fn confirm(&mut self, plan: &TransactionPlan) -> bool {
        if !plan.targets.is_empty() {
            self.render_summary(&plan.targets);
        }
        if self.should_prompt() {
            Self::read_yes_no(&plan.prompt)
        } else {
            true
        }
    }
}
