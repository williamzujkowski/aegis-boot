// SPDX-License-Identifier: MIT OR Apache-2.0

//! Trust-tier verdict for a single row in the rescue-tui ISO list.
//!
//! Six tiers, each with a color, glyph, and descriptive message. Drive
//! both display (list-pane badge, info-pane header) and boot gating
//! (`is_bootable` — tier 4/5/6 refuse Enter). Canonical source for the
//! tier table surfaced to operator docs via #462 (`tiers-docgen`).
//!
//! ## Trust-tier contract
//!
//! See [`docs/design/rescue-tui-ux-overhaul.md`] for the full model.
//! Summary:
//!
//! | Tier | Variant                 | Bootable | Boot-time friction            |
//! | ---- | ----------------------- | -------- | ------------------------------ |
//! | 1    | [`OperatorAttested`]    | yes      | Enter alone                   |
//! | 2    | [`BareUnverified`]      | yes      | typed-confirmation challenge  |
//! | 3    | [`KeyNotTrusted`]       | yes      | typed-confirmation challenge  |
//! | 4    | [`ParseFailed`]         | **no**   | Enter refused; reason shown   |
//! | 5    | [`SecureBootBlocked`]   | **no**   | Enter refused; reason shown   |
//! | 6    | [`HashMismatch`]        | **no**   | Enter refused; reason shown   |
//!
//! ## Source-of-truth pairing
//!
//! Tier 4 is built from [`iso_probe::FailedIso`] (the `DiscoveryReport::failed`
//! list). Tiers 1/2/3/5/6 are built from [`iso_probe::DiscoveredIso`] +
//! ambient [`SecureBootStatus`]. Tier 5 keys off the `NotKexecBootable`
//! quirk and off `UnsignedKernel` + Secure Boot Enforcing (since the
//! kernel would be rejected by `kexec_file_load` at boot time).
//!
//! [`OperatorAttested`]: TrustVerdict::OperatorAttested
//! [`BareUnverified`]: TrustVerdict::BareUnverified
//! [`KeyNotTrusted`]: TrustVerdict::KeyNotTrusted
//! [`ParseFailed`]: TrustVerdict::ParseFailed
//! [`SecureBootBlocked`]: TrustVerdict::SecureBootBlocked
//! [`HashMismatch`]: TrustVerdict::HashMismatch
//! [`docs/design/rescue-tui-ux-overhaul.md`]: ../../../../docs/design/rescue-tui-ux-overhaul.md
//!
use iso_probe::{DiscoveredIso, FailedIso, HashVerification, Quirk, SignatureVerification};
use ratatui::style::Color;

use crate::state::SecureBootStatus;
use crate::theme::Theme;

/// Trust-tier verdict for a single list row.
///
/// Six tiers spanning three bootable (1/2/3) and three blocked (4/5/6).
/// See the module docs for the full contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TrustVerdict {
    /// Tier 1 — hash OR signature verified against a trusted source.
    /// Green, boots on Enter with no friction.
    OperatorAttested,
    /// Tier 2 — no sibling `.sha256` / `.minisig`. Parseable + bootable
    /// but the operator hasn't attested the bytes. Gray, typed-
    /// confirmation required before boot.
    BareUnverified,
    /// Tier 3 — signature parsed but the signer isn't in
    /// `AEGIS_TRUSTED_KEYS`. Yellow, typed-confirmation required.
    KeyNotTrusted,
    /// Tier 4 — iso-parser couldn't extract kernel/initrd from this
    /// `.iso`. Reason carries the sanitized iso-parser error. Red,
    /// boot refused.
    ParseFailed {
        /// TUI-safe, pre-sanitized explanation from iso-parser.
        reason: String,
    },
    /// Tier 5 — the kernel would be rejected by the platform keyring.
    /// Either a Windows/non-Linux boot protocol (`NotKexecBootable`
    /// quirk) or an `UnsignedKernel` distro running under Secure Boot
    /// enforcement. Red, boot refused.
    SecureBootBlocked {
        /// TUI-safe explanation naming the specific block reason.
        reason: String,
    },
    /// Tier 6 — the ISO bytes don't match either the declared sidecar
    /// hash or the minisign signature. Strong tamper signal. Red,
    /// boot refused.
    HashMismatch {
        /// Hex digest the sidecar declared.
        expected: String,
        /// Hex digest the actual ISO bytes produced.
        actual: String,
        /// Source of the expected hash (sidecar path or "minisign").
        source: String,
    },
}

