# aegis-hwsim persona schema (draft)

**Status:** Draft. Tracking under [#226](https://github.com/aegis-boot/aegis-boot/issues/226). The sibling repo `aegis-hwsim` doesn't exist yet — this doc exists in aegis-boot so the schema can be reviewed before the sibling repo is spun up.

**Audience:** reviewers of the persona format; future authors adding hardware personas to the library.

## What a persona is

A YAML fixture describing one shipping hardware configuration in enough detail to drive a QEMU + OVMF + swtpm invocation that boots through aegis-boot's rescue chain the way that machine would. The fixture covers the **Linux-visible** surface: DMI strings the kernel exposes at `/sys/class/dmi/id/`, Secure Boot posture, TPM presence, kernel lockdown mode. It does **not** attempt to simulate vendor-specific UEFI UI (Lenovo's blue-screen MOK Manager, Dell's F12 boot menu look, HP Fast Boot timing) — those remain real-hardware-only.

## Example persona

```yaml
# personas/lenovo-thinkpad-x1-carbon-gen11.yaml
schema_version: 1
id: lenovo-thinkpad-x1-carbon-gen11
vendor: LENOVO
display_name: "Lenovo ThinkPad X1 Carbon Gen 11"
year: 2023
source:
  # Every persona must cite where its DMI + firmware values came from.
  # Allowed citation types:
  #   - community_report: points at a closed hardware-report GitHub issue
  #   - lvfs_catalog:     points at the fwupd/LVFS firmware-archive URL
  #   - vendor_docs:      points at a vendor-published spec sheet
  kind: community_report
  ref: "github.com/aegis-boot/aegis-boot/issues/307"
  captured_at: 2026-03-14

dmi:
  # Fields mapped 1:1 to /sys/class/dmi/id/<name> — QEMU injects via
  # `-smbios type=1,manufacturer=...,product=...,version=...` and
  # `-smbios type=0,vendor=...,version=...,date=...` et al.
  sys_vendor: LENOVO
  product_name: 21HMCTO1WW               # the SKU code Lenovo exposes
  product_version: "ThinkPad X1 Carbon Gen 11"  # the friendly name
  bios_vendor: LENOVO
  bios_version: "N3HET70W (1.50)"
  bios_date: 01/15/2024
  # Optional; only fill when the vendor populates them with non-
  # placeholder strings. `doctor` already filters "To Be Filled By
  # O.E.M." and friends.
  board_name: 21HMCTO1WW
  chassis_type: "10"                      # SMBIOS chassis code (10 = notebook)

secure_boot:
  # Which OVMF variant to boot under, which VARs file to seed.
  ovmf_variant: ms_enrolled               # one of: ms_enrolled, custom_pk, setup_mode, disabled
  # Optional — when ovmf_variant == custom_pk, point at the
  # hwsim-generated test PK/KEK/db keyring (never a production key,
  # CN must carry TEST_ONLY_NOT_FOR_PRODUCTION per security review).
  custom_keyring: null

tpm:
  # none | 1.2 | 2.0. Drives the swtpm invocation + QEMU tpm-tis device.
  version: "2.0"
  # Optional manufacturer/vendor fields for TPM2 capabilities. Most
  # sticks don't care about these; set when testing TPM-specific paths.
  manufacturer: IFX                        # Infineon
  firmware_version: "7.2.3.1"

kernel:
  # aegis-boot's initramfs ships a kernel; the persona can pin a
  # specific lockdown mode to test the rescue-tui's SB-enforcing
  # diagnostic paths. "inherit" means use whatever the initramfs ships.
  lockdown: inherit                       # inherit | none | integrity | confidentiality

quirks:
  # Advisory list of vendor-specific behaviors hwsim can't simulate.
  # Exposed to the scenario runner so test reports can annotate
  # "this would work here but not on real hardware because X".
  #
  # Each quirk is a short tag + long-form description. Tags are
  # free-form but should match `/^[a-z0-9][a-z0-9-]*[a-z0-9]$/` so
  # they're grep-able.
  - tag: fast-boot-default-on
    description: "Ships with Fast Boot enabled; must be disabled in BIOS for USB enumeration to be reliable under aegis-boot flash."
  - tag: boot-key-f12
    description: "Firmware boot-menu key is F12 (not the vendor default F1/F2)."
  - tag: mok-manager-blue-background
    description: "MOK Manager renders blue-on-black, matching the text in the #202 walkthrough."

scenarios:
  # Opt-in / opt-out per scenario. Scenarios live in the hwsim runner,
  # not the persona. A persona can opt out of scenarios that are
  # known-broken on real hardware (rare — prefer fixing the scenario
  # over skipping it per-persona).
  signed-boot-ubuntu: run
  mok-enroll-alpine: run
  kexec-refuses-unsigned: run
  attestation-roundtrip: run
```

## Field-by-field

| Field | Type | Required | Purpose |
|-------|------|----------|---------|
| `schema_version` | integer | yes | Pins the parser to an exact schema. Mismatched parsers refuse to load. Bumped on breaking changes; additive fields don't bump it. |
| `id` | string, kebab-case | yes | Stable identifier used as the YAML filename (without `.yaml`) and as the CLI arg to `aegis-hwsim run <id>`. |
| `vendor` | string | yes | SMBIOS sys_vendor value verbatim as the vendor ships it (preserve case). |
| `display_name` | string | yes | Human-readable name shown in the coverage-grid output. |
| `year` | integer | recommended | Year the SKU first shipped. Useful for sorting + understanding firmware-era context. |
| `source` | object | yes | Provenance record — where the DMI + firmware values came from. See "Source citation" below. |
| `dmi.*` | various | yes | Mapped 1:1 to `/sys/class/dmi/id/<field>`. QEMU's `-smbios` flags inject these. |
| `secure_boot.ovmf_variant` | enum | yes | Which OVMF firmware to boot under. |
| `secure_boot.custom_keyring` | path or null | when variant == custom_pk | Path to the hwsim-generated test keyring. |
| `tpm.version` | enum | yes | `none` / `1.2` / `2.0`. |
| `tpm.manufacturer` | string | optional | TPM2 vendor code (IFX, NTC, AMD, STM, INTC, ...). |
| `tpm.firmware_version` | string | optional | For testing TPM2-FW-bug-specific paths. |
| `kernel.lockdown` | enum | yes | `inherit` / `none` / `integrity` / `confidentiality`. |
| `quirks[]` | array of {tag, description} | optional | Real-world quirks hwsim can't simulate. Informational. |
| `scenarios.<name>` | enum | optional | `run` (default) / `skip`. Per-scenario opt-out. |

## Source citation

Every persona must cite its origin so reviewers can trace any field back to a primary source. Three citation kinds:

- `community_report` — points at a closed `hardware-report` GitHub issue (filed via the `.github/ISSUE_TEMPLATE/hardware-report.yml` form or `aegis-boot compat --submit`). Reserved for personas derived from a real operator running the full flash → boot → kexec chain.
- `lvfs_catalog` — points at the fwupd/LVFS archive URL for the firmware version. Reserved for DMI fields we can verify against the vendor's published firmware metadata.
- `vendor_docs` — points at a vendor-published spec sheet (Lenovo PSREF, Dell Product Support, Framework Marketplace). Lowest-confidence citation; only use for fields the other two sources don't cover.

Unsourced personas are rejected at PR review. "Verified outcomes only" matches the existing `aegis-boot compat` DB policy.

## Validation

A persona is valid iff:

1. `schema_version` matches a supported version at the parser.
2. `id` matches the YAML filename (case-sensitive; stops `lenovo-thinkpad-x1.yaml` from shipping an `id: lenovo_thinkpad_x1` drift).
3. Every required field per the table above is populated with a non-placeholder value.
4. `secure_boot.ovmf_variant == custom_pk` implies `custom_keyring` is set and points at a path under `$AEGIS_HWSIM_ROOT/firmware/` (path-traversal guard).
5. Every `quirks[].tag` matches `^[a-z0-9][a-z0-9-]*[a-z0-9]$`.
6. `source.kind` is one of `community_report` / `lvfs_catalog` / `vendor_docs` and `source.ref` is a plain non-empty string.

## QEMU invocation shape (draft)

For the persona above, the runner would synthesize roughly:

```bash
qemu-system-x86_64 \
  -machine q35,smm=on \
  -global driver=cfi.pflash01,property=secure,value=on \
  -m 2048M \
  \
  # -- OVMF variant = ms_enrolled
  -drive "if=pflash,format=raw,unit=0,file=$OVMF_CODE_SECBOOT,readonly=on" \
  -drive "if=pflash,format=raw,unit=1,file=$VARS_COPY" \
  \
  # -- DMI injection (fields truncated for brevity)
  -smbios type=0,vendor=LENOVO,version="N3HET70W (1.50)",date=01/15/2024 \
  -smbios type=1,manufacturer=LENOVO,product=21HMCTO1WW,version="ThinkPad X1 Carbon Gen 11" \
  -smbios type=2,manufacturer=LENOVO,product=21HMCTO1WW \
  -smbios type=3,type=10 \
  \
  # -- TPM 2.0 via swtpm socket
  -chardev socket,id=chrtpm,path=$SWTPM_SOCK \
  -tpmdev emulator,id=tpm0,chardev=chrtpm \
  -device tpm-tis,tpmdev=tpm0 \
  \
  # -- Test stick (passed in by the scenario runner)
  -drive file=$TEST_STICK_IMG,if=none,id=usbstick,format=raw \
  -device qemu-xhci,id=xhci \
  -device usb-storage,bus=xhci.0,drive=usbstick \
  \
  -nographic -serial file:"$SERIAL_LOG"
```

All string values must be shell-escaped and passed via `Command::args([...])`, **not** concatenated into a shell string — see security constraint #1 in #226.

## Open questions

- Scenario schema (separate from persona schema) — deferred to Phase 2.
- How far to go on TPM2 firmware-version simulation. swtpm exposes some TPM2 capability fields but not all; real TPM2 firmware bugs may be non-reproducible.
- Whether to check personas against LVFS periodically so BIOS version drift is caught. Nice-to-have, not MVP.

## Non-goals (for this doc)

- Scenario YAML schema — scenarios are Rust code in the `scenarios/` directory, not YAML fixtures. A future doc will cover them if scenario reuse pressures emerge.
- Runner architecture — belongs in its own design doc under `aegis-hwsim/` once the repo exists.
- Coverage-grid output format — belongs with the runner doc.

## Review checklist before the sibling repo spawns

- [ ] Schema validates against the YAML above (YAML lints, JSONSchema file written, at least two contributors comment)
- [ ] Security constraints 1-4 from #226 reflected in the validation rules
- [ ] Source-citation policy matches `aegis-boot compat` DB's "verified outcomes only" stance
- [ ] At least one real operator reviews the `quirks[]` format for usability
