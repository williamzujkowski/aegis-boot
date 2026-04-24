// SPDX-License-Identifier: MIT OR Apache-2.0

//! Consistent-obfuscation redaction for bug-report bundles (#342).
//!
//! Pattern borrowed from [`sos clean`](https://manpages.ubuntu.com/manpages/jammy/man1/sos-clean.1.html):
//! the same real value deterministically maps to the same synthetic value
//! across every appearance in a bundle, so structural relationships
//! (e.g., "this hostname appears in both `/etc/hostname` and `lsblk`")
//! are preserved while the PII itself isn't. The real↔synthetic mapping
//! is held by the [`Redactor`] and can be dumped to disk via
//! [`Redactor::dump_mapping`] — operators keep that file locally so they
//! can de-anonymize their own bundle if they need to.
//!
//! Hashing uses SHA-256 truncated to 6 hex chars (24 bits). That's
//! enough entropy for uniqueness inside a single bundle (dozens to
//! hundreds of distinct values, not millions) and short enough that
//! `host-ab12cd` reads as clearly-synthetic to a human.
//!
//! What gets redacted:
//! * hostname
//! * username
//! * DMI serial numbers (laptop chassis, motherboard, system)
//! * drive serial numbers
//! * MAC addresses (`xx:xx:xx:xx:xx:xx`)
//! * IPv4 addresses (stable remap to `10.0.0.<N>` per value)
//!
//! What does NOT get redacted (still treated as non-PII):
//! * DMI vendor / product / BIOS version (the point of a bug report
//!   is often to correlate per-vendor behavior)
//! * kernel version, module names, `/proc/cmdline` (no per-user data)
//! * aegis-boot version + doctor verdicts (wire-format contract)
//!
//! `--no-redact` removes all of the above; operators have to type a
//! confirmation string to use it.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Consistent-obfuscation mapper. Deterministic within a single run,
/// non-deterministic across runs (the salt changes so two separate
/// bundles from the same machine don't link).
pub(crate) struct Redactor {
    /// Real → synthetic mapping accumulated as values are redacted.
    mapping: BTreeMap<String, String>,
    /// Per-run salt mixed into every hash. Prevents two bundles from
    /// the same host linking on `host-abc123` style tokens.
    salt: [u8; 16],
    /// When false, redaction is a no-op (identity passthrough). Used
    /// for `--no-redact`.
    active: bool,
}

impl Redactor {
    pub(crate) fn new(active: bool) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static CONSTRUCTION_COUNTER: AtomicU64 = AtomicU64::new(0);

        let mut salt = [0u8; 16];
        // Fill salt from (nanos-precision timestamp) XOR'd with (PID)
        // XOR'd with (monotonic in-process counter). Not cryptographic
        // salting; just enough entropy to unlink two runs on the same
        // host AND to differentiate two Redactors constructed back-to-
        // back within the same clock granularity (macOS's
        // `SystemTime::now` reports microsecond-resolution values on
        // many configurations — so two consecutive calls can return
        // identical nanos. The counter closes that gap).
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = u128::from(std::process::id());
        let counter = u128::from(CONSTRUCTION_COUNTER.fetch_add(1, Ordering::Relaxed));
        let mixed = nanos
            .wrapping_mul(pid.wrapping_add(1))
            .wrapping_add(counter.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        for (i, byte) in mixed.to_le_bytes().iter().enumerate() {
            salt[i] = *byte;
        }
        Self {
            mapping: BTreeMap::new(),
            salt,
            active,
        }
    }

    /// Redact a hostname. Empty input → empty output.
    pub(crate) fn hostname(&mut self, real: &str) -> String {
        if !self.active || real.is_empty() {
            return real.to_string();
        }
        self.remap(real, "host-")
    }

    pub(crate) fn username(&mut self, real: &str) -> String {
        if !self.active || real.is_empty() {
            return real.to_string();
        }
        self.remap(real, "user-")
    }

    /// Redact a DMI / drive serial. Keeps the short marker "serial-"
    /// so readers know what was redacted.
    pub(crate) fn serial(&mut self, real: &str) -> String {
        if !self.active || real.is_empty() {
            return real.to_string();
        }
        self.remap(real, "serial-")
    }

