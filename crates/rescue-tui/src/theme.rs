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

    /// Okabe-Ito colorblind-safe palette — no red-on-green status pairs
    /// that trip deuteranopia/protanopia. Green → bluish-green (#009E73),
    /// warning → orange (#E69F00), error → vermillion (#D55E00). See
    /// jfly.uni-koeln.de/color. (#93)
    #[must_use]
    pub const fn okabe_ito() -> Self {
        Self {
            success: Color::Rgb(0x00, 0x9E, 0x73),
            warning: Color::Rgb(0xE6, 0x9F, 0x00),
            error: Color::Rgb(0xD5, 0x5E, 0x00),
        }
    }

    /// aegis brand palette (#76). Matches assets/brand/BRAND.md — steel
    /// blue primary, emerald success, amber warning, vermillion error.
    /// All five tested against deuteranopia/protanopia and distinct
    /// from Ubuntu/Fedora/Arch distro palettes.
    #[must_use]
    pub const fn aegis() -> Self {
        Self {
            success: Color::Rgb(0x22, 0xC5, 0x5E),
            warning: Color::Rgb(0xEA, 0xB3, 0x08),
            error: Color::Rgb(0xEF, 0x44, 0x44),
        }
    }

    /// Resolve a theme name (case-insensitive). Unknown names fall back
    /// to the default palette so a typo never bricks the TUI.
    #[must_use]
    pub fn from_name(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "monochrome" | "mono" | "none" => Self::monochrome(),
            "high-contrast" | "high_contrast" | "hc" => Self::high_contrast(),
            "okabe-ito" | "okabe_ito" | "okabeito" | "cb" | "colorblind" => Self::okabe_ito(),
            "aegis" | "brand" => Self::aegis(),
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
        assert_eq!(Theme::from_name("okabe-ito"), Theme::okabe_ito());
        assert_eq!(Theme::from_name("colorblind"), Theme::okabe_ito());
        assert_eq!(Theme::from_name("cb"), Theme::okabe_ito());
        assert_eq!(Theme::from_name("aegis"), Theme::aegis());
        assert_eq!(Theme::from_name("BRAND"), Theme::aegis());
    }

    #[test]
    fn aegis_theme_uses_brand_hex_values() {
        use ratatui::style::Color;
        let t = Theme::aegis();
        // Steel-blue brand colour lives on the widget border, not the
        // Theme struct (Theme holds verdict colours). These three are
        // the verdict trio from assets/brand/BRAND.md.
        assert_eq!(t.success, Color::Rgb(0x22, 0xC5, 0x5E));
        assert_eq!(t.warning, Color::Rgb(0xEA, 0xB3, 0x08));
        assert_eq!(t.error, Color::Rgb(0xEF, 0x44, 0x44));
    }

    #[test]
    fn okabe_ito_uses_colorblind_safe_rgb() {
        use ratatui::style::Color;
        let t = Theme::okabe_ito();
        // Bluish-green, orange, vermillion — the three non-primary
        // Okabe-Ito colors that remain distinguishable under
        // deuteranopia / protanopia.
        assert_eq!(t.success, Color::Rgb(0x00, 0x9E, 0x73));
        assert_eq!(t.warning, Color::Rgb(0xE6, 0x9F, 0x00));
        assert_eq!(t.error, Color::Rgb(0xD5, 0x5E, 0x00));
    }

    #[test]
    fn from_name_falls_back_to_default_on_unknown() {
        assert_eq!(Theme::from_name(""), Theme::default_theme());
        assert_eq!(Theme::from_name("solarized-dark"), Theme::default_theme());
        assert_eq!(Theme::from_name("xyzzy"), Theme::default_theme());
    }
}
