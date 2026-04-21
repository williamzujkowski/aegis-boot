// SPDX-License-Identifier: MIT OR Apache-2.0

// First production caller (`flash::build_flash_plan`) uses a subset of
// `Operation` variants + `Plan::new`/`add`. Variants and accessors reserved
// for the `update`/`add`/`init`/`expand` rollout per #247 are kept
// behind this module-level allow until those commands wire up.
#![allow(dead_code)]

//! `Plan` — typed, inspectable description of operations a command would perform.
//!
//! A command builds a `Plan` before executing it. Then either:
//!   - prints the plan and exits (`--dry-run` mode), or
//!   - executes the plan in order and emits a receipt.
//!
//! First production caller: `flash::build_flash_plan`. Per-command rollout
//! continues in follow-ups (`update`, `add`, `init`, `expand`) per #247.
//!
//! # Why a typed plan, not a `Vec<String>`
//!
//! A free-text plan is impossible to programmatically inspect, can't
//! enforce that every command lists *all* its side-effects, and drifts
//! from reality every time someone adds a new step without updating the
//! description. A typed `Operation` enum makes every command's
//! side-effect surface visible to `--dry-run` *by construction*.
//!
//! # Backwards compatibility
//!
//! The `Operation` variants are append-only — adding a new variant is a
//! semver-minor change. Renaming or repurposing a variant is a breaking
//! change and requires bumping the major version of `aegis-cli`.

use std::fmt;
use std::path::PathBuf;

/// One side-effecting operation that a command intends to perform.
///
/// New side-effect classes get a new variant — that way `--dry-run`
/// stays exhaustive by construction.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Operation {
    /// Verify a cryptographic signature against a key.
    VerifySignature {
        /// Path to the artifact whose signature is being verified.
        artifact: PathBuf,
        /// Identifier of the key the signature is verified against
        /// (e.g. `cosign:williamzujkowski`, `gpg:0xABCD…`).
        key_id: String,
        /// Algorithm name (e.g. `sha256`, `ed25519`).
        algorithm: String,
    },

    /// Refuse to proceed unless the named predicate holds. Used at the
    /// top of a plan to surface the safety gates a command will apply.
    PrecheckRefuseUnless {
        /// One-line predicate name (e.g. `removable && transport=usb`).
        predicate: String,
        /// Human-readable detail of how the predicate is currently
        /// satisfied or rejected.
        details: String,
    },

    /// Write bytes to a block device.
    WriteToBlockDevice {
        /// Target block device path.
        device: PathBuf,
        /// Source file (image / partition payload).
        source: PathBuf,
        /// Number of bytes the operation will write.
        bytes: u64,
        /// Optional ETA hint shown in dry-run output.
        estimated_duration_secs: Option<u64>,
    },

    /// Read back N bytes from a device and verify a hash.
    ReadbackVerify {
        /// Device to read back from.
        device: PathBuf,
        /// Number of bytes to read back.
        bytes: u64,
        /// Optional sha256 (hex) the readback must match.
        expected_sha256: Option<String>,
    },

    /// Persist an attestation receipt to disk.
    WriteAttestation {
        /// Destination path of the receipt file.
        destination: PathBuf,
    },

    /// Modify a partition table on a device (e.g. expand last
    /// partition, recreate ESP).
    ModifyPartitionTable {
        /// Target device.
        device: PathBuf,
        /// Free-text action description.
        action: String,
    },

    /// Resize a filesystem in place.
    ResizeFilesystem {
        /// Device hosting the filesystem (e.g. `/dev/sda2`).
        device: PathBuf,
        /// New filesystem size in bytes.
        new_size_bytes: u64,
    },

    /// Mount a filesystem.
    Mount {
        /// Source device or image.
        source: PathBuf,
        /// Mount point.
        target: PathBuf,
        /// Filesystem type (e.g. `vfat`, `exfat`, `iso9660`).
        fs: String,
    },

    /// Unmount a filesystem.
    Unmount {
        /// Mount point to unmount.
        target: PathBuf,
    },

    /// Copy a file.
    CopyFile {
        /// Source path.
        source: PathBuf,
        /// Target path.
        target: PathBuf,
        /// Bytes copied.
        bytes: u64,
    },

    /// Add or remove an entry in a signed manifest.
    UpdateManifest {
        /// Path to the manifest being mutated.
        path: PathBuf,
        /// Free-text change description.
        change: String,
    },
}

/// An ordered list of `Operation`s a command will perform plus a
/// one-line description of the overall intent.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    /// One-line description of the command's overall goal.
    intent: String,
    /// Operations in execution order.
    operations: Vec<Operation>,
}

impl Plan {
    /// Create a new empty plan with the given intent.
    #[must_use]
    pub fn new(intent: impl Into<String>) -> Self {
        Self {
            intent: intent.into(),
            operations: Vec::new(),
        }
    }

    /// Append an operation. Returns `&mut Self` for fluent chaining.
    pub fn add(&mut self, op: Operation) -> &mut Self {
        self.operations.push(op);
        self
    }

    /// Borrow the intent.
    #[must_use]
    pub fn intent(&self) -> &str {
        &self.intent
    }

    /// Borrow the operations.
    #[must_use]
    pub fn operations(&self) -> &[Operation] {
        &self.operations
    }

    /// Number of operations in the plan.
    #[must_use]
    pub fn len(&self) -> usize {
        self.operations.len()
    }

    /// Whether the plan has no operations.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }
}

impl fmt::Display for Plan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.intent.is_empty() {
            writeln!(f, "Plan: {}", self.intent)?;
        }
        if self.operations.is_empty() {
            writeln!(f, "  (no operations)")?;
        }
        for (idx, op) in self.operations.iter().enumerate() {
            writeln!(f, "  {}. {}", idx + 1, render_op(op))?;
        }
        Ok(())
    }
}

