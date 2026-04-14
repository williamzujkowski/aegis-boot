//! Thin wrapper around `kexec_file_load(2)` for the aegis-boot rescue TUI.
//!
//! # Scope
//!
//! Only the file-descriptor-based [`kexec_file_load(2)`] syscall is supported.
//! The classic `kexec_load(2)` is intentionally **not** exposed:
//!
//! - It is blocked under `lockdown=integrity` (which we require).
//! - It has no upstream signature-verification story — `KEXEC_SIG` only
//!   applies to `kexec_file_load`.
//!
//! See [ADR 0001](../../../docs/adr/0001-runtime-architecture.md) for the
//! Secure Boot rationale.
//!
//! # Status
//!
//! Skeleton only. Implementation will wire `libc::syscall(SYS_kexec_file_load, ...)`
//! behind a typed API and surface the kernel's signature-verification result.
//!
//! [`kexec_file_load(2)`]: https://man7.org/linux/man-pages/man2/kexec_file_load.2.html

#![forbid(unsafe_code)]

use std::path::PathBuf;

/// Parameters for a `kexec_file_load` invocation.
#[derive(Debug, Clone)]
pub struct KexecRequest {
    /// Path to the target kernel image (must be signed by a key in the
    /// platform or MOK keyring when SB is enforced).
    pub kernel: PathBuf,
    /// Optional initrd path.
    pub initrd: Option<PathBuf>,
    /// Kernel command line.
    pub cmdline: String,
}

/// Errors returned while preparing or invoking kexec.
#[derive(Debug, thiserror::Error)]
pub enum KexecError {
    /// Underlying syscall failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// Kernel signature verification failed (KEXEC_SIG).
    #[error("kernel signature verification failed (KEXEC_SIG rejected image)")]
    SignatureRejected,
    /// Lockdown / SB refused the operation.
    #[error("operation refused by kernel lockdown")]
    LockdownRefused,
}

/// Load and immediately exec the requested kernel via `kexec_file_load(2)`.
///
/// This function does not return on success — the calling process is replaced
/// by the new kernel once `reboot(LINUX_REBOOT_CMD_KEXEC)` is issued. On
/// failure it returns a classified [`KexecError`] so the TUI can present a
/// specific diagnostic (bad signature vs lockdown vs I/O) instead of a black
/// screen.
///
/// # Errors
///
/// See [`KexecError`].
pub fn load_and_exec(_req: &KexecRequest) -> Result<std::convert::Infallible, KexecError> {
    // TODO(#4): bind libc::SYS_kexec_file_load + LINUX_REBOOT_CMD_KEXEC.
    Err(KexecError::LockdownRefused)
}