impl TrustVerdict {
    /// One-line, single-word label suitable for a list-row badge or
    /// monochrome render. ASCII-only so it survives serial/braille
    /// output paths.
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::OperatorAttested => "VERIFIED",
            Self::BareUnverified => "UNVERIFIED",
            Self::KeyNotTrusted => "UNTRUSTED KEY",
            Self::ParseFailed { .. } => "PARSE FAILED",
            Self::SecureBootBlocked { .. } => "BOOT BLOCKED",
            Self::HashMismatch { .. } => "HASH MISMATCH",
        }
    }

    /// Long-form reason suitable for an info-pane body. For variants
    /// carrying data, this includes the payload string so operators
    /// see *why* the verdict is what it is.
    pub(crate) fn reason(&self) -> String {
        match self {
            Self::OperatorAttested => {
                "hash or signature verified against a trusted source".to_string()
            }
            Self::BareUnverified => "no sibling .sha256 or .minisig found".to_string(),
            Self::KeyNotTrusted => {
                "signature parses but key is not in AEGIS_TRUSTED_KEYS".to_string()
            }
            Self::ParseFailed { reason } | Self::SecureBootBlocked { reason } => reason.clone(),
            Self::HashMismatch {
                expected,
                actual,
                source,
            } => {
                format!(
                    "sidecar declares {expected}, ISO bytes hash to {actual} (source: {source})"
                )
            }
        }
    }

    /// Color from the active theme. 16-color-safe; never depends on
    /// truecolor being available.
    pub(crate) fn color(&self, theme: &Theme) -> Color {
        match self {
            Self::OperatorAttested => theme.success,
            Self::BareUnverified => Color::Gray,
            Self::KeyNotTrusted => theme.warning,
            Self::ParseFailed { .. }
            | Self::SecureBootBlocked { .. }
            | Self::HashMismatch { .. } => theme.error,
        }
    }

    /// Single-character status glyph for a list row. Visible in
    /// monochrome themes (no color reliance) — matches the glyph
    /// convention used before #457.
    pub(crate) fn glyph(&self) -> &'static str {
        match self {
            Self::OperatorAttested => "[+]",
            Self::KeyNotTrusted => "[~]",
            Self::BareUnverified => "[ ]",
            Self::ParseFailed { .. } | Self::HashMismatch { .. } => "[!]",
            Self::SecureBootBlocked { .. } => "[X]",
        }
    }

    /// Whether a row with this verdict should be bootable. Tiers 1/2/3
    /// are bootable (with the existing typed-confirmation gate
    /// handling 2/3); tiers 4/5/6 are not bootable because the
    /// failure mode precludes a successful kexec.
    pub(crate) fn is_bootable(&self) -> bool {
        matches!(
            self,
            Self::OperatorAttested | Self::BareUnverified | Self::KeyNotTrusted
        )
    }

    /// Derive a verdict for a successfully-parsed [`DiscoveredIso`].
    /// The `secure_boot` argument is consulted for the tier-5
    /// `UnsignedKernel` case — on `Disabled` or `Unknown` systems the
    /// quirk surfaces only as a warning rather than a hard block.
    pub(crate) fn from_discovered(iso: &DiscoveredIso, secure_boot: SecureBootStatus) -> Self {
        // Tier 6 first — tamper signals trump everything else.
        if let HashVerification::Mismatch {
            expected,
            actual,
            source,
        } = &iso.hash_verification
        {
            return Self::HashMismatch {
                expected: short_hex(expected),
                actual: short_hex(actual),
                source: source.clone(),
            };
        }
        if let SignatureVerification::Forged { sig_path } = &iso.signature_verification {
            return Self::HashMismatch {
                expected: "(minisign signer's recorded digest)".to_string(),
                actual: "(mismatched digest of ISO bytes)".to_string(),
                source: sig_path.display().to_string(),
            };
        }

        // Tier 5 — Secure Boot gates. Windows/non-Linux boot protocol
        // is always tier 5 (kexec can't load it regardless of SB).
        // UnsignedKernel is tier 5 only under SB enforcement; otherwise
        // the ISO may still kexec successfully.
        if iso.quirks.contains(&Quirk::NotKexecBootable) {
            return Self::SecureBootBlocked {
                reason: format!(
                    "{} uses a boot protocol incompatible with kexec_file_load \
                     (Windows NT loader or equivalent); kexec would refuse",
                    iso.distribution.label()
                ),
            };
        }
        if iso.quirks.contains(&Quirk::UnsignedKernel)
            && matches!(secure_boot, SecureBootStatus::Enforcing)
        {
            return Self::SecureBootBlocked {
                reason: format!(
                    "{} ships an unsigned kernel; platform keyring under Secure Boot \
                     enforcement will reject kexec_file_load",
                    iso.distribution.label()
                ),
            };
        }

        // Tier 1 — hash or signature verified.
        if matches!(
            iso.signature_verification,
            SignatureVerification::Verified { .. }
        ) || matches!(iso.hash_verification, HashVerification::Verified { .. })
        {
            return Self::OperatorAttested;
        }

        // Tier 3 — signature parses but key untrusted.
        if matches!(
            iso.signature_verification,
            SignatureVerification::KeyNotTrusted { .. }
        ) {
            return Self::KeyNotTrusted;
        }

        // Tier 2 — no verification material.
        Self::BareUnverified
    }

    /// Derive a verdict for an ISO that failed to parse. Always
    /// produces [`TrustVerdict::ParseFailed`] with the failure reason.
    pub(crate) fn from_failed(failed: &FailedIso) -> Self {
        Self::ParseFailed {
            reason: failed.reason.clone(),
        }
    }

    /// Origin-trust axis (#558). "Did this ISO come from a trustworthy
    /// place?" Computed from the ISO's signature + hash sidecar signals.
    /// Independent of [`Self::media_verdict`]; the combined
    /// [`Self::from_discovered`] continues to be the boot-gating verdict.
    pub(crate) fn source_verdict(iso: &DiscoveredIso) -> SourceVerdict {
        SourceVerdict::from_iso(iso)
    }

    /// Storage-integrity axis (#558). "Do the on-stick ISO bytes match
    /// the most-recent recorded hash (sidecar or verify-now)?" The
    /// verify-now flow (#548) is the operator-driven path that turns
    /// `Unverified` into `Verified` (or surfaces `Mismatch` as a tamper
    /// signal).
    pub(crate) fn media_verdict(iso: &DiscoveredIso) -> MediaVerdict {
        MediaVerdict::from_iso(iso)
    }
}

