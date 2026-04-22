# Security Policy

Aegis-Boot is security-critical infrastructure (UEFI Secure Boot orchestration).
Treat all findings as potentially high-impact until triaged.

## Reporting a Vulnerability

**Do not open public GitHub issues for security findings.**

Use GitHub's private vulnerability reporting:
https://github.com/aegis-boot/aegis-boot/security/advisories/new

Include:
- Affected component (crate / script / Dockerfile / workflow)
- Reproduction steps or proof-of-concept
- Impact assessment (boot-chain compromise, key exposure, parser crash, etc.)
- Suggested remediation if known

Expect an acknowledgement within 7 days. Coordinated disclosure preferred.

## Scope

In scope:
- UEFI Secure Boot key handling (PK/KEK/db/dbx)
- MOK enrollment logic
- SBAT revocation semantics
- ISO parser (memory safety, path traversal, resource exhaustion)
- Reproducible build pipeline (toolchain pinning, supply-chain integrity)

Out of scope:
- Issues in upstream dependencies (report to their maintainers; we will patch when advisory lands)
- Theoretical attacks requiring prior root / physical access beyond the stated threat model (see [THREAT_MODEL.md](./THREAT_MODEL.md))

## Threat Model

See [THREAT_MODEL.md](./THREAT_MODEL.md) for the full threat model, trust boundaries, and explicit non-goals.
