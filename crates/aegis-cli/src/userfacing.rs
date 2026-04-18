//! `UserFacing` — structured, operator-friendly errors.
//!
//! Replaces the today-pattern of free-text error strings with a typed
//! contract every operator-visible error implements. Each error
//! provides:
//!
//!   * **summary** — one-line headline suitable for a header
//!   * **detail** — one paragraph explaining what specifically went
//!     wrong, including the inputs the system was processing
//!   * **suggestion** — optional, operator-actionable next step
//!   * **`docs_url`** — optional pointer to deeper documentation
//!   * **code** — optional stable identifier (e.g. `FLASH_WRITE_FAILED`)
//!     for tooling that keys off identifiers rather than free text
//!
//! `summary()` is named to avoid colliding with `std::error::Error::cause()`
//! (deprecated since 1.33 in favor of `source()`), which would otherwise
//! cause method-resolution ambiguity on `&dyn UserFacing`.
//!
//! No callers wired up in this PR. Tracked in #247.
//!
//! # Why an in-house renderer instead of `miette`
//!
//! `miette` is a good fit at scale, but pulling it in for the
//! foundation PR would be a 30-crate dependency change. Shipping the
//! trait + a plain renderer first keeps the PR small and lets each
//! per-command rollout (`flash`, `update`, `add`, `init`, `expand`)
//! land independently. Switching the renderer to `miette` later is a
//! one-file change that doesn't touch any error implementations.

use std::fmt;

/// Trait implemented by structured operator errors.
///
/// Implementors must also implement `std::error::Error` so the standard
/// library's error machinery (source chains, `?`, `Box<dyn Error>`)
/// continues to work.
pub trait UserFacing: std::error::Error {
    /// One-line summary suitable for the top of an error block.
    fn summary(&self) -> &str;

    /// One paragraph explaining what specifically went wrong, the
    /// inputs the system was processing, and the proximal failure.
    fn detail(&self) -> &str;

    /// Optional operator-actionable next step. Multi-line allowed.
    fn suggestion(&self) -> Option<&str> {
        None
    }

    /// Optional pointer to deeper documentation.
    fn docs_url(&self) -> Option<&str> {
        None
    }

    /// Optional stable identifier (e.g. `FLASH_WRITE_FAILED`) for
    /// tooling. Conventionally `SCREAMING_SNAKE_CASE`.
    fn code(&self) -> Option<&str> {
        None
    }
}

/// Render a `UserFacing` error into a `fmt::Formatter`. Use this when
/// implementing `fmt::Display` on a top-level error wrapper.
///
/// # Errors
///
/// Returns the underlying formatter error if writing fails.
#[allow(dead_code)] // Sibling of `render_string`. No `Display` impl uses
                    // this yet; kept so the two render paths stay in
                    // sync via the `display_via_render_matches_render_string`
                    // test and become importable when `Display` integration
                    // appears on an error wrapper.
pub fn render(err: &dyn UserFacing, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    if let Some(code) = err.code() {
        writeln!(f, "error[{code}]: {}", err.summary())?;
    } else {
        writeln!(f, "error: {}", err.summary())?;
    }
    writeln!(f, "  what happened: {}", err.detail())?;
    if let Some(s) = err.suggestion() {
        writeln!(f, "  try: {s}")?;
    }
    if let Some(u) = err.docs_url() {
        writeln!(f, "  see: {u}")?;
    }
    Ok(())
}

/// Render a `UserFacing` error to a `String`. Convenience wrapper for
/// callers that want to write the result to `stderr` directly.
#[must_use]
pub fn render_string(err: &dyn UserFacing) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    if let Some(code) = err.code() {
        let _ = writeln!(s, "error[{code}]: {}", err.summary());
    } else {
        let _ = writeln!(s, "error: {}", err.summary());
    }
    let _ = writeln!(s, "  what happened: {}", err.detail());
    if let Some(sug) = err.suggestion() {
        let _ = writeln!(s, "  try: {sug}");
    }
    if let Some(u) = err.docs_url() {
        let _ = writeln!(s, "  see: {u}");
    }
    s
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unnecessary_literal_bound
)]
mod tests {
    use super::*;
    use std::fmt;

    #[derive(Debug)]
    struct FullError;
    impl fmt::Display for FullError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("flash failed")
        }
    }
    impl std::error::Error for FullError {}
    impl UserFacing for FullError {
        fn summary(&self) -> &str {
            "signature verification failed"
        }
        fn detail(&self) -> &str {
            "shim's signature couldn't be verified against the signing key currently enrolled."
        }
        fn suggestion(&self) -> Option<&str> {
            Some("re-run `aegis-boot flash` to re-enroll the signing key")
        }
        fn docs_url(&self) -> Option<&str> {
            Some("https://aegis-boot.dev/docs/errors/sig-verify-failed")
        }
        fn code(&self) -> Option<&str> {
            Some("SIG_VERIFY_FAILED")
        }
    }

    #[derive(Debug)]
    struct MinimalError;
    impl fmt::Display for MinimalError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("minimal")
        }
    }
    impl std::error::Error for MinimalError {}
    impl UserFacing for MinimalError {
        fn summary(&self) -> &str {
            "device busy"
        }
        fn detail(&self) -> &str {
            "the target device is currently mounted by another process."
        }
    }

    #[test]
    fn render_string_emits_code_when_present() {
        let s = render_string(&FullError);
        assert!(s.starts_with("error[SIG_VERIFY_FAILED]:"), "got: {s}");
    }

    #[test]
    fn render_string_omits_code_bracket_when_absent() {
        let s = render_string(&MinimalError);
        assert!(s.starts_with("error: device busy"), "got: {s}");
        assert!(!s.contains('['), "expected no code bracket: {s}");
    }

    #[test]
    fn render_string_emits_what_happened_line() {
        let s = render_string(&FullError);
        assert!(s.contains("what happened: shim"), "got: {s}");
    }

    #[test]
    fn render_string_emits_try_when_suggestion_present() {
        let s = render_string(&FullError);
        assert!(s.contains("try: re-run"), "got: {s}");
    }

    #[test]
    fn render_string_omits_try_when_suggestion_absent() {
        let s = render_string(&MinimalError);
        assert!(!s.contains("try:"), "expected no try line: {s}");
    }

    #[test]
    fn render_string_emits_see_when_docs_url_present() {
        let s = render_string(&FullError);
        assert!(s.contains("see: https://"), "got: {s}");
    }

    #[test]
    fn render_string_omits_see_when_docs_url_absent() {
        let s = render_string(&MinimalError);
        assert!(!s.contains("see:"), "expected no see line: {s}");
    }

    /// Implementing `Display` via `render` matches `render_string`.
    /// Keeps the renderer paths in sync.
    #[test]
    fn display_via_render_matches_render_string() {
        struct DisplayWrapper<'a>(&'a dyn UserFacing);
        impl fmt::Display for DisplayWrapper<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                render(self.0, f)
            }
        }
        let display_str = DisplayWrapper(&FullError).to_string();
        let direct_str = render_string(&FullError);
        assert_eq!(display_str, direct_str);
    }
}