/// Origin-trust axis. Captures the question "is the ISO's claimed
/// content vouched for by something we recognize?" Independent of
/// whether the on-stick bytes match — that's [`MediaVerdict`].
///
/// Mapping from `iso-probe` signals (see [`SourceVerdict::from_iso`]):
///
/// | iso-probe state                                                  | SourceVerdict     |
/// | ---------------------------------------------------------------- | ----------------- |
/// | `SignatureVerification::Verified` (key in `AEGIS_TRUSTED_KEYS`)  | `Verified`        |
/// | `HashVerification::Verified` (sidecar matches; no signature)     | `Verified`        |
/// | `SignatureVerification::Forged`                                  | `Tampered`        |
/// | `SignatureVerification::KeyNotTrusted`                           | `KeyNotTrusted`   |
/// | nothing else passes                                              | `Bare`            |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceVerdict {
    /// A trusted attestation (signature or sidecar) vouches for the
    /// claimed ISO content.
    Verified,
    /// A signature parses but the signing key is not in
    /// `AEGIS_TRUSTED_KEYS`. The bytes may be intact, but no party we
    /// recognize attests them.
    KeyNotTrusted,
    /// A signature parses against a recognized key BUT the signed
    /// digest does not match what's on disk. Strong tamper signal at
    /// the source layer (the publisher's claim disagrees with reality).
    Tampered,
    /// No verification material at all (no `.sha256`, no `.minisig`).
    Bare,
}

impl SourceVerdict {
    /// Compute from [`DiscoveredIso`] signals. Order of precedence
    /// matters: a forged signature wins over a signature-verified
    /// branch (defense in depth — the more-suspicious result surfaces).
    pub(crate) fn from_iso(iso: &DiscoveredIso) -> Self {
        use HashVerification as H;
        use SignatureVerification as S;
        // Tamper signals first.
        if matches!(iso.signature_verification, S::Forged { .. }) {
            return Self::Tampered;
        }
        // Trust signals next.
        if matches!(iso.signature_verification, S::Verified { .. })
            || matches!(iso.hash_verification, H::Verified { .. })
        {
            return Self::Verified;
        }
        // Untrusted-but-parseable signature.
        if matches!(iso.signature_verification, S::KeyNotTrusted { .. }) {
            return Self::KeyNotTrusted;
        }
        // No verification material present (or hash mismatch with no
        // sig — the bytes-vs-recorded divergence is reported on the
        // Media axis, not Source).
        Self::Bare
    }

    /// One-word ASCII label for monochrome / serial / braille displays.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Verified => "VERIFIED",
            Self::KeyNotTrusted => "UNTRUSTED KEY",
            Self::Tampered => "TAMPERED",
            Self::Bare => "UNVERIFIED",
        }
    }

    /// Color from the active theme. 16-color-safe.
    pub(crate) fn color(self, theme: &Theme) -> Color {
        match self {
            Self::Verified => theme.success,
            Self::KeyNotTrusted => theme.warning,
            Self::Tampered => theme.error,
            Self::Bare => Color::Gray,
        }
    }
}

