// SPDX-License-Identifier: MIT OR Apache-2.0

// Phase 6 of #286 — README.md becomes the rustdoc landing page.
// `clippy::doc_markdown = allow` — README prose; see iso-parser for
// rationale.
#![allow(clippy::doc_markdown)]
#![doc = include_str!("../README.md")]
//!
//! ---
//!
//! # Rust API
//!
//! Safe wrapper around `kexec_file_load(2)` for the aegis-boot rescue TUI.
//!
//! # Scope
//!
//! Only the file-descriptor-based [`kexec_file_load(2)`] syscall is supported.
//! The classic [`kexec_load(2)`] is intentionally **not** exposed:
//!
//! - It is blocked under `lockdown=integrity` (which we require).
//! - It has no upstream signature-verification story — `KEXEC_SIG` only
//!   applies to `kexec_file_load`.
//!
//! See [ADR 0001](https://github.com/williamzujkowski/aegis-boot/blob/main/docs/adr/0001-runtime-architecture.md)
//! for the Secure Boot rationale.
//!
//! # Safety
//!
//! This crate opts into `unsafe` narrowly (see [`syscall`] module) to invoke
//! `kexec_file_load(2)` and `reboot(2)`. Every unsafe block documents its
//! invariant. The rest of the workspace is `unsafe_code = forbid`.
//!
//! [`kexec_file_load(2)`]: https://man7.org/linux/man-pages/man2/kexec_file_load.2.html
//! [`kexec_load(2)`]: https://man7.org/linux/man-pages/man2/kexec_load.2.html

use std::ffi::CString;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
mod syscall;

/// Parameters for a `kexec_file_load` invocation.
#[derive(Debug, Clone)]
pub struct KexecRequest {
    /// Path to the target kernel image. Must be signed by a key in the
    /// platform or MOK keyring when Secure Boot is enforced.
    pub kernel: PathBuf,
    /// Optional initrd path.
    pub initrd: Option<PathBuf>,
    /// Kernel command line.
    pub cmdline: String,
}

/// Errors returned while preparing or invoking kexec.
///
/// Classification is deliberately narrow so the TUI can render a specific,
/// user-actionable diagnostic instead of a black screen.
#[derive(Debug, thiserror::Error)]
pub enum KexecError {
    /// Kernel signature verification (`KEXEC_SIG`) rejected the image.
    ///
    /// Maps to `EKEYREJECTED` from `kexec_file_load(2)`. Typical cause: the
    /// ISO's kernel is self-signed or signed by a CA not present in the
    /// platform / MOK keyring. User remedy: enroll the key via `mokutil`.
    #[error("kernel signature verification failed (KEXEC_SIG rejected the image)")]
    SignatureRejected,

    /// Kernel lockdown / Secure Boot refused the operation.
    ///
    /// Maps to `EPERM` from `kexec_file_load(2)` when lockdown is integrity
    /// or confidentiality mode. User remedy: none — by design.
    #[error("operation refused by kernel lockdown (Secure Boot enforcing)")]
    LockdownRefused,

    /// Image format rejected by the kernel's file loader (e.g. not a bzImage).
    ///
    /// Maps to `ENOEXEC`.
    #[error("kernel image format not recognized (kexec_file_load returned ENOEXEC)")]
    UnsupportedImage,

