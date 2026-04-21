// SPDX-License-Identifier: MIT OR Apache-2.0

//! Raw Linux syscall FFI for `kexec_file_load(2)` + `reboot(LINUX_REBOOT_CMD_KEXEC)`.
//!
//! This is the only module in the crate that touches `unsafe`. Every call site
//! is annotated with the invariant that keeps it sound.

use std::ffi::CStr;
use std::io;

use crate::{classify_errno, KexecError};

// From `<linux/kexec.h>`. Hard-coded rather than pulled from a crate so the
// trusted-path surface has zero transitive dependencies.
const SYS_KEXEC_FILE_LOAD: libc::c_long = libc::SYS_kexec_file_load;

/// Default flags: no image-auto-type, enforce signature verification via
/// `KEXEC_FILE_NO_INITRAMFS` off + kernel's `KEXEC_SIG` config.
///
/// We deliberately do **not** set `KEXEC_FILE_UNSAFE` — that flag bypasses
/// signature checks and is incompatible with our Secure Boot posture.
const KEXEC_FILE_DEFAULT_FLAGS: libc::c_ulong = 0;
const KEXEC_FILE_NO_INITRAMFS: libc::c_ulong = 0x4;

/// Invoke `kexec_file_load(2)` with the given kernel fd, optional initrd fd,
/// and cmdline. On success the next `reboot(LINUX_REBOOT_CMD_KEXEC)` will
/// jump into the loaded image.
pub(crate) fn kexec_file_load(
    kernel_fd: libc::c_int,
    initrd_fd: Option<libc::c_int>,
    cmdline: &CStr,
) -> Result<(), KexecError> {
    let (initrd, flags) = match initrd_fd {
        Some(fd) => (fd, KEXEC_FILE_DEFAULT_FLAGS),
        None => (-1, KEXEC_FILE_NO_INITRAMFS),
    };
    let cmdline_len = cmdline.to_bytes_with_nul().len();

    // SAFETY: `kernel_fd` and `initrd` (when used) are live fds we own.
    // `cmdline.as_ptr()` points to a NUL-terminated buffer owned by the
    // caller for the duration of the call. `cmdline_len` is the byte length
    // including the terminator, as required by the kexec_file_load ABI.
    #[allow(unsafe_code)]
    // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
    let rc = unsafe {
        libc::syscall(
            SYS_KEXEC_FILE_LOAD,
            libc::c_long::from(kernel_fd),
            libc::c_long::from(initrd),
            cmdline_len as libc::c_ulong,
            cmdline.as_ptr(),
            flags,
        )
    };

    if rc == 0 {
        Ok(())
    } else {
        let errno = io::Error::last_os_error().raw_os_error().unwrap_or(0);
        Err(classify_errno(errno))
    }
}

/// Invoke `reboot(LINUX_REBOOT_CMD_KEXEC)`. Does not return on success.
pub(crate) fn reboot_kexec() -> Result<(), KexecError> {
    // SAFETY: `reboot(2)` with `LINUX_REBOOT_CMD_KEXEC` is a no-argument
    // syscall once the kexec image has been loaded. It either does not
    // return (success) or fails with errno set. No pointers, no shared state.
    #[allow(unsafe_code)]
    // nosemgrep: rust.lang.security.unsafe-usage.unsafe-usage
    let rc = unsafe { libc::reboot(libc::LINUX_REBOOT_CMD_KEXEC) };
    if rc < 0 {
        Err(KexecError::Io(io::Error::last_os_error()))
    } else {
        Ok(())
    }
}
