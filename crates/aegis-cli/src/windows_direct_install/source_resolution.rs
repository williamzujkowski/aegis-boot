// SPDX-License-Identifier: MIT OR Apache-2.0

//! #497 — Signed-chain source-path resolution for the Windows
//! direct-install pipeline.
//!
//! The Linux path hardcodes `/usr/lib/shim/shimx64.efi.signed` +
//! siblings because those are canonical apt-package install paths.
//! Windows has no equivalent package layout, so this module resolves
//! source paths from an operator-controlled `out_dir` (typically the
//! same directory `scripts/mkusb.sh` writes the aegis initramfs
//! into), with per-file env var overrides for power users who keep
//! the chain somewhere else.
//!
//! ## Default layout (under `out_dir`)
//!
//! | ESP path                  | Host filename         | Env override      |
//! | ------------------------- | --------------------- | ----------------- |
//! | `/EFI/BOOT/BOOTX64.EFI`   | `shimx64.efi.signed`  | `AEGIS_SHIM_SRC`  |
//! | `/EFI/BOOT/grubx64.efi`   | `grubx64.efi.signed`  | `AEGIS_GRUB_SRC`  |
//! | `/EFI/BOOT/mmx64.efi`     | `mmx64.efi.signed`    | `AEGIS_MM_SRC`    |
//! | `/EFI/BOOT/grub.cfg`      | `grub.cfg`            | `AEGIS_GRUB_CFG`  |
//! | `/vmlinuz`                | `vmlinuz`             | `AEGIS_KERNEL_SRC`|
//! | `/initramfs.cpio.gz`      | `initramfs.cpio.gz`   | `AEGIS_INITRD_SRC`|
//!
//! Env vars take precedence. If set, the override path is used
//! regardless of whether `out_dir/<default-name>` would exist.
//!
//! ## What this module is NOT
//!
//! - Not a downloader (that's #417 — runtime signed-chain fetch).
//! - Not a validator beyond "file exists + is readable" (hash /
//!   signature verification happens downstream in
//!   [`crate::windows_direct_install::raw_write::stage_esp`]'s
//!   preflight or at operator-post-flash verify time).
//! - Not a writer (the operator stages the files into `out_dir` via
//!   whatever build pipeline they use — `scripts/mkusb.sh` for Linux
//!   devs, future Windows-installer bundle, or manual copy).

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use crate::windows_direct_install::raw_write::EspStagingSources;

/// Default host-side filename for each of the 6 ESP chain files
/// when the matching env-var override is not set. Deliberately
/// matches the naming `scripts/mkusb.sh` writes into `out/`, so a
/// developer who built the chain on Linux can run direct-install
/// on Windows against the same directory without renaming.
pub(crate) const DEFAULT_SHIM_FILENAME: &str = "shimx64.efi.signed";
pub(crate) const DEFAULT_GRUB_FILENAME: &str = "grubx64.efi.signed";
pub(crate) const DEFAULT_MM_FILENAME: &str = "mmx64.efi.signed";
pub(crate) const DEFAULT_GRUB_CFG_FILENAME: &str = "grub.cfg";
pub(crate) const DEFAULT_KERNEL_FILENAME: &str = "vmlinuz";
pub(crate) const DEFAULT_INITRD_FILENAME: &str = "initramfs.cpio.gz";

/// Env var name → default-filename mapping, in the order
/// [`EspFile::ALL`] stages.
pub(crate) const ENV_SHIM_SRC: &str = "AEGIS_SHIM_SRC";
pub(crate) const ENV_GRUB_SRC: &str = "AEGIS_GRUB_SRC";
pub(crate) const ENV_MM_SRC: &str = "AEGIS_MM_SRC";
pub(crate) const ENV_GRUB_CFG: &str = "AEGIS_GRUB_CFG";
pub(crate) const ENV_KERNEL_SRC: &str = "AEGIS_KERNEL_SRC";
pub(crate) const ENV_INITRD_SRC: &str = "AEGIS_INITRD_SRC";

