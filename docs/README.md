# Documentation index

| Audience | Doc | What's in it |
|---|---|---|
| **Operator** | [`INSTALL.md`](./INSTALL.md) | End-to-end: flash → add ISOs → boot → select. The first thing to read. |
| **Operator** | [`CLI.md`](./CLI.md) | `aegis-boot` CLI reference — `flash`, `list`, `add` subcommand details |
| **Operator** | [`TROUBLESHOOTING.md`](./TROUBLESHOOTING.md) | Common errors and fixes (errno 61, won't-boot, mount issues, MOK pitfalls) |
| **Operator** | [`UNSIGNED_KERNEL.md`](./UNSIGNED_KERNEL.md) | What to do when an ISO ships an unsigned kernel (Alpine, Arch, NixOS) |
| **Operator** | [`USB_LAYOUT.md`](./USB_LAYOUT.md) | GPT + ESP + AEGIS_ISOS scheme; manual loop-mount workflow; FAT32 vs ext4 trade-off |
| **Operator** | [`compatibility/iso-matrix.md`](./compatibility/iso-matrix.md) | Per-distro kexec compatibility — what works, what surfaces a quirk |
| **Developer** | [`ARCHITECTURE.md`](./ARCHITECTURE.md) | One-page mental model — boot chain, crate dependencies, trust boundaries |
| **Developer** | [`LOCAL_TESTING.md`](./LOCAL_TESTING.md) | 8-stage local CI equivalent; `qemu-loaded-stick.sh --attach` modes; iteration recipes |
| **Developer** | [`../BUILDING.md`](../BUILDING.md) | Reproducible build setup (Docker `Dockerfile.locked` + Nix `flake.nix`) |
| **Developer** | [`../scripts/README.md`](../scripts/README.md) | What each script does and when to run it |
| **Architect** | [`adr/`](./adr/) | Architecture Decision Records |
| **Architect** | [`adr/0001-runtime-architecture.md`](./adr/0001-runtime-architecture.md) | Why "signed Linux rescue + ratatui + kexec" (Option B) over EDK II / dracut |
| **Security reviewer** | [`../SECURITY.md`](../SECURITY.md) | Vulnerability reporting (private path) |
| **Security reviewer** | [`../THREAT_MODEL.md`](../THREAT_MODEL.md) | UEFI SB threat model — PK/KEK/MOK/SBAT, kexec chain of trust |
| **Maintainer** | [`content-audit.md`](./content-audit.md) | Log of doc-accuracy audits + cadence |
| **Contributor** | [`../CONTRIBUTING.md`](../CONTRIBUTING.md) | Workflow, commit style, PR checklist |
| **Contributor** | [`../CODE_OF_CONDUCT.md`](../CODE_OF_CONDUCT.md) | Contributor Covenant 2.1 |
| Everyone | [`../CHANGELOG.md`](../CHANGELOG.md) | Per-release notes, what shipped and when |
| Everyone | [`../README.md`](../README.md) | Project overview, quickstart, status |

## Status pages

- **Releases:** https://github.com/williamzujkowski/aegis-boot/releases
- **Roadmap:** [`../ROADMAP.md`](../ROADMAP.md)
- **Open epics / issues:** https://github.com/williamzujkowski/aegis-boot/issues