/// Storage-integrity axis. Captures the question "do the on-stick ISO
/// bytes match the most-recent recorded hash?"
///
/// "Recorded hash" can come from two places:
/// 1. The sibling `.sha256` / minisig sidecar checked at discovery time.
/// 2. The verify-now (#548) operator flow, which recomputes the hash
///    at boot time and updates [`DiscoveredIso::hash_verification`] in
///    place.
///
/// Mapping from `iso-probe` signals (see [`MediaVerdict::from_iso`]):
///
/// | iso-probe state                                | `MediaVerdict`   |
/// | ---------------------------------------------- | --------------- |
/// | `SignatureVerification::Verified`              | `Verified`      |
/// | `SignatureVerification::Forged`                | `Mismatch`      |
/// | `HashVerification::Verified`                   | `Verified`      |
/// | `HashVerification::Mismatch`                   | `Mismatch`      |
/// | otherwise                                      | `Unverified`    |
///
/// `Verified` requires either a passing signature (which transitively
/// vouches for the bytes) or a matching sidecar sha. `Mismatch` is the
/// tamper-detection signal at the storage layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MediaVerdict {
    /// On-stick bytes match the recorded hash (sidecar or verify-now).
    Verified,
    /// On-stick bytes differ from the recorded hash. Strong tamper
    /// signal at the storage layer.
    Mismatch,
    /// No recorded hash to compare against, or the bytes haven't been
    /// re-hashed since discovery. The verify-now flow (#548) is the
    /// operator-driven path that resolves this state.
    Unverified,
}

impl MediaVerdict {
    pub(crate) fn from_iso(iso: &DiscoveredIso) -> Self {
        use HashVerification as H;
        use SignatureVerification as S;
        // Signature checks are computed against bytes — a passing
        // signature implies the bytes match the signer's claim, which
        // is exactly the Media-axis property.
        if matches!(iso.signature_verification, S::Verified { .. }) {
            return Self::Verified;
        }
        if matches!(iso.signature_verification, S::Forged { .. }) {
            return Self::Mismatch;
        }
        match &iso.hash_verification {
            H::Verified { .. } => Self::Verified,
            H::Mismatch { .. } => Self::Mismatch,
            _ => Self::Unverified,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Verified => "VERIFIED",
            Self::Mismatch => "MISMATCH",
            Self::Unverified => "UNVERIFIED",
        }
    }

    pub(crate) fn color(self, theme: &Theme) -> Color {
        match self {
            Self::Verified => theme.success,
            Self::Mismatch => theme.error,
            Self::Unverified => Color::Gray,
        }
    }
}

/// Truncate a long hex digest to the first 12 chars with an ellipsis —
/// keeps the `HashMismatch` reason line readable in narrow terminals.
fn short_hex(hex: &str) -> String {
    if hex.len() <= 14 {
        return hex.to_string();
    }
    format!("{}…", &hex[..12])
}

/// Extension trait on [`iso_parser::Distribution`] giving a short,
/// capitalized display label for reason strings.
trait DistroLabel {
    fn label(&self) -> &'static str;
}