/// All six (env-var-name, default-filename) pairs in staging order.
/// Used by [`build_staging_sources`] and exposed publicly so the
/// `--help` text + error messages can enumerate them consistently.
pub(crate) const ENV_DEFAULT_PAIRS: [(&str, &str); 6] = [
    (ENV_SHIM_SRC, DEFAULT_SHIM_FILENAME),
    (ENV_GRUB_SRC, DEFAULT_GRUB_FILENAME),
    (ENV_MM_SRC, DEFAULT_MM_FILENAME),
    (ENV_GRUB_CFG, DEFAULT_GRUB_CFG_FILENAME),
    (ENV_KERNEL_SRC, DEFAULT_KERNEL_FILENAME),
    (ENV_INITRD_SRC, DEFAULT_INITRD_FILENAME),
];

/// Reasons source resolution can fail. Each carries enough context
/// for the operator to act without re-running in verbose mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SourceResolutionError {
    /// One (or more — all collected, not fail-fast) of the six chain
    /// files isn't at the resolved path. The vec is already formatted
    /// as `ENV_VAR or <default-path>: missing (resolved to <actual>)`
    /// lines so the caller can just join them.
    MissingFiles(Vec<MissingFile>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MissingFile {
    pub(crate) env_var: &'static str,
    pub(crate) resolved_path: PathBuf,
    pub(crate) default_filename: &'static str,
}

impl std::fmt::Display for SourceResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingFiles(v) => {
                writeln!(
                    f,
                    "windows direct-install: {} source file(s) missing:",
                    v.len()
                )?;
                for m in v {
                    writeln!(
                        f,
                        "  - {} (env {} or default filename {}): no file at {}",
                        m.default_filename,
                        m.env_var,
                        m.default_filename,
                        m.resolved_path.display()
                    )?;
                }
                write!(
                    f,
                    "Set the env var for each missing file to a different path, \
                     or build the signed chain under the out_dir first \
                     (scripts/mkusb.sh writes the Linux-equivalent filenames)."
                )
            }
        }
    }
}

/// Decide the resolved host-side path for one chain file.
///
/// Separated from [`build_staging_sources`] so a single override-vs-
/// default decision is unit-testable without needing to stand up six
/// env vars + a tempdir per case.
pub(crate) fn resolve_one(
    out_dir: &Path,
    env_override: Option<&str>,
    default_filename: &str,
) -> PathBuf {
    match env_override {
        Some(s) if !s.is_empty() => PathBuf::from(s),
        _ => out_dir.join(default_filename),
    }
}

/// Resolve all 6 chain files + validate each one exists. Accepts an
/// env-var lookup closure so the call site decides whether to query
/// the real environment or a test map; the pure logic stays unit-
/// testable without `set_var`/`remove_var` side effects.
///
/// Collects every missing file before returning so an operator who
/// staged 4 of 6 files sees all 2 remaining names in one error, not
/// one failure at a time.
pub(crate) fn build_staging_sources_using<F>(
    out_dir: &Path,
    env_lookup: F,
) -> Result<EspStagingSources, SourceResolutionError>
where
    F: Fn(&str) -> Option<String>,
{
    let shim_x64 = resolve_one(
        out_dir,
        env_lookup(ENV_SHIM_SRC).as_deref(),
        DEFAULT_SHIM_FILENAME,
    );
    let grub_x64 = resolve_one(
        out_dir,
        env_lookup(ENV_GRUB_SRC).as_deref(),
        DEFAULT_GRUB_FILENAME,
    );
    let mm_x64 = resolve_one(
        out_dir,
        env_lookup(ENV_MM_SRC).as_deref(),
        DEFAULT_MM_FILENAME,
    );
    let grub_cfg = resolve_one(
        out_dir,
        env_lookup(ENV_GRUB_CFG).as_deref(),
        DEFAULT_GRUB_CFG_FILENAME,
    );
    let vmlinuz = resolve_one(
        out_dir,
        env_lookup(ENV_KERNEL_SRC).as_deref(),
        DEFAULT_KERNEL_FILENAME,
    );
    let initramfs = resolve_one(
        out_dir,
        env_lookup(ENV_INITRD_SRC).as_deref(),
        DEFAULT_INITRD_FILENAME,
    );

    let checks: [(&str, &Path, &str); 6] = [
        (ENV_SHIM_SRC, &shim_x64, DEFAULT_SHIM_FILENAME),
        (ENV_GRUB_SRC, &grub_x64, DEFAULT_GRUB_FILENAME),
        (ENV_MM_SRC, &mm_x64, DEFAULT_MM_FILENAME),
        (ENV_GRUB_CFG, &grub_cfg, DEFAULT_GRUB_CFG_FILENAME),
        (ENV_KERNEL_SRC, &vmlinuz, DEFAULT_KERNEL_FILENAME),
        (ENV_INITRD_SRC, &initramfs, DEFAULT_INITRD_FILENAME),
    ];

    let mut missing = Vec::new();
    for (env_var, p, default_filename) in &checks {
        if !p.is_file() {
            missing.push(MissingFile {
                env_var,
                resolved_path: (*p).to_path_buf(),
                default_filename,
            });
        }
    }

    if !missing.is_empty() {
        return Err(SourceResolutionError::MissingFiles(missing));
    }

    Ok(EspStagingSources {
        shim_x64,
        grub_x64,
        mm_x64,
        grub_cfg,
        vmlinuz,
        initramfs,
    })
}

