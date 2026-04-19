# kexec-loader

Safe wrapper around [`kexec_file_load(2)`](https://man7.org/linux/man-pages/man2/kexec_file_load.2.html) for boot handoff on Linux. Loads a kernel + initrd + cmdline into the running kernel's reserved kexec memory region and invokes `reboot(LINUX_REBOOT_CMD_KEXEC)` to jump into it — all without going through BIOS/UEFI or the bootloader a second time.

Part of the [aegis-boot](https://github.com/williamzujkowski/aegis-boot) rescue environment — a signed-chain UEFI Secure Boot stick that boots any ISO.

## Scope

Only [`kexec_file_load(2)`](https://man7.org/linux/man-pages/man2/kexec_file_load.2.html) is supported. The classic [`kexec_load(2)`](https://man7.org/linux/man-pages/man2/kexec_load.2.html) is intentionally **not** exposed:

- It is blocked under `lockdown=integrity` (which aegis-boot requires for its SB-enforced handoff).
- It has no upstream signature-verification story — `KEXEC_SIG` only applies to `kexec_file_load`.

See [ADR 0001](https://github.com/williamzujkowski/aegis-boot/blob/main/docs/adr/0001-runtime-architecture.md) in the parent project for the Secure Boot rationale.

## Platform support

| Target                     | Behavior                                                  |
| -------------------------- | --------------------------------------------------------- |
| `target_os = "linux"`      | Functional — shells out to `kexec_file_load(2)` via libc  |
| Any other target           | Compiles; every public fn returns `KexecError::Unsupported` |

Non-Linux builds compile cleanly so downstream workspaces stay portable.

## Safety

One narrowly-scoped `unsafe` block: the syscall invocation itself. Inputs are rigorously validated before the syscall — paths are canonicalized and must exist as regular files, cmdline is NUL-terminated and length-checked. See the module docs for the full invariant list.

## Usage

```text
// Illustrative shape only — the real API surface (field names,
// error types, Result shape) is documented inline on the
// `load_and_exec` item below.
use kexec_loader::{load_and_exec, KexecRequest};
use std::path::Path;

let req = KexecRequest {
    kernel: Path::new("/run/media/aegis-isos/live/vmlinuz"),
    initrd: Path::new("/run/media/aegis-isos/live/initrd.gz"),
    cmdline: "root=LABEL=RESCUE quiet",
};
load_and_exec(&req)?;  // does not return on success — the process is replaced
```

See the [API docs](https://docs.rs/kexec-loader) for the full surface.

## Status

**Pre-1.0.** API is settling through real-hardware validation on the parent project's test fleet. Publishing to crates.io at 1.0. Until then, consume via the [aegis-boot workspace](https://github.com/williamzujkowski/aegis-boot).

## License

Licensed under either of [Apache-2.0](../../LICENSE-APACHE) or [MIT](../../LICENSE-MIT) at your option.