    /// Underlying I/O or file-descriptor failure.
    #[error("io error: {0}")]
    Io(#[from] io::Error),

    /// Caller supplied a path containing an interior NUL byte.
    #[error("path contains interior NUL byte: {0}")]
    InvalidPath(PathBuf),

    /// Crate compiled for a non-Linux target; no kexec available.
    #[error("kexec is only available on Linux")]
    Unsupported,
}

/// Load the requested kernel via `kexec_file_load(2)` without triggering the
/// subsequent reboot.
///
/// On success the image is staged in kernel memory and
/// `/sys/kernel/kexec_loaded` flips to `1`. Callers that want to actually
/// hand off to the loaded kernel should use [`load_and_exec`]; this entry
/// point exists so integration tests can verify the syscall path against a
/// real kernel without replacing the test process.
///
/// # Errors
///
/// See [`KexecError`].
#[cfg(target_os = "linux")]
pub fn load_dry(req: &KexecRequest) -> Result<(), KexecError> {
    let kernel_fd = open_path(&req.kernel)?;
    let initrd_fd = req.initrd.as_deref().map(open_path).transpose()?;
    let cmdline = CString::new(req.cmdline.as_bytes())
        .map_err(|_| KexecError::InvalidPath(PathBuf::from(&req.cmdline)))?;
    syscall::kexec_file_load(
        kernel_fd.as_raw(),
        initrd_fd.as_ref().map(OwnedFd::as_raw),
        &cmdline,
    )
}

/// Non-Linux stub — always returns [`KexecError::Unsupported`].
#[cfg(not(target_os = "linux"))]
pub fn load_dry(_req: &KexecRequest) -> Result<(), KexecError> {
    Err(KexecError::Unsupported)
}

/// Load the requested kernel via `kexec_file_load(2)` and immediately trigger
/// `reboot(LINUX_REBOOT_CMD_KEXEC)`.
///
/// On success this function **does not return** — the calling process is
/// replaced by the new kernel. Returning [`Ok`] with [`std::convert::Infallible`]
/// is unreachable in practice; the type signature documents the intent.
///
/// # Errors
///
/// See [`KexecError`] — every non-success path is classified so the caller
/// can present a specific diagnostic.
#[cfg(target_os = "linux")]
pub fn load_and_exec(req: &KexecRequest) -> Result<std::convert::Infallible, KexecError> {
    load_dry(req)?;
    syscall::reboot_kexec()?;
    // reboot_kexec never returns on success. If we got here, treat as Io.
    Err(KexecError::Io(io::Error::other(
        "reboot(LINUX_REBOOT_CMD_KEXEC) returned unexpectedly",
    )))
}

/// Non-Linux stub — always returns [`KexecError::Unsupported`].
#[cfg(not(target_os = "linux"))]
pub fn load_and_exec(_req: &KexecRequest) -> Result<std::convert::Infallible, KexecError> {
    Err(KexecError::Unsupported)
}

/// RAII owned file descriptor. Closes on drop.
///
/// We don't use `std::os::fd::OwnedFd` directly so the drop behavior and
/// raw-fd extraction can be reviewed in one place alongside the syscall.
#[cfg(target_os = "linux")]
struct OwnedFd(libc::c_int);

#[cfg(target_os = "linux")]
impl OwnedFd {
    fn as_raw(&self) -> libc::c_int {
        self.0
    }
}

#[cfg(target_os = "linux")]
impl Drop for OwnedFd {
    fn drop(&mut self) {
        // SAFETY: fd was obtained from `open(2)` in `open_path` and is not
        // shared. Closing an already-closed or never-opened fd would be UB,
        // but construction paths guarantee `self.0 >= 0` and single-owner.
        #[allow(unsafe_code)]
        // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
        unsafe {
            libc::close(self.0);
        }
    }
}

#[cfg(target_os = "linux")]
fn open_path(path: &Path) -> Result<OwnedFd, KexecError> {
    let c_path = path_to_cstring(path)?;
    // SAFETY: `c_path` is a valid NUL-terminated C string pointing to an
    // owned buffer that outlives the syscall. `O_RDONLY | O_CLOEXEC` is a
    // safe flag combination. Return value is checked below.
    #[allow(unsafe_code)]
    // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
    let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(KexecError::Io(io::Error::last_os_error()));
    }
    Ok(OwnedFd(fd))
}

fn path_to_cstring(path: &Path) -> Result<CString, KexecError> {
    use std::os::unix::ffi::OsStrExt;
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| KexecError::InvalidPath(path.to_path_buf()))
}

/// Classify a raw `errno` from `kexec_file_load(2)` into [`KexecError`].
///
/// Exposed so tests can verify the mapping without issuing the real syscall.
#[must_use]
pub fn classify_errno(errno: i32) -> KexecError {
    match errno {
        libc::EKEYREJECTED => KexecError::SignatureRejected,
        libc::EPERM => KexecError::LockdownRefused,
        libc::ENOEXEC => KexecError::UnsupportedImage,
        other => KexecError::Io(io::Error::from_raw_os_error(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_signature_rejection() {
        assert!(matches!(
            classify_errno(libc::EKEYREJECTED),
            KexecError::SignatureRejected
        ));
    }

    #[test]
    fn classify_lockdown() {
        assert!(matches!(
            classify_errno(libc::EPERM),
            KexecError::LockdownRefused
        ));
    }

    #[test]
    fn classify_bad_image() {
        assert!(matches!(
            classify_errno(libc::ENOEXEC),
            KexecError::UnsupportedImage
        ));
    }

    #[test]
    fn classify_generic_io_preserves_errno() {
        let err = classify_errno(libc::ENOENT);
        let KexecError::Io(io_err) = err else {
            panic!("expected Io variant");
        };
        assert_eq!(io_err.raw_os_error(), Some(libc::ENOENT));
    }

    #[test]
    fn path_ok_round_trips() {
        let ok = Path::new("/boot/vmlinuz-rescue");
        let c = path_to_cstring(ok).unwrap_or_else(|_| panic!("valid path"));
        assert_eq!(c.to_bytes(), b"/boot/vmlinuz-rescue");
    }

    #[test]
    fn path_with_nul_byte_rejected() {
        let bad = Path::new("/tmp/has\0nul");
        assert!(matches!(
            path_to_cstring(bad),
            Err(KexecError::InvalidPath(_))
        ));
    }

    /// Integration guard: exercising the real syscall requires `CAP_SYS_BOOT`
    /// and will reboot the machine on success. Run manually only.
    #[test]
    #[ignore = "requires root + would kexec the host; opt-in via `cargo test -- --ignored`"]
    #[cfg(target_os = "linux")]
    fn load_and_exec_rejects_nonexistent_kernel() {
        let req = KexecRequest {
            kernel: PathBuf::from("/nonexistent/vmlinuz"),
            initrd: None,
            cmdline: String::new(),
        };
        let err = load_and_exec(&req).expect_err("must fail");
        assert!(matches!(err, KexecError::Io(_)));
    }
}