/// Production form — queries the process's real environment via
/// [`std::env::var`]. Prefer [`build_staging_sources_using`] in tests.
pub(crate) fn build_staging_sources(
    out_dir: &Path,
) -> Result<EspStagingSources, SourceResolutionError> {
    build_staging_sources_using(out_dir, |name| std::env::var(name).ok())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// Build a closure that returns env values from a `HashMap` —
    /// the default lookup for most tests. Keeps each test's env-var
    /// shape inline with its assertions.
    fn env_from(map: HashMap<&'static str, String>) -> impl Fn(&str) -> Option<String> {
        move |k| map.get(k).cloned()
    }

    fn no_env() -> impl Fn(&str) -> Option<String> {
        |_| None
    }

    /// Drop all 6 default-named files into `dir` so `is_file()` passes
    /// when no env override is set.
    fn populate_defaults(dir: &Path) {
        for (_, filename) in ENV_DEFAULT_PAIRS {
            let p = dir.join(filename);
            std::fs::write(&p, b"stub").unwrap();
        }
    }

    #[test]
    fn resolve_one_uses_default_when_no_override() {
        let p = resolve_one(Path::new("/out"), None, "shim.efi");
        assert_eq!(p, PathBuf::from("/out/shim.efi"));
    }

    #[test]
    fn resolve_one_uses_override_when_set() {
        let p = resolve_one(Path::new("/out"), Some("/etc/aegis/shim.efi"), "shim.efi");
        assert_eq!(p, PathBuf::from("/etc/aegis/shim.efi"));
    }

    #[test]
    fn resolve_one_treats_empty_override_as_unset() {
        let p = resolve_one(Path::new("/out"), Some(""), "shim.efi");
        assert_eq!(p, PathBuf::from("/out/shim.efi"));
    }

    #[test]
    fn build_all_defaults_succeeds_when_files_present() {
        let dir = tempfile::tempdir().unwrap();
        populate_defaults(dir.path());

        let srcs = build_staging_sources_using(dir.path(), no_env()).expect("all present");
        assert_eq!(srcs.shim_x64, dir.path().join(DEFAULT_SHIM_FILENAME));
        assert_eq!(srcs.grub_x64, dir.path().join(DEFAULT_GRUB_FILENAME));
        assert_eq!(srcs.mm_x64, dir.path().join(DEFAULT_MM_FILENAME));
        assert_eq!(srcs.grub_cfg, dir.path().join(DEFAULT_GRUB_CFG_FILENAME));
        assert_eq!(srcs.vmlinuz, dir.path().join(DEFAULT_KERNEL_FILENAME));
        assert_eq!(srcs.initramfs, dir.path().join(DEFAULT_INITRD_FILENAME));
    }

    #[test]
    fn build_reports_all_missing_files_not_just_first() {
        let dir = tempfile::tempdir().unwrap();
        // Only stage shim + grub — 4 remain missing.
        std::fs::write(dir.path().join(DEFAULT_SHIM_FILENAME), b"stub").unwrap();
        std::fs::write(dir.path().join(DEFAULT_GRUB_FILENAME), b"stub").unwrap();

        let err = build_staging_sources_using(dir.path(), no_env()).unwrap_err();
        match err {
            SourceResolutionError::MissingFiles(v) => {
                assert_eq!(v.len(), 4);
                // mmx64, grub.cfg, vmlinuz, initramfs.cpio.gz missing.
                let names: Vec<&str> = v.iter().map(|m| m.default_filename).collect();
                assert!(names.contains(&DEFAULT_MM_FILENAME));
                assert!(names.contains(&DEFAULT_GRUB_CFG_FILENAME));
                assert!(names.contains(&DEFAULT_KERNEL_FILENAME));
                assert!(names.contains(&DEFAULT_INITRD_FILENAME));
            }
        }
    }

    #[test]
    fn build_uses_env_override_for_single_file() {
        let default_dir = tempfile::tempdir().unwrap();
        let override_dir = tempfile::tempdir().unwrap();
        // Default dir has everything except the kernel.
        for (_, filename) in ENV_DEFAULT_PAIRS {
            if filename != DEFAULT_KERNEL_FILENAME {
                std::fs::write(default_dir.path().join(filename), b"stub").unwrap();
            }
        }
        // Override dir holds the kernel at a custom name.
        let custom_kernel = override_dir.path().join("vmlinuz-6.1.0-special");
        std::fs::write(&custom_kernel, b"stub").unwrap();

        let mut map = HashMap::new();
        map.insert(ENV_KERNEL_SRC, custom_kernel.to_string_lossy().into_owned());

        let srcs = build_staging_sources_using(default_dir.path(), env_from(map))
            .expect("override should supply the missing kernel");
        assert_eq!(srcs.vmlinuz, custom_kernel);
        // Non-overridden files still resolve via default_dir.
        assert_eq!(
            srcs.shim_x64,
            default_dir.path().join(DEFAULT_SHIM_FILENAME)
        );
    }

    #[test]
    fn build_reports_missing_when_env_override_points_nowhere() {
        let dir = tempfile::tempdir().unwrap();
        populate_defaults(dir.path());
        let mut map = HashMap::new();
        map.insert(ENV_SHIM_SRC, "/nonexistent/shim.efi".to_string());

        let err = build_staging_sources_using(dir.path(), env_from(map)).unwrap_err();
        match err {
            SourceResolutionError::MissingFiles(v) => {
                assert_eq!(v.len(), 1);
                assert_eq!(v[0].env_var, ENV_SHIM_SRC);
                assert_eq!(v[0].resolved_path, PathBuf::from("/nonexistent/shim.efi"));
            }
        }
    }

    #[test]
    fn build_treats_empty_env_override_as_unset() {
        // An operator who sets `AEGIS_SHIM_SRC=` (empty) gets the default.
        let dir = tempfile::tempdir().unwrap();
        populate_defaults(dir.path());
        let mut map = HashMap::new();
        map.insert(ENV_SHIM_SRC, String::new());

        let srcs =
            build_staging_sources_using(dir.path(), env_from(map)).expect("defaults still work");
        assert_eq!(srcs.shim_x64, dir.path().join(DEFAULT_SHIM_FILENAME));
    }

    #[test]
    fn error_display_lists_every_missing_file_and_closes_with_remediation() {
        let err = SourceResolutionError::MissingFiles(vec![
            MissingFile {
                env_var: ENV_SHIM_SRC,
                resolved_path: PathBuf::from("/out/shimx64.efi.signed"),
                default_filename: DEFAULT_SHIM_FILENAME,
            },
            MissingFile {
                env_var: ENV_KERNEL_SRC,
                resolved_path: PathBuf::from("/out/vmlinuz"),
                default_filename: DEFAULT_KERNEL_FILENAME,
            },
        ]);
        let s = format!("{err}");
        assert!(s.contains("2 source file(s) missing"));
        assert!(s.contains(DEFAULT_SHIM_FILENAME));
        assert!(s.contains(DEFAULT_KERNEL_FILENAME));
        assert!(s.contains(ENV_SHIM_SRC));
        assert!(s.contains(ENV_KERNEL_SRC));
        // Remediation hint at the tail — tells the operator what to do.
        assert!(s.contains("mkusb.sh"));
    }

    #[test]
    fn env_default_pairs_has_all_six_in_staging_order() {
        // Tight contract: the 6 pairs must match
        // [`EspFile::ALL`] ordering exactly — that's what
        // build_staging_sources walks.
        let names: Vec<&str> = ENV_DEFAULT_PAIRS.iter().map(|(_, f)| *f).collect();
        assert_eq!(
            names,
            vec![
                DEFAULT_SHIM_FILENAME,
                DEFAULT_GRUB_FILENAME,
                DEFAULT_MM_FILENAME,
                DEFAULT_GRUB_CFG_FILENAME,
                DEFAULT_KERNEL_FILENAME,
                DEFAULT_INITRD_FILENAME,
            ]
        );
    }
}
