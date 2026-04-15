//! TPM PCR extension for kexec attestation.
//!
//! Before aegis-boot hands control to a user-selected ISO via kexec, we
//! measure `sha256(iso_path || cmdline)` into a PCR — by default PCR 12,
//! which is in the firmware-agnostic "application bootloader" range that
//! TPM PCR spec reserves for tools like this.
//!
//! Why:
//! - Downstream attestation: after kexec, the booted OS can read PCR 12
//!   and verify what aegis-boot selected. A remote attestation server
//!   can decide whether to release secrets based on that value.
//! - Forensic audit: even without remote attestation, a local sealed
//!   key tied to a specific PCR 12 value will fail to unseal if the
//!   operator selected a different ISO.
//!
//! Trade-offs:
//! - Shells out to `tpm2_pcrextend` (from `tpm2-tools`) rather than
//!   linking `libtss2-esys`. Keeps the trusted-path crate surface
//!   small; the tool is 2-3 MB compiled and trivial to ship in the
//!   initramfs alongside busybox + losetup.
//! - On hardware without TPM (or where the device is busy), the
//!   function logs a warning and returns an [`Unavailable`][TpmError::Unavailable]
//!   error. rescue-tui continues to kexec — TPM measurement is
//!   policy, not a hard gate, because the rescue use case is often
//!   physical-access recovery where the operator may legitimately
//!   want to boot without attestation infra.

use std::path::Path;
use std::process::Command;

use sha2::{Digest, Sha256};

/// Default PCR index to extend. TPM2 spec reserves PCR 8..15 for OS
/// components; 12 is the conventional "boot-loader measurement" slot.
pub const DEFAULT_PCR: u32 = 12;

/// Outcome of a PCR-extend attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TpmError {
    /// No `/dev/tpm*` device or `tpm2_pcrextend` not on PATH.
    /// Recoverable — rescue-tui proceeds without measurement.
    Unavailable(String),
    /// The tool ran but exited non-zero. stderr preserved.
    ToolFailed(String),
}

impl std::fmt::Display for TpmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(r) => write!(f, "TPM unavailable: {r}"),
            Self::ToolFailed(r) => write!(f, "tpm2_pcrextend failed: {r}"),
        }
    }
}

impl std::error::Error for TpmError {}

/// Compute the measurement hash that gets extended into the PCR.
///
/// Algorithm: `sha256(iso_path_bytes || 0x00 || cmdline_bytes)`.
/// The NUL separator prevents ambiguity between long paths and short
/// paths + long cmdlines that would otherwise hash identically.
#[must_use]
pub fn compute_measurement(iso_path: &Path, cmdline: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    // PathBuf has no guaranteed byte repr; use os_str bytes on Unix.
    h.update(iso_path.as_os_str().as_encoded_bytes());
    h.update([0u8]);
    h.update(cmdline.as_bytes());
    h.finalize().into()
}

/// Extend `measurement` into `pcr` on the local TPM.
///
/// Returns the measurement as a lowercase hex string for logging.
///
/// # Errors
///
/// - [`TpmError::Unavailable`] — no TPM device or no `tpm2_pcrextend`.
/// - [`TpmError::ToolFailed`] — the tool ran but refused the extend.
pub fn extend_pcr(pcr: u32, measurement: &[u8; 32]) -> Result<String, TpmError> {
    if !Path::new("/dev/tpm0").exists() && !Path::new("/dev/tpmrm0").exists() {
        return Err(TpmError::Unavailable("no /dev/tpm0 or /dev/tpmrm0".to_string()));
    }
    if Command::new("tpm2_pcrextend")
        .arg("--version")
        .output()
        .is_err()
    {
        return Err(TpmError::Unavailable(
            "tpm2_pcrextend not on PATH".to_string(),
        ));
    }

    let hex = hex::encode(measurement);
    let arg = format!("{pcr}:sha256={hex}");
    let out = Command::new("tpm2_pcrextend")
        .arg(&arg)
        .output()
        .map_err(|e| TpmError::ToolFailed(format!("exec: {e}")))?;
    if !out.status.success() {
        return Err(TpmError::ToolFailed(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(hex)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn measurement_is_deterministic() {
        let p = PathBuf::from("/run/media/fixture.iso");
        let c = "quiet splash";
        let a = compute_measurement(&p, c);
        let b = compute_measurement(&p, c);
        assert_eq!(a, b);
    }

    #[test]
    fn measurement_changes_on_path() {
        let a = compute_measurement(&PathBuf::from("/a.iso"), "c");
        let b = compute_measurement(&PathBuf::from("/b.iso"), "c");
        assert_ne!(a, b);
    }

    #[test]
    fn measurement_changes_on_cmdline() {
        let p = PathBuf::from("/x.iso");
        let a = compute_measurement(&p, "quiet");
        let b = compute_measurement(&p, "quiet splash");
        assert_ne!(a, b);
    }

    #[test]
    fn nul_separator_prevents_path_cmdline_ambiguity() {
        // Without a separator: "/foo/bar" + "baz" == "/foo/" + "barbaz".
        // With NUL: the two differ.
        let a = compute_measurement(&PathBuf::from("/foo/bar"), "baz");
        let b = compute_measurement(&PathBuf::from("/foo/"), "barbaz");
        assert_ne!(a, b);
    }

    #[test]
    fn tpm_error_display() {
        assert_eq!(
            format!("{}", TpmError::Unavailable("no device".to_string())),
            "TPM unavailable: no device"
        );
        assert_eq!(
            format!("{}", TpmError::ToolFailed("busy".to_string())),
            "tpm2_pcrextend failed: busy"
        );
    }
}
