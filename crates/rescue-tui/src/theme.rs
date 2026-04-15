//! Color theming for the rescue TUI.
//!
//! Operators can override the default palette with `AEGIS_THEME=<name>`
//! (set in the kernel cmdline as `aegis.theme=<name>` and propagated by
//! /init, or exported manually from a debug shell). Themes are
//! intentionally limited to a small set — the rescue environment runs
//! against unknown TTYs (serial, framebuffer, OVMF console) where some
//! palettes render unreadably.

use ratatui::style::Color;

/// Named palette controlling foreground colors for status spans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Verified / success state (checksum ok, signature trusted).
    pub success: Color,
    /// Warning / soft failure (untrusted signer, parse error).
    pub warning: Color,
    /// Hard failure (mismatch, forged signature).
    pub error: Color,
}

impl Theme {
    /// Default 16-color palette suitable for most VT100-class consoles.
    #[must_use]
    pub const fn default_theme() -> Self {
        Self {
            success: Color::Green,
            warning: Color::Yellow,
            error: Color::Red,
        }
    }

    /// Monochrome palette — every status renders as the terminal's
    /// default foreground. Useful on serial consoles whose ANSI color
    /// support is unreliable or where a screen reader strips color.
    #[must_use]
    pub const fn monochrome() -> Self {
        Self {
            success: Color::Reset,
            warning: Color::Reset,
            error: Color::Reset,
        }
    }

    /// High-contrast palette — bright variants only, for low-contrast
    /// framebuffer consoles (OVMF default font, some HDMI capture cards).
    #[must_use]
    pub const fn high_contrast() -> Self {
        Self {
            success: Color::LightGreen,
            warning: Color::LightYellow,
            error: Color::LightRed,
        }
    }

    /// Resolve a theme name (case-insensitive). Unknown names fall back
    /// to the default palette so a typo never bricks the TUI.
    #[must_use]
    pub fn from_name(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "monochrome" | "mono" | "none" => Self::monochrome(),
            "high-contrast" | "high_contrast" | "hc" => Self::high_contrast(),
            _ => Self::default_theme(),
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::default_theme()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_uses_standard_ansi_colors() {
        let t = Theme::default_theme();
        assert_eq!(t.success, Color::Green);
        assert_eq!(t.warning, Color::Yellow);
        assert_eq!(t.error, Color::Red);
    }

    #[test]
    fn monochrome_resets_all_slots() {
        let t = Theme::monochrome();
        assert_eq!(t.success, Color::Reset);
        assert_eq!(t.warning, Color::Reset);
        assert_eq!(t.error, Color::Reset);
    }

    #[test]
    fn from_name_is_case_insensitive_and_accepts_aliases() {
        assert_eq!(Theme::from_name("monochrome"), Theme::monochrome());
        assert_eq!(Theme::from_name("MONO"), Theme::monochrome());
        assert_eq!(Theme::from_name("none"), Theme::monochrome());
        assert_eq!(Theme::from_name("high-contrast"), Theme::high_contrast());
        assert_eq!(Theme::from_name("HC"), Theme::high_contrast());
    }

    #[test]
    fn from_name_falls_back_to_default_on_unknown() {
        assert_eq!(Theme::from_name(""), Theme::default_theme());
        assert_eq!(Theme::from_name("solarized-dark"), Theme::default_theme());
        assert_eq!(Theme::from_name("xyzzy"), Theme::default_theme());
    }
}
