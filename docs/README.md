# Documentation index

| Audience | Doc | What's in it |
|---|---|---|
| **Operator (new)** | [`HOW_IT_WORKS.md`](./HOW_IT_WORKS.md) | "aegis-boot in 5 minutes" — what it does, why other tools require disabling Secure Boot, the trust chain |
| **Operator (new)** | [`TOUR.md`](./TOUR.md) | First-time procedural walkthrough: doctor → init → fetch → add → boot. ~10 minutes hands-on |
| **Operator** | [`INSTALL.md`](./INSTALL.md) | End-to-end: flash → add ISOs → boot → select. The first thing to read. |
| **Operator** | [`CLI.md`](./CLI.md) | `aegis-boot` CLI reference — `flash`, `list`, `add` subcommand details |
| **Operator** | [`TROUBLESHOOTING.md`](./TROUBLESHOOTING.md) | Common errors and fixes (errno 61, won't-boot, mount issues, MOK pitfalls) |
| **Operator** | [`UNSIGNED_KERNEL.md`](./UNSIGNED_KERNEL.md) | What to do when an ISO ships an unsigned kernel (Alpine, Arch, NixOS) |
| **Operator** | [`USB_LAYOUT.md`](./USB_LAYOUT.md) | GPT + ESP + AEGIS_ISOS scheme; manual loop-mount workflow; exFAT (default) / FAT32 / ext4 trade-offs |
| **Operator** | [`HARDWARE_COMPAT.md`](./HARDWARE_COMPAT.md) | Community-curated table of validated machines; how to submit your own report |
| **Operator** | [`CATALOG_POLICY.md`](./CATALOG_POLICY.md) | What gets into the `aegis-boot recommend` catalog; how to propose additions |
| **Operator** | [`compatibility/iso-matrix.md`](./compatibility/iso-matrix.md) | Per-distro kexec compatibility — what works, what surfaces a quirk |
| **Developer** | [`ARCHITECTURE.md`](./ARCHITECTURE.md) | One-page mental model — boot chain, crate dependencies, trust boundaries |
| **Developer** | [`LOCAL_TESTING.md`](./LOCAL_TESTING.md) | 8-stage local CI equivalent; `qemu-loaded-stick.sh --attach` modes; iteration recipes |
| **Developer** | [`../BUILDING.md`](../BUILDING.md) | Reproducible build setup (Docker `Dockerfile.locked` + Nix `flake.nix`) |
| **Developer** | [`../scripts/README.md`](../scripts/README.md) | What each script does and when to run it |
| **Architect** | [`adr/`](./adr/) | Architecture Decision Records |
| **Architect** | [`adr/0001-runtime-architecture.md`](./adr/0001-runtime-architecture.md) | Why "signed Linux rescue + ratatui + kexec" (Option B) over EDK II / dracut |
| **Architect** | [`architecture/KEY_MANAGEMENT.md`](./architecture/KEY_MANAGEMENT.md) | Cosign keyless + minisign Ed25519 signing surfaces; key-rotation playbook |
| **Architect** | [`architecture/LAST_BOOTED_PERSISTENCE.md`](./architecture/LAST_BOOTED_PERSISTENCE.md) | How rescue-tui persists the last-booted ISO to `/var/lib/aegis-boot` |
| **Architect** | [`design/aegis-hwsim-persona-schema.md`](./design/aegis-hwsim-persona-schema.md) | Hardware-simulation persona schema (QEMU + libvirt test personas) |
| **Security reviewer** | [`../SECURITY.md`](../SECURITY.md) | Vulnerability reporting (private path) |
| **Security reviewer** | [`../THREAT_MODEL.md`](../THREAT_MODEL.md) | UEFI SB threat model — PK/KEK/MOK/SBAT, kexec chain of trust |
| **Maintainer** | [`content-audit.md`](./content-audit.md) | Log of doc-accuracy audits + cadence |
| **Maintainer** | [`RELEASE_CRATES.md`](./RELEASE_CRATES.md) | Per-crate publish playbook (iso-parser, kexec-loader) |
| **Maintainer** | [`RELEASE_NOTES_FOOTER.md`](./RELEASE_NOTES_FOOTER.md) | Canonical cosign `verify-blob` recipe included in every release |
| **Maintainer** | [`governance/ORG_MIGRATION_PLAN.md`](./governance/ORG_MIGRATION_PLAN.md) | Plan for migrating to a dedicated `aegis-boot` GitHub org |
| **Maintainer** | [`validation/REAL_HARDWARE_REPORT_132.md`](./validation/REAL_HARDWARE_REPORT_132.md) | Real-hardware validation report (#132) — attached USB under libvirt |
| **Contributor** | [`../CONTRIBUTING.md`](../CONTRIBUTING.md) | Workflow, commit style, PR checklist |
| **Contributor** | [`../CODE_OF_CONDUCT.md`](../CODE_OF_CONDUCT.md) | Contributor Covenant 2.1 |
| Everyone | [`../CHANGELOG.md`](../CHANGELOG.md) | Per-release notes, what shipped and when |
| Everyone | [`../README.md`](../README.md) | Project overview, quickstart, status |

## Status pages

- **Releases:** https://github.com/williamzujkowski/aegis-boot/releases
- **Roadmap:** [`../ROADMAP.md`](../ROADMAP.md)
- **Open epics / issues:** https://github.com/williamzujkowski/aegis-boot/issues