impl DistroLabel for iso_probe::Distribution {
    fn label(&self) -> &'static str {
        use iso_probe::Distribution as D;
        match self {
            D::Arch => "Arch Linux",
            D::Debian => "Debian-family",
            D::Fedora => "Fedora",
            D::RedHat => "RHEL-family",
            D::Alpine => "Alpine Linux",
            D::NixOS => "NixOS",
            D::Windows => "Windows",
            D::Unknown => "This ISO",
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use iso_probe::{Distribution, HashVerification, Quirk, SignatureVerification};
    use std::path::PathBuf;

    fn iso_with(
        hash: HashVerification,
        sig: SignatureVerification,
        quirks: Vec<Quirk>,
        distro: Distribution,
    ) -> DiscoveredIso {
        DiscoveredIso {
            iso_path: PathBuf::from("/isos/test.iso"),
            label: "test".to_string(),
            pretty_name: None,
            distribution: distro,
            kernel: PathBuf::from("boot/vmlinuz"),
            initrd: None,
            cmdline: None,
            quirks,
            hash_verification: hash,
            signature_verification: sig,
            size_bytes: Some(100),
            contains_installer: false,
            sidecar: None,
        }
    }

    #[test]
    fn every_variant_has_non_empty_label_reason_glyph() {
        let variants = [
            TrustVerdict::OperatorAttested,
            TrustVerdict::BareUnverified,
            TrustVerdict::KeyNotTrusted,
            TrustVerdict::ParseFailed {
                reason: "x".to_string(),
            },
            TrustVerdict::SecureBootBlocked {
                reason: "x".to_string(),
            },
            TrustVerdict::HashMismatch {
                expected: "a".to_string(),
                actual: "b".to_string(),
                source: "c".to_string(),
            },
        ];
        for v in variants {
            assert!(!v.label().is_empty(), "empty label for {v:?}");
            assert!(!v.reason().is_empty(), "empty reason for {v:?}");
            assert!(!v.glyph().is_empty(), "empty glyph for {v:?}");
        }
    }

    #[test]
    fn is_bootable_true_for_tier_1_2_3() {
        assert!(TrustVerdict::OperatorAttested.is_bootable());
        assert!(TrustVerdict::BareUnverified.is_bootable());
        assert!(TrustVerdict::KeyNotTrusted.is_bootable());
    }

    #[test]
    fn is_bootable_false_for_tier_4_5_6() {
        assert!(!TrustVerdict::ParseFailed { reason: "x".into() }.is_bootable());
        assert!(!TrustVerdict::SecureBootBlocked { reason: "x".into() }.is_bootable());
        assert!(
            !TrustVerdict::HashMismatch {
                expected: "a".into(),
                actual: "b".into(),
                source: "c".into()
            }
            .is_bootable()
        );
    }

    #[test]
    fn from_discovered_green_when_signature_verified() {
        let iso = iso_with(
            HashVerification::NotPresent,
            SignatureVerification::Verified {
                key_id: "abc".to_string(),
                sig_path: PathBuf::from("/isos/test.iso.minisig"),
            },
            vec![],
            Distribution::Debian,
        );
        let v = TrustVerdict::from_discovered(&iso, SecureBootStatus::Enforcing);
        assert!(matches!(v, TrustVerdict::OperatorAttested));
    }

    #[test]
    fn from_discovered_green_when_hash_verified() {
        let iso = iso_with(
            HashVerification::Verified {
                digest: "hashhash".to_string(),
                source: "/isos/test.iso.sha256".to_string(),
            },
            SignatureVerification::NotPresent,
            vec![],
            Distribution::Arch,
        );
        let v = TrustVerdict::from_discovered(&iso, SecureBootStatus::Disabled);
        assert!(matches!(v, TrustVerdict::OperatorAttested));
    }

    #[test]
    fn from_discovered_yellow_when_key_not_trusted() {
        let iso = iso_with(
            HashVerification::NotPresent,
            SignatureVerification::KeyNotTrusted {
                key_id: "untrusted".to_string(),
            },
            vec![],
            Distribution::Debian,
        );
        let v = TrustVerdict::from_discovered(&iso, SecureBootStatus::Enforcing);
        assert!(matches!(v, TrustVerdict::KeyNotTrusted));
    }

    #[test]
    fn from_discovered_gray_when_no_material() {
        let iso = iso_with(
            HashVerification::NotPresent,
            SignatureVerification::NotPresent,
            vec![],
            Distribution::Debian,
        );
        let v = TrustVerdict::from_discovered(&iso, SecureBootStatus::Enforcing);
        assert!(matches!(v, TrustVerdict::BareUnverified));
    }

    #[test]
    fn from_discovered_tier6_on_hash_mismatch() {
        let iso = iso_with(
            HashVerification::Mismatch {
                expected: "a".repeat(64),
                actual: "b".repeat(64),
                source: "/isos/test.iso.sha256".to_string(),
            },
            SignatureVerification::NotPresent,
            vec![],
            Distribution::Debian,
        );
        let v = TrustVerdict::from_discovered(&iso, SecureBootStatus::Enforcing);
        match v {
            TrustVerdict::HashMismatch {
                expected,
                actual,
                source,
            } => {
                // Long hashes get truncated with an ellipsis for display.
                assert!(expected.ends_with('…'));
                assert!(actual.ends_with('…'));
                assert!(source.contains("test.iso.sha256"));
            }
            other => panic!("expected HashMismatch, got {other:?}"),
        }
    }

    #[test]
    fn from_discovered_tier6_on_forged_signature() {
        let iso = iso_with(
            HashVerification::NotPresent,
            SignatureVerification::Forged {
                sig_path: PathBuf::from("/isos/test.iso.minisig"),
            },
            vec![],
            Distribution::Debian,
        );
        let v = TrustVerdict::from_discovered(&iso, SecureBootStatus::Enforcing);
        assert!(matches!(v, TrustVerdict::HashMismatch { .. }));
    }

    #[test]
    fn from_discovered_tier5_windows_always_blocked() {
        // Windows is NotKexecBootable regardless of Secure Boot state —
        // it's a different boot protocol entirely.
        let iso = iso_with(
            HashVerification::NotPresent,
            SignatureVerification::NotPresent,
            vec![Quirk::NotKexecBootable],
            Distribution::Windows,
        );
        for sb in [
            SecureBootStatus::Enforcing,
            SecureBootStatus::Disabled,
            SecureBootStatus::Unknown,
        ] {
            let v = TrustVerdict::from_discovered(&iso, sb);
            assert!(
                matches!(v, TrustVerdict::SecureBootBlocked { .. }),
                "Windows must be blocked under {sb:?}, got {v:?}",
            );
        }
    }

    #[test]
    fn from_discovered_tier5_unsigned_kernel_only_under_sb_enforcing() {
        // Unsigned kernel on an Arch ISO: Secure Boot Enforcing blocks
        // (keyring rejects kexec_file_load); Disabled / Unknown still
        // allow boot at lower tier (BareUnverified or KeyNotTrusted
        // based on sig state).
        let iso = iso_with(
            HashVerification::NotPresent,
            SignatureVerification::NotPresent,
            vec![Quirk::UnsignedKernel],
            Distribution::Arch,
        );
        let v_enforcing = TrustVerdict::from_discovered(&iso, SecureBootStatus::Enforcing);
        assert!(matches!(
            v_enforcing,
            TrustVerdict::SecureBootBlocked { .. }
        ));
        let v_disabled = TrustVerdict::from_discovered(&iso, SecureBootStatus::Disabled);
        assert!(matches!(v_disabled, TrustVerdict::BareUnverified));
        let v_unknown = TrustVerdict::from_discovered(&iso, SecureBootStatus::Unknown);
        assert!(matches!(v_unknown, TrustVerdict::BareUnverified));
    }

    #[test]
    fn from_discovered_tier6_takes_precedence_over_tier5() {
        // Hash mismatch must win over NotKexecBootable — the tamper
        // signal is stronger information than the kexec protocol gap.
        let iso = iso_with(
            HashVerification::Mismatch {
                expected: "a".repeat(64),
                actual: "b".repeat(64),
                source: "/isos/sha256sums".to_string(),
            },
            SignatureVerification::NotPresent,
            vec![Quirk::NotKexecBootable],
            Distribution::Windows,
        );
        let v = TrustVerdict::from_discovered(&iso, SecureBootStatus::Enforcing);
        assert!(matches!(v, TrustVerdict::HashMismatch { .. }));
    }

    // ---- #558 Source/Media verdict split (additive) -------------------

    /// #558 migration property: every ISO fixture produces the SAME
    /// 6-tier `TrustVerdict::from_discovered()` result before and after
    /// the split. The new accessors are additive — they don't change
    /// boot-gating or the canonical tier label.
    ///
    /// Pinned by exhaustive matrix below. If a future refactor breaks
    /// this, every consumer of the combined verdict is also broken;
    /// the panic message names the offending fixture so triage is one
    /// step instead of three.
    #[test]
    fn from_discovered_combined_unchanged_after_split() {
        let cases: &[(_, _, Vec<_>, _, SecureBootStatus, &str)] = &[
            (
                HashVerification::NotPresent,
                SignatureVerification::Verified {
                    key_id: "x".to_string(),
                    sig_path: PathBuf::from("/x.minisig"),
                },
                vec![],
                Distribution::Debian,
                SecureBootStatus::Enforcing,
                "OperatorAttested",
            ),
            (
                HashVerification::Verified {
                    digest: "h".to_string(),
                    source: "/x.sha256".to_string(),
                },
                SignatureVerification::NotPresent,
                vec![],
                Distribution::Arch,
                SecureBootStatus::Disabled,
                "OperatorAttested",
            ),
            (
                HashVerification::NotPresent,
                SignatureVerification::KeyNotTrusted {
                    key_id: "x".to_string(),
                },
                vec![],
                Distribution::Debian,
                SecureBootStatus::Enforcing,
                "KeyNotTrusted",
            ),
            (
                HashVerification::NotPresent,
                SignatureVerification::NotPresent,
                vec![],
                Distribution::Debian,
                SecureBootStatus::Enforcing,
                "BareUnverified",
            ),
            (
                HashVerification::Mismatch {
                    expected: "a".repeat(64),
                    actual: "b".repeat(64),
                    source: "/x.sha256".to_string(),
                },
                SignatureVerification::NotPresent,
                vec![],
                Distribution::Debian,
                SecureBootStatus::Enforcing,
                "HashMismatch",
            ),
            (
                HashVerification::NotPresent,
                SignatureVerification::Forged {
                    sig_path: PathBuf::from("/x.minisig"),
                },
                vec![],
                Distribution::Debian,
                SecureBootStatus::Enforcing,
                "HashMismatch",
            ),
            (
                HashVerification::NotPresent,
                SignatureVerification::NotPresent,
                vec![Quirk::NotKexecBootable],
                Distribution::Windows,
                SecureBootStatus::Enforcing,
                "SecureBootBlocked",
            ),
        ];
        for (hash, sig, quirks, distro, sb, expected_tier) in cases {
            let iso = iso_with(hash.clone(), sig.clone(), quirks.clone(), *distro);
            let v = TrustVerdict::from_discovered(&iso, *sb);
            let actual_tier = match v {
                TrustVerdict::OperatorAttested => "OperatorAttested",
                TrustVerdict::BareUnverified => "BareUnverified",
                TrustVerdict::KeyNotTrusted => "KeyNotTrusted",
                TrustVerdict::ParseFailed { .. } => "ParseFailed",
                TrustVerdict::SecureBootBlocked { .. } => "SecureBootBlocked",
                TrustVerdict::HashMismatch { .. } => "HashMismatch",
            };
            assert_eq!(
                actual_tier, *expected_tier,
                "tier drift on fixture: hash={hash:?} sig={sig:?} quirks={quirks:?} sb={sb:?}"
            );
        }
    }

    #[test]
    fn source_verdict_verified_on_signature_or_hash() {
        let iso_sig = iso_with(
            HashVerification::NotPresent,
            SignatureVerification::Verified {
                key_id: "k".to_string(),
                sig_path: PathBuf::from("/x.minisig"),
            },
            vec![],
            Distribution::Debian,
        );
        assert_eq!(
            TrustVerdict::source_verdict(&iso_sig),
            SourceVerdict::Verified
        );
        let iso_hash = iso_with(
            HashVerification::Verified {
                digest: "h".to_string(),
                source: "/x.sha256".to_string(),
            },
            SignatureVerification::NotPresent,
            vec![],
            Distribution::Arch,
        );
        assert_eq!(
            TrustVerdict::source_verdict(&iso_hash),
            SourceVerdict::Verified
        );
    }

    #[test]
    fn source_verdict_tampered_when_signature_forged() {
        let iso = iso_with(
            HashVerification::NotPresent,
            SignatureVerification::Forged {
                sig_path: PathBuf::from("/x.minisig"),
            },
            vec![],
            Distribution::Debian,
        );
        assert_eq!(
            TrustVerdict::source_verdict(&iso),
            SourceVerdict::Tampered,
            "forged sig is a tamper signal at the source layer"
        );
    }

    #[test]
    fn source_verdict_key_not_trusted_propagates() {
        let iso = iso_with(
            HashVerification::NotPresent,
            SignatureVerification::KeyNotTrusted {
                key_id: "k".to_string(),
            },
            vec![],
            Distribution::Debian,
        );
        assert_eq!(
            TrustVerdict::source_verdict(&iso),
            SourceVerdict::KeyNotTrusted
        );
    }

    #[test]
    fn source_verdict_bare_when_no_material() {
        let iso = iso_with(
            HashVerification::NotPresent,
            SignatureVerification::NotPresent,
            vec![],
            Distribution::Debian,
        );
        assert_eq!(TrustVerdict::source_verdict(&iso), SourceVerdict::Bare);
    }

    #[test]
    fn source_verdict_bare_when_hash_mismatch_alone() {
        // Hash mismatch with no signature: the bytes-vs-recorded
        // divergence is a Media-axis problem, NOT a Source-axis one.
        // The sidecar isn't itself authenticated; we don't know whether
        // the recorded value or the actual bytes are the canonical one.
        let iso = iso_with(
            HashVerification::Mismatch {
                expected: "a".repeat(64),
                actual: "b".repeat(64),
                source: "/x.sha256".to_string(),
            },
            SignatureVerification::NotPresent,
            vec![],
            Distribution::Debian,
        );
        assert_eq!(TrustVerdict::source_verdict(&iso), SourceVerdict::Bare);
    }

    #[test]
    fn media_verdict_verified_on_signature_or_hash() {
        let iso_sig = iso_with(
            HashVerification::NotPresent,
            SignatureVerification::Verified {
                key_id: "k".to_string(),
                sig_path: PathBuf::from("/x.minisig"),
            },
            vec![],
            Distribution::Debian,
        );
        assert_eq!(
            TrustVerdict::media_verdict(&iso_sig),
            MediaVerdict::Verified,
            "passing sig transitively vouches for bytes"
        );
        let iso_hash = iso_with(
            HashVerification::Verified {
                digest: "h".to_string(),
                source: "/x.sha256".to_string(),
            },
            SignatureVerification::NotPresent,
            vec![],
            Distribution::Arch,
        );
        assert_eq!(
            TrustVerdict::media_verdict(&iso_hash),
            MediaVerdict::Verified
        );
    }

    #[test]
    fn media_verdict_mismatch_on_forged_sig_or_hash_mismatch() {
        let iso_forged = iso_with(
            HashVerification::NotPresent,
            SignatureVerification::Forged {
                sig_path: PathBuf::from("/x.minisig"),
            },
            vec![],
            Distribution::Debian,
        );
        assert_eq!(
            TrustVerdict::media_verdict(&iso_forged),
            MediaVerdict::Mismatch,
            "forged sig means bytes diverged from signer's claim"
        );
        let iso_mismatch = iso_with(
            HashVerification::Mismatch {
                expected: "a".repeat(64),
                actual: "b".repeat(64),
                source: "/x.sha256".to_string(),
            },
            SignatureVerification::NotPresent,
            vec![],
            Distribution::Debian,
        );
        assert_eq!(
            TrustVerdict::media_verdict(&iso_mismatch),
            MediaVerdict::Mismatch
        );
    }

    #[test]
    fn media_verdict_unverified_when_no_recorded_hash() {
        // No sidecar, no sig, nothing to compare bytes against.
        let iso = iso_with(
            HashVerification::NotPresent,
            SignatureVerification::NotPresent,
            vec![],
            Distribution::Debian,
        );
        assert_eq!(TrustVerdict::media_verdict(&iso), MediaVerdict::Unverified);
    }

    /// Orthogonality demo: green source + red media = "publisher signed
    /// these bytes but on-stick bytes diverge" (= forged sig). Distinct
    /// from the inverse combinations the existing tier model collapses.
    #[test]
    fn source_and_media_axes_are_orthogonal() {
        // Tampered + Mismatch — sig says X but bytes hash to Y.
        let forged = iso_with(
            HashVerification::NotPresent,
            SignatureVerification::Forged {
                sig_path: PathBuf::from("/x.minisig"),
            },
            vec![],
            Distribution::Debian,
        );
        assert_eq!(
            TrustVerdict::source_verdict(&forged),
            SourceVerdict::Tampered
        );
        assert_eq!(TrustVerdict::media_verdict(&forged), MediaVerdict::Mismatch);

        // Bare + Mismatch — sidecar disagrees with bytes; no sig to authenticate.
        let bare_mismatch = iso_with(
            HashVerification::Mismatch {
                expected: "a".repeat(64),
                actual: "b".repeat(64),
                source: "/x.sha256".to_string(),
            },
            SignatureVerification::NotPresent,
            vec![],
            Distribution::Debian,
        );
        assert_eq!(
            TrustVerdict::source_verdict(&bare_mismatch),
            SourceVerdict::Bare
        );
        assert_eq!(
            TrustVerdict::media_verdict(&bare_mismatch),
            MediaVerdict::Mismatch
        );

        // Bare + Unverified — no sidecar, no sig, no claim either way.
        let pure_bare = iso_with(
            HashVerification::NotPresent,
            SignatureVerification::NotPresent,
            vec![],
            Distribution::Debian,
        );
        assert_eq!(
            TrustVerdict::source_verdict(&pure_bare),
            SourceVerdict::Bare
        );
        assert_eq!(
            TrustVerdict::media_verdict(&pure_bare),
            MediaVerdict::Unverified
        );
    }

    #[test]
    fn source_and_media_labels_non_empty_for_every_variant() {
        for s in [
            SourceVerdict::Verified,
            SourceVerdict::KeyNotTrusted,
            SourceVerdict::Tampered,
            SourceVerdict::Bare,
        ] {
            assert!(!s.label().is_empty());
        }
        for m in [
            MediaVerdict::Verified,
            MediaVerdict::Mismatch,
            MediaVerdict::Unverified,
        ] {
            assert!(!m.label().is_empty());
        }
    }

    #[test]
    fn from_failed_always_produces_parse_failed_with_reason() {
        let failed = FailedIso {
            iso_path: PathBuf::from("/isos/broken.iso"),
            reason: "mount: wrong fs type".to_string(),
            kind: iso_probe::FailureKind::MountFailed,
        };
        let v = TrustVerdict::from_failed(&failed);
        match v {
            TrustVerdict::ParseFailed { reason } => {
                assert_eq!(reason, "mount: wrong fs type");
            }
            other => panic!("expected ParseFailed, got {other:?}"),
        }
    }

    #[test]
    fn short_hex_truncates_long_digests_with_ellipsis() {
        let full = "a".repeat(64);
        let out = short_hex(&full);
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().filter(|c| *c == 'a').count(), 12);
    }

    #[test]
    fn short_hex_passes_short_digests_verbatim() {
        assert_eq!(short_hex("deadbeef"), "deadbeef");
        // Boundary: exactly 14 chars stays as-is.
        assert_eq!(short_hex("0123456789abcd"), "0123456789abcd");
    }

    #[test]
    fn distribution_label_populated_for_every_variant() {
        use iso_probe::Distribution as D;
        for d in [
            D::Arch,
            D::Debian,
            D::Fedora,
            D::RedHat,
            D::Alpine,
            D::NixOS,
            D::Windows,
            D::Unknown,
        ] {
            assert!(!d.label().is_empty());
        }
    }
}