    /// Replace all occurrences of the Redactor's real values with
    /// their synthetic mappings inside `text`. Used to sweep through
    /// multi-line captures (dmesg, lsblk) after individual field
    /// extractions have populated the mapping.
    pub(crate) fn sweep(&self, text: &str) -> String {
        if !self.active {
            return text.to_string();
        }
        let mut out = text.to_string();
        for (real, synthetic) in &self.mapping {
            if real.len() >= 3 {
                // Avoid accidental mass-replacement of short strings
                // like "a" or "go". 3 char floor is a simple but
                // effective guard.
                out = out.replace(real, synthetic);
            }
        }
        out
    }

    /// Serialize the real↔synthetic mapping for local persistence.
    /// Format: one `real<TAB>synthetic` line per entry, sorted.
    pub(crate) fn dump_mapping(&self) -> String {
        let mut out = String::from(
            "# aegis-boot bug-report redaction map\n\
             # Keep this file LOCAL — it de-anonymizes any bundle you share.\n\
             # Format: <real>\\t<synthetic>\n",
        );
        for (real, synthetic) in &self.mapping {
            out.push_str(real);
            out.push('\t');
            out.push_str(synthetic);
            out.push('\n');
        }
        out
    }

    /// Returns `true` when redaction is active. Operators explicitly
    /// passing `--no-redact` construct a [`Redactor`] with `active=false`.
    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    fn remap(&mut self, real: &str, prefix: &str) -> String {
        if let Some(existing) = self.mapping.get(real) {
            return existing.clone();
        }
        let mut hasher = Sha256::new();
        hasher.update(self.salt);
        hasher.update(real.as_bytes());
        let digest = hasher.finalize();
        let mut token = String::with_capacity(6);
        for byte in digest.iter().take(3) {
            use std::fmt::Write as _;
            let _ = write!(token, "{byte:02x}");
        }
        let synthetic = format!("{prefix}{token}");
        self.mapping.insert(real.to_string(), synthetic.clone());
        synthetic
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn hostname_remaps_consistently_within_one_run() {
        let mut r = Redactor::new(true);
        let a = r.hostname("my-laptop");
        let b = r.hostname("my-laptop");
        assert_eq!(a, b, "same input must map to same output");
        assert!(a.starts_with("host-"));
        assert_eq!(a.len(), "host-".len() + 6);
    }

    #[test]
    fn different_values_get_different_synthetic_tokens() {
        let mut r = Redactor::new(true);
        let a = r.hostname("alpha");
        let b = r.hostname("bravo");
        assert_ne!(a, b);
    }

    #[test]
    fn two_redactors_differ_on_same_input() {
        // Salt differs across runs so two bundles don't link.
        // This is a probabilistic property (vanishingly unlikely to
        // collide), so if it ever flakes, revisit salt entropy.
        let mut r1 = Redactor::new(true);
        let mut r2 = Redactor::new(true);
        assert_ne!(r1.hostname("same-host"), r2.hostname("same-host"));
    }

    #[test]
    fn inactive_redactor_is_identity() {
        let mut r = Redactor::new(false);
        assert_eq!(r.hostname("my-laptop"), "my-laptop");
        assert_eq!(r.username("gary"), "gary");
        assert_eq!(r.serial("ABC123"), "ABC123");
    }

    #[test]
    fn sweep_replaces_all_occurrences() {
        let mut r = Redactor::new(true);
        let redacted_host = r.hostname("work-laptop");
        let sample = "work-laptop reports work-laptop at work-laptop";
        let swept = r.sweep(sample);
        assert_eq!(swept.matches(&redacted_host).count(), 3);
        assert!(!swept.contains("work-laptop"));
    }

    #[test]
    fn sweep_skips_short_values() {
        // Two-char real values must not sweep; they'd mass-replace.
        let mut r = Redactor::new(true);
        r.hostname("go"); // 2 chars — intentionally short
        let sample = "go to the good golang playground";
        let swept = r.sweep(sample);
        assert_eq!(swept, sample, "short values must not trigger sweep");
    }

    #[test]
    fn dump_mapping_includes_every_entry() {
        let mut r = Redactor::new(true);
        let h = r.hostname("alpha-host");
        let u = r.username("charlie");
        let dump = r.dump_mapping();
        assert!(dump.contains("alpha-host"));
        assert!(dump.contains(&h));
        assert!(dump.contains("charlie"));
        assert!(dump.contains(&u));
    }

    #[test]
    fn empty_input_passes_through() {
        let mut r = Redactor::new(true);
        assert_eq!(r.hostname(""), "");
        assert_eq!(r.username(""), "");
        assert_eq!(r.serial(""), "");
    }
}