fn render_op(op: &Operation) -> String {
    match op {
        Operation::VerifySignature {
            artifact,
            key_id,
            algorithm,
        } => format!(
            "Verify {algorithm} signature on {} against key {key_id}",
            artifact.display()
        ),
        Operation::PrecheckRefuseUnless { predicate, details } => {
            format!("Precheck: refuse unless {predicate} ({details})")
        }
        Operation::WriteToBlockDevice {
            device,
            source,
            bytes,
            estimated_duration_secs,
        } => {
            let eta = estimated_duration_secs
                .map(|s| format!(" (~{s}s)"))
                .unwrap_or_default();
            format!(
                "Write {bytes} bytes from {} to {}{eta}",
                source.display(),
                device.display()
            )
        }
        Operation::ReadbackVerify {
            device,
            bytes,
            expected_sha256,
        } => {
            let hash = expected_sha256
                .as_deref()
                .map(|h| format!(" against sha256 {}", &h[..h.len().min(12)]))
                .unwrap_or_default();
            format!(
                "Read back first {bytes} bytes of {} and verify{hash}",
                device.display()
            )
        }
        Operation::WriteAttestation { destination } => {
            format!("Write attestation receipt to {}", destination.display())
        }
        Operation::ModifyPartitionTable { device, action } => {
            format!("{action} on partition table of {}", device.display())
        }
        Operation::ResizeFilesystem {
            device,
            new_size_bytes,
        } => format!(
            "Resize filesystem on {} to {new_size_bytes} bytes",
            device.display()
        ),
        Operation::Mount { source, target, fs } => {
            format!("Mount {} at {} as {fs}", source.display(), target.display())
        }
        Operation::Unmount { target } => format!("Unmount {}", target.display()),
        Operation::CopyFile {
            source,
            target,
            bytes,
        } => format!(
            "Copy {} ({bytes} bytes) to {}",
            source.display(),
            target.display()
        ),
        Operation::UpdateManifest { path, change } => {
            format!("Update manifest at {}: {change}", path.display())
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn empty_plan_renders_with_no_operations_marker() {
        let plan = Plan::new("");
        let s = plan.to_string();
        assert!(s.contains("(no operations)"), "got: {s}");
    }

    #[test]
    fn intent_appears_at_top_when_set() {
        let plan = Plan::new("flash a USB stick");
        let s = plan.to_string();
        assert!(s.starts_with("Plan: flash a USB stick"), "got: {s}");
    }

    #[test]
    fn add_appends_in_order_and_numbers_steps() {
        let mut plan = Plan::new("two-step plan");
        plan.add(Operation::PrecheckRefuseUnless {
            predicate: "is_usb".into(),
            details: "transport=usb".into(),
        });
        plan.add(Operation::WriteAttestation {
            destination: PathBuf::from("/tmp/receipt.json"),
        });
        let s = plan.to_string();
        let line1_pos = s.find("1. Precheck").unwrap();
        let line2_pos = s.find("2. Write attestation").unwrap();
        assert!(line1_pos < line2_pos, "operations out of order: {s}");
        assert_eq!(plan.len(), 2);
        assert!(!plan.is_empty());
    }

    #[test]
    fn write_to_block_device_renders_eta_when_present() {
        let mut plan = Plan::new("");
        plan.add(Operation::WriteToBlockDevice {
            device: PathBuf::from("/dev/sda"),
            source: PathBuf::from("/tmp/aegis.img"),
            bytes: 2_147_483_648,
            estimated_duration_secs: Some(240),
        });
        let s = plan.to_string();
        assert!(s.contains("~240s"), "expected ETA in render: {s}");
    }

    #[test]
    fn write_to_block_device_omits_eta_when_absent() {
        let mut plan = Plan::new("");
        plan.add(Operation::WriteToBlockDevice {
            device: PathBuf::from("/dev/sda"),
            source: PathBuf::from("/tmp/aegis.img"),
            bytes: 1024,
            estimated_duration_secs: None,
        });
        let s = plan.to_string();
        assert!(!s.contains('~'), "expected no ETA marker: {s}");
    }

    #[test]
    fn readback_verify_truncates_long_sha256() {
        let long_hash = "a".repeat(64);
        let mut plan = Plan::new("");
        plan.add(Operation::ReadbackVerify {
            device: PathBuf::from("/dev/sda"),
            bytes: 65_536,
            expected_sha256: Some(long_hash),
        });
        let s = plan.to_string();
        // Truncated to first 12 chars to keep the dry-run output readable.
        assert!(s.contains("aaaaaaaaaaaa"), "got: {s}");
        assert!(
            !s.contains(&"a".repeat(20)),
            "expected truncation, got full hash: {s}"
        );
    }

    #[test]
    fn readback_verify_omits_hash_when_none() {
        let mut plan = Plan::new("");
        plan.add(Operation::ReadbackVerify {
            device: PathBuf::from("/dev/sda"),
            bytes: 65_536,
            expected_sha256: None,
        });
        let s = plan.to_string();
        assert!(!s.contains("sha256"), "got: {s}");
    }

    #[test]
    fn intent_borrow_matches_input() {
        let plan = Plan::new("verify and flash");
        assert_eq!(plan.intent(), "verify and flash");
    }

    #[test]
    fn operations_borrow_returns_inserted_items() {
        let mut plan = Plan::new("");
        let op = Operation::Unmount {
            target: PathBuf::from("/mnt/aegis"),
        };
        plan.add(op.clone());
        assert_eq!(plan.operations(), &[op]);
    }
}
