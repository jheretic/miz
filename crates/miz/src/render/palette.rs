//! Terminal color styling for miz output.
//!
//! Color is OFF by default (built-in), ON in the shipped vendor config
//! (`/usr/lib/miz/miz.toml` sets `[options] color = true`). The effective choice
//! is resolved ONCE into a [`Palette`] in `main.rs` (from the loaded config's
//! `color` option), then threaded into the operations. Resolution precedence
//! (strongest first):
//!
//! 1. `NO_COLOR` set to a non-empty value (https://no-color.org/) -> off.
//! 2. Output stream is not a TTY (piped/redirected) -> off.
//! 3. Otherwise -> the config's `[options] color` value.
//!
//! Styles are held as [`console::Style`]; when color is disabled every style is
//! a no-op (`Style::new()` with no attributes), so call sites can style
//! unconditionally without branching.

use console::Style;
use std::io::IsTerminal;

/// The resolved set of styles for one run. Cheap to clone (each field is a small
/// `console::Style`). Construct via [`Palette::resolve`].
#[derive(Clone)]
pub struct Palette {
    /// `error:` prefix (bold red).
    pub error: Style,
    /// `warning:` prefix (bold yellow).
    pub warning: Style,
    /// `::` / `==>` status lines (bold blue).
    pub status: Style,
    /// Section headers / field labels (bold).
    pub header: Style,
    /// Package names (cyan).
    pub package: Style,
    /// Version strings (green).
    pub version: Style,
}

impl Palette {
    /// Resolve the palette from the config's `color` flag plus the environment
    /// and whether the output is a TTY. `color` is `options.color` from the
    /// loaded config. Keyed on STDERR's TTY-ness: every colorized sink in miz
    /// (the transaction summary, `::` status lines, progress bars) writes to
    /// stderr, so `miz -S ... | pager` must still colorize the visible stderr
    /// stream even though stdout is piped.
    pub fn resolve(color: bool) -> Self {
        Self::with_enabled(color_enabled(color, std::io::stderr().is_terminal()))
    }

    fn with_enabled(enabled: bool) -> Self {
        // console::Style::force styling on/off per style so the decision is
        // baked in and call sites never branch. When disabled, `Style::new()`
        // with force(false) renders text verbatim.
        let s = || Style::new().force_styling(enabled);
        Palette {
            error: s().red().bold(),
            warning: s().yellow().bold(),
            status: s().blue().bold(),
            header: s().bold(),
            package: s().cyan(),
            version: s().green(),
        }
    }
}

/// Pure resolution of the color decision, split out for unit testing:
/// NO_COLOR wins, then non-TTY forces off, else the config value.
fn color_enabled(config_color: bool, is_tty: bool) -> bool {
    if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    if !is_tty {
        return false;
    }
    config_color
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_tty_forces_off_regardless_of_config() {
        // NO_COLOR is process-global; this case is independent of it because a
        // non-tty short-circuits after the NO_COLOR check only when NO_COLOR is
        // unset. Guard by asserting the non-tty branch directly.
        assert!(!color_enabled(true, false));
        assert!(!color_enabled(false, false));
    }

    #[test]
    fn tty_follows_config_when_no_color_unset() {
        if std::env::var_os("NO_COLOR").is_some() {
            return; // environment forces off; skip the config-follows assertion
        }
        assert!(color_enabled(true, true));
        assert!(!color_enabled(false, true));
    }

    #[test]
    fn disabled_palette_renders_plain() {
        // A forced-off style leaves text unchanged (no ANSI escapes).
        let p = Palette::with_enabled(false);
        assert_eq!(p.error.apply_to("boom").to_string(), "boom");
    }

    #[test]
    fn enabled_palette_emits_ansi() {
        let p = Palette::with_enabled(true);
        let rendered = p.error.apply_to("boom").to_string();
        assert!(
            rendered.contains('\u{1b}'),
            "expected ANSI escape: {rendered:?}"
        );
        assert!(rendered.contains("boom"));
    }
}
