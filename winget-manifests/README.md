# winget-manifests/

Template winget manifests for publishing `aegis-boot` to the Windows Package
Manager (winget) community repo.

> **Status: scaffolding only.** Phase B of [#365] has not yet produced a
> Windows build. These templates exist so that once `aegis-boot.exe` is
> built (via the FSCTL flash path), the publishing workflow has a known
> target shape to substitute into.

## What this directory is

- **`templates/`** — schema-conformant template manifests with
  `{{PLACEHOLDER}}` tokens. These are the source of truth for what
  aegis-boot's winget entry will look like.
- This `README.md` — publishing process + known caveats.

## What this directory is NOT

- It is **not** a copy of the upstream manifest set. Published manifests
  live in [microsoft/winget-pkgs] under
  `manifests/a/AegisBootProject/aegis-boot/<version>/`. We do not
  vendor them here.
- It is **not** wired into CI yet. No workflow consumes these templates
  today. The auto-publish workflow is a separate future PR tracked in
  #365 Phase B2.

## Files

Three manifests are required per version, following the winget-pkgs 1.6.0
schema:

| File                                          | Manifest type   | Purpose                                                           |
| --------------------------------------------- | --------------- | ----------------------------------------------------------------- |
| `AegisBootProject.aegis-boot.yaml`            | `version`       | Version index — points at the locale + installer manifests.       |
| `AegisBootProject.aegis-boot.installer.yaml`  | `installer`     | Architecture, URL, SHA256, installer type, commands exposed.      |
| `AegisBootProject.aegis-boot.locale.en-US.yaml` | `defaultLocale` | Human-readable metadata: publisher, description, license, tags.   |

Upstream path convention:

```
manifests/a/AegisBootProject/aegis-boot/<VERSION>/
  AegisBootProject.aegis-boot.yaml
  AegisBootProject.aegis-boot.installer.yaml
  AegisBootProject.aegis-boot.locale.en-US.yaml
```

## Placeholders

Templates use `{{UPPERCASE}}` tokens so a future substitution step
(e.g. `sed -e 's/{{VERSION}}/0.17.0/g'` or a scripted renderer) can fill
them cleanly. Placeholders are wrapped in double quotes in the templates
(e.g. `PackageVersion: "{{VERSION}}"`) so that the files parse as valid
YAML before substitution — the `{{ … }}` sequence would otherwise be
interpreted as an inline flow-mapping. The sed pattern still matches
because the `{{TOKEN}}` substring is preserved inside the quotes.

Current tokens:

| Token                 | Filled with                                                      | Source                                   |
| --------------------- | ---------------------------------------------------------------- | ---------------------------------------- |
| `{{VERSION}}`         | Release version without a leading `v` (e.g. `0.17.0`)            | Cargo workspace version / git tag        |
| `{{INSTALLER_URL}}`   | Absolute URL to the `aegis-boot-<VERSION>-windows-x64.zip` asset | GitHub Release asset URL                 |
| `{{INSTALLER_SHA256}}` | Uppercase hex SHA-256 of the zip archive                         | Computed during the release job          |

## Distribution shape

- **`InstallerType: zip`** with `NestedInstallerType: portable` pointing
  at `aegis-boot.exe`. This lets the archive bundle license files
  (`LICENSE-MIT`, `LICENSE-APACHE`) alongside the binary while still
  exposing `aegis-boot.exe` on PATH via the portable-command alias.
- **`Scope: user`** — aegis-boot is a CLI tool. It does not need admin
  install; `winget install aegis-boot` should not prompt for UAC.
- **`Architecture: x64`** only, for now. Phase B targets
  `x86_64-pc-windows-msvc` first. When an ARM64 Windows build exists
  (future, likely Phase C), add a second entry under `Installers:` with
  `Architecture: arm64` and a matching SHA256 for the ARM zip.

## Publishing flow (planned)

This is the intended flow once Phase B produces a Windows build. It is
not implemented yet.

1. A release tag `vX.Y.Z` is pushed.
2. The release workflow builds `aegis-boot-X.Y.Z-windows-x64.zip`,
   uploads it to the GitHub Release, and records the zip's SHA-256.
3. A publishing step (tracked in #365 Phase B2) uses [wingetcreate] or an
   equivalent tool to:
   - Copy the three templates from `winget-manifests/templates/`.
   - Substitute `{{VERSION}}`, `{{INSTALLER_URL}}`, `{{INSTALLER_SHA256}}`.
   - Open a pull request against [microsoft/winget-pkgs] adding the
     manifests at `manifests/a/AegisBootProject/aegis-boot/X.Y.Z/`.
4. winget-pkgs CI runs the validation bot; maintainers merge once green.

## Validation caveats

- The winget-pkgs validation bot rejects manifests whose
  `InstallerUrl`, `PublisherUrl`, `PackageUrl`, `LicenseUrl`,
  `PublisherSupportUrl`, or `ReleaseNotesUrl` are unreachable. All
  placeholders and live URLs must resolve before submission; do not
  submit with `{{…}}` tokens unsubstituted.
- `InstallerSha256` must match the hash of the artifact at
  `InstallerUrl` exactly — a mismatch fails the validation bot and
  blocks merge.
- Schema version 1.6.0 is strict about unknown fields. Do not add
  fields that are not in the [1.6.0 installer schema] or
  [1.6.0 defaultLocale schema]. The validator will reject them.
- Updates for a new version are additive: submit a new directory at
  `manifests/a/AegisBootProject/aegis-boot/<NEW_VERSION>/` with all
  three manifests. Do not edit previously published version directories.

## Org transfer note

Publisher identity is **Aegis Boot Project** (the project entity, not
`williamzujkowski`). Once the GitHub org transfer to `aegis-boot/` lands
(see #365 "org move"), the URLs in
`AegisBootProject.aegis-boot.locale.en-US.yaml` (PublisherUrl,
PackageUrl, PublisherSupportUrl, LicenseUrl, ReleaseNotesUrl,
PrivacyUrl) will continue to be correct because they already point at
`github.com/aegis-boot/aegis-boot`. If the org name changes again,
update this directory in the same PR that performs the move so there is
no drift between published manifests and the upstream repo.

## References

- [microsoft/winget-pkgs] — community manifest repo.
- [winget-pkgs authoring guide] — how to add a new package.
- [1.6.0 version schema][1.6.0 version] / [1.6.0 installer schema] /
  [1.6.0 defaultLocale schema].
- [wingetcreate] — Microsoft's manifest-authoring helper tool.

[#365]: https://github.com/williamzujkowski/aegis-boot/issues/365
[microsoft/winget-pkgs]: https://github.com/microsoft/winget-pkgs
[winget-pkgs authoring guide]: https://github.com/microsoft/winget-pkgs/blob/master/doc/README.md#authoring-a-manifest
[1.6.0 version]: https://github.com/microsoft/winget-cli/blob/master/schemas/JSON/manifests/v1.6.0/manifest.version.1.6.0.json
[1.6.0 installer schema]: https://github.com/microsoft/winget-cli/blob/master/schemas/JSON/manifests/v1.6.0/manifest.installer.1.6.0.json
[1.6.0 defaultLocale schema]: https://github.com/microsoft/winget-cli/blob/master/schemas/JSON/manifests/v1.6.0/manifest.defaultLocale.1.6.0.json
[wingetcreate]: https://github.com/microsoft/winget-create
