// SPDX-License-Identifier: MIT OR Apache-2.0

//! Operator-curated metadata that travels alongside an ISO.
//!
//! The rescue-TUI menu shows ISO filenames, which are dense and
//! cryptic at 2 AM (`debian-12.5.0-amd64-netinst.iso`). A sidecar
//! TOML at `<iso>.aegis.toml` carries human-curated `display_name`,
//! `description`, `version`, `category`, and a "last verified" record
//! so the menu can render `Network-install Debian 12 (verified
//! 2026-02 on T440p, OK)` instead.
//!
//! Sidecars are **not signed** by default. Tampering with one can
//! change display strings but cannot affect what boots — boot
//! decisions still consume the sha256-attested manifest, not the
//! sidecar. Future enhancement (`aegis-boot sign --include-sidecars`)
//! could fold them into the signed manifest for fleet operators.
//!
//! # File location
//!
//! For an ISO at `/mnt/aegis-isos/debian.iso`, the sidecar is
//! `/mnt/aegis-isos/debian.iso.aegis.toml`. The double-extension
//! convention matches existing sidecars (`.iso.sha256`,
//! `.iso.minisig`).
//!
//! # Schema versioning
//!
//! Every field is optional + has a `#[serde(default)]`. Adding new
//! fields is semver-minor; renaming or removing fields is breaking.
//! Tracks #246.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Operator-curated metadata for one ISO.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct IsoSidecar {
    /// Human-readable display name (e.g. `"Network-install Debian 12"`).
    pub display_name: Option<String>,
    /// One-line description shown beneath the display name.
    pub description: Option<String>,
    /// Distro version string (e.g. `"12.5.0"`, `"24.04 LTS"`).
    pub version: Option<String>,
    /// Free-text category. Conventionally one of `install`, `live`,
    /// `rescue`, `firmware`, `other` — but operators may define their
    /// own.
    pub category: Option<String>,
    /// Date this ISO was last verified to boot (YYYY-MM-DD).
    pub last_verified_at: Option<String>,
    /// Hardware persona the ISO was last verified against (e.g.
    /// `"lenovo-thinkpad-t440p-tpm12"`). Free-text — the sidecar is
    /// operator-local.
    pub last_verified_on: Option<String>,
    /// Free-text operator notes (firmware-specific quirks, etc.).
    pub notes: Option<String>,
}

impl IsoSidecar {
    /// Whether the sidecar carries any populated fields.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.display_name.is_none()
            && self.description.is_none()
            && self.version.is_none()
            && self.category.is_none()
            && self.last_verified_at.is_none()
            && self.last_verified_on.is_none()
            && self.notes.is_none()
    }
}

/// Compute the canonical sidecar path for an ISO.
///
/// `<iso>.aegis.toml` — kept consistent with the `<iso>.sha256` and
/// `<iso>.minisig` double-extension convention.
#[must_use]
pub fn sidecar_path_for(iso_path: &Path) -> PathBuf {
    let mut s = iso_path.as_os_str().to_owned();
    s.push(".aegis.toml");
    PathBuf::from(s)
}

/// Errors raised while loading or writing a sidecar.
#[derive(Debug, thiserror::Error)]
pub enum SidecarError {
    /// I/O error reading or writing the sidecar file.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// TOML parser rejected the file body.
    #[error("invalid toml in {path}: {source}")]
    InvalidToml {
        /// Path of the file that failed to parse.
        path: PathBuf,
        /// Underlying parser error.
        #[source]
        source: toml::de::Error,
    },
    /// TOML serializer rejected the in-memory struct.
    #[error("toml serialize: {0}")]
    SerializeToml(#[from] toml::ser::Error),
}

/// Load an ISO's sidecar metadata, if a `<iso>.aegis.toml` file
/// exists at the canonical path.
///
/// Returns:
/// - `Ok(Some(sidecar))` when the file exists and parses cleanly.
/// - `Ok(None)` when no sidecar file is present (the common case).
/// - `Err(SidecarError::Io)` on any I/O error other than `NotFound`.
/// - `Err(SidecarError::InvalidToml)` when the file exists but the
///   TOML body is malformed.
///
/// # Errors
///
/// See variants above.
pub fn load_sidecar(iso_path: &Path) -> Result<Option<IsoSidecar>, SidecarError> {
    let path = sidecar_path_for(iso_path);
    let body = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(SidecarError::Io(e)),
    };
    let sidecar: IsoSidecar =
        toml::from_str(&body).map_err(|source| SidecarError::InvalidToml {
            path: path.clone(),
            source,
        })?;
    Ok(Some(sidecar))
}

/// Serialize an `IsoSidecar` to TOML. Useful for `aegis-boot add
/// --description ...` which writes a sidecar at copy time.
///
/// # Errors
///
/// Returns `SidecarError::SerializeToml` if serde rejects the value.
pub fn to_toml(sidecar: &IsoSidecar) -> Result<String, SidecarError> {
    Ok(toml::to_string_pretty(sidecar)?)
}

/// Write an `IsoSidecar` to disk at the canonical sidecar path for
/// `iso_path`. Overwrites any existing sidecar at that path.
///
/// # Errors
///
/// Returns `SidecarError::Io` on any write failure or
/// `SidecarError::SerializeToml` if serde rejects the value.
pub fn write_sidecar(iso_path: &Path, sidecar: &IsoSidecar) -> Result<PathBuf, SidecarError> {
    let path = sidecar_path_for(iso_path);
    let body = to_toml(sidecar)?;
    fs::write(&path, body)?;
    Ok(path)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn sidecar_path_appends_double_extension() {
        let p = sidecar_path_for(Path::new("/mnt/aegis-isos/debian.iso"));
        assert_eq!(p, PathBuf::from("/mnt/aegis-isos/debian.iso.aegis.toml"));
    }

    #[test]
    fn sidecar_path_works_with_no_extension() {
        let p = sidecar_path_for(Path::new("/tmp/oddly-named-image"));
        assert_eq!(p, PathBuf::from("/tmp/oddly-named-image.aegis.toml"));
    }

    #[test]
    fn load_returns_none_when_no_sidecar_present() {
        let dir = tempdir().unwrap();
        let iso = dir.path().join("nothing.iso");
        let result = load_sidecar(&iso).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn load_returns_populated_sidecar_when_present() {
        let dir = tempdir().unwrap();
        let iso = dir.path().join("debian.iso");
        let sidecar_path = sidecar_path_for(&iso);
        let body = r#"display_name = "Network-install Debian 12"
description = "Recommended for headless servers"
version = "12.5.0"
category = "install"
last_verified_at = "2026-02-18"
last_verified_on = "lenovo-thinkpad-t440p-tpm12"
notes = "Boots cleanly under Secure Boot via shim."
"#;
        fs::write(&sidecar_path, body).unwrap();

        let sidecar = load_sidecar(&iso).unwrap().unwrap();
        assert_eq!(
            sidecar.display_name.as_deref(),
            Some("Network-install Debian 12")
        );
        assert_eq!(sidecar.version.as_deref(), Some("12.5.0"));
        assert_eq!(sidecar.category.as_deref(), Some("install"));
        assert_eq!(sidecar.last_verified_at.as_deref(), Some("2026-02-18"));
        assert!(!sidecar.is_empty());
    }

    #[test]
    fn load_returns_empty_sidecar_when_file_is_blank() {
        let dir = tempdir().unwrap();
        let iso = dir.path().join("blank.iso");
        fs::write(sidecar_path_for(&iso), "").unwrap();

        let sidecar = load_sidecar(&iso).unwrap().unwrap();
        assert!(sidecar.is_empty());
    }

    #[test]
    fn load_accepts_partial_sidecar_with_serde_defaults() {
        let dir = tempdir().unwrap();
        let iso = dir.path().join("partial.iso");
        let body = "display_name = \"Just a name\"\n";
        fs::write(sidecar_path_for(&iso), body).unwrap();

        let sidecar = load_sidecar(&iso).unwrap().unwrap();
        assert_eq!(sidecar.display_name.as_deref(), Some("Just a name"));
        assert!(sidecar.description.is_none());
        assert!(sidecar.version.is_none());
    }

    #[test]
    fn load_rejects_malformed_toml() {
        let dir = tempdir().unwrap();
        let iso = dir.path().join("bad.iso");
        fs::write(sidecar_path_for(&iso), "this is not = valid = toml\n").unwrap();

        match load_sidecar(&iso) {
            Err(SidecarError::InvalidToml { path, .. }) => {
                assert_eq!(path, sidecar_path_for(&iso));
            }
            other => panic!("expected InvalidToml, got {other:?}"),
        }
    }

    #[test]
    fn load_rejects_unknown_top_level_keys_with_default_serde_strict_mode() {
        // Default serde TOML accepts unknown keys (forward-compat). This
        // test pins that behavior — adding new fields in future versions
        // must remain backward-compatible.
        let dir = tempdir().unwrap();
        let iso = dir.path().join("future.iso");
        let body = "display_name = \"x\"\nfuture_field = 42\n";
        fs::write(sidecar_path_for(&iso), body).unwrap();
        let sidecar = load_sidecar(&iso).unwrap().unwrap();
        assert_eq!(sidecar.display_name.as_deref(), Some("x"));
    }

    #[test]
    fn write_then_load_roundtrips_full_sidecar() {
        let dir = tempdir().unwrap();
        let iso = dir.path().join("roundtrip.iso");
        let original = IsoSidecar {
            display_name: Some("Network-install Debian 12".into()),
            description: Some("Recommended for headless servers".into()),
            version: Some("12.5.0".into()),
            category: Some("install".into()),
            last_verified_at: Some("2026-02-18".into()),
            last_verified_on: Some("framework-laptop-12gen".into()),
            notes: Some("Boots cleanly under Secure Boot via shim.".into()),
        };

        let written_path = write_sidecar(&iso, &original).unwrap();
        assert_eq!(written_path, sidecar_path_for(&iso));

        let loaded = load_sidecar(&iso).unwrap().unwrap();
        assert_eq!(loaded, original);
    }

    #[test]
    fn write_then_load_roundtrips_empty_sidecar() {
        let dir = tempdir().unwrap();
        let iso = dir.path().join("empty.iso");
        let original = IsoSidecar::default();

        write_sidecar(&iso, &original).unwrap();
        let loaded = load_sidecar(&iso).unwrap().unwrap();
        assert_eq!(loaded, original);
        assert!(loaded.is_empty());
    }

    #[test]
    fn is_empty_default_sidecar() {
        let s = IsoSidecar::default();
        assert!(s.is_empty());
    }

    #[test]
    fn is_empty_false_when_any_field_populated() {
        let s = IsoSidecar {
            description: Some("just one field".into()),
            ..Default::default()
        };
        assert!(!s.is_empty());
    }

    #[test]
    fn to_toml_omits_none_fields() {
        let s = IsoSidecar {
            display_name: Some("name".into()),
            ..Default::default()
        };
        let out = to_toml(&s).unwrap();
        assert!(out.contains("display_name = \"name\""), "got: {out}");
        // Optional none-fields should be omitted by serde's default skip.
        // toml-rs default doesn't skip None — we serialize as nothing
        // because Option<String> renders as missing key when None.
        assert!(!out.contains("description"), "got: {out}");
        assert!(!out.contains("version"), "got: {out}");
    }
}
