# ADR 0002: Key Management for Signed Attestation + Bundle Trust Anchor

**Status:** PROPOSED
**Date:** 2026-04-20
**Tracking issue:** [#366](https://github.com/williamzujkowski/aegis-boot/issues/366)
**Consumers:** [#349](https://github.com/williamzujkowski/aegis-boot/issues/349) (signed attestation manifest), [#367](https://github.com/williamzujkowski/aegis-boot/issues/367) (cross-platform `--direct-install` trust anchor, Phase D of [#365](https://github.com/williamzujkowski/aegis-boot/issues/365))
**Supersedes:** portion of `crates/aegis-cli/src/attest.rs` header comment (lines 22–28) that defers signing to "epic #139"; this ADR claims the design.

> **Reviewer note.** This ADR is PROPOSED pending consensus. Objections and alternative votes belong on #366, not scattered in PR review comments — consolidate discussion there so the decision log stays single-threaded. The `Alternatives considered` section below names the three strongest contrarian paths explicitly; please answer them before proposing a fourth.

---

## 1. Context

Aegis-boot needs signed trust decisions in two adjacent-but-distinct places, and each one would invent its own key-management story if left to its own feature PR. Solving it twice would fork the trust chain.

### 1.1 Consumer A — #349: signed attestation manifest

Today, `aegis-boot flash` writes a JSON attestation manifest to `$XDG_DATA_HOME/aegis-boot/attestations/<disk-guid>-<ts>.json` (see `crates/aegis-cli/src/attest.rs:209-263`). The schema is stable (`schema_version: 1`, `aegis_wire_formats::Attestation`). It records: tool version, host kernel, Secure Boot state, target device + model + disk GUID, image SHA-256, per-ISO additions.

The manifest is currently **unsigned** (`attest.rs:22-28`). An operator who walks away with a USB stick and the manifest JSON has no cryptographic evidence that *this tool*, at *this version*, *on this host*, produced this manifest. A motivated attacker with write access to `$XDG_DATA_HOME/aegis-boot/attestations/` could forge a manifest matching any stick.

The forensics + fleet-inventory + audit-trail use-case the module header cites (lines 15–20) fails the sniff test without a signature. Consumer A needs: the tool signs the manifest on write; any verifier with the operator-facing trust anchor can check it offline, days or years later.

### 1.2 Consumer B — #367: `--direct-install` trust anchor on macOS/Windows

`--direct-install` on Linux today uses apt-installed `shim-signed` + `grub-efi-amd64-signed` + `linux-image-virtual` from Ubuntu's `secure-boot` archive (`.github/workflows/release.yml:57-60`). The trust anchor is Canonical's CA, inherited transitively.

macOS and Windows hosts (#365 Phase D) have no apt and no `shim-signed` package. To flash an aegis-boot stick cross-platform, the CLI must download the signed chain (shim / grub / kernel / initrd / grub.cfg) at runtime and verify the downloaded bundle against an embedded trust anchor *before* flashing. Rufus solves this with an embedded RSA pubkey + `DownloadSignedFile` (referenced in #367). Aegis-boot needs the equivalent.

The bundle itself is built from the same Ubuntu `secure-boot` packages we already consume in `release.yml` — the new trust question isn't about the shim's CA chain (that's still Microsoft → Canonical, unchanged), it's about *whether the bundle mirror we downloaded was published by us, and not a man-in-the-middle*. That's a one-hop signature over the bundle manifest.

### 1.3 Common primitives

Both consumers need: (a) a signing tool that runs offline, (b) an operator-verifiable public key, (c) a rotation story, (d) a revocation story. Implementing them independently forks the trust chain. One design resolves both.

### 1.4 Constraints (ambient; bounds the solution space)

- **Federal-employee maintainer.** No D-U-N-S number, no authenticode certificate, no Apple Developer Program enrollment. Rules out any solution that requires corporate PKI.
- **Solo maintainer.** No ceremony-heavy key-custody protocol ("two-person offline signing with HSMs") is realistic. One person, one laptop, GitHub as ambient identity.
- **Repo mobility.** Project currently lives at `github.com/williamzujkowski/aegis-boot`; per #365 it migrates to an `aegis-boot` org. Any solution coupled to the owner slug must rotate on that move.
- **Existing minisign use.** `crates/iso-probe/src/minisign.rs` already verifies minisign detached sigs against `AEGIS_TRUSTED_KEYS` for per-ISO sidecars. Operators who have used `aegis-boot add` have already pointed that env var at a keyring. Pattern + mental model already exists.
- **Existing cosign use.** `release.yml:75-205` already signs every release asset (binary, initramfs, SBOM, `aegis-boot.img`, `SHA256SUMS`) via cosign keyless with the workflow-bound identity `https://github.com/williamzujkowski/aegis-boot/`. Operators running `fetch-image` already verify via cosign (`crates/aegis-cli/src/fetch_image.rs:173-303`).

---

## 2. Decision

One keypair. Minisign. Baked into the binary. Rotate only on compromise. No CRL. Cosign stays for release-artifact signing only.

Concretely:

| # | Question | Decision |
|---|----------|----------|
| 1 | Signing tool for attestation + bundle manifest | **minisign (Ed25519)** |
| 2 | One key or two | **One "aegis-boot project" Ed25519 keypair for both** |
| 3 | Public-key distribution | **Baked into binary at build time** via `include_bytes!` in a new `crates/aegis-cli/src/trust_anchor.rs` module |
| 4 | Rotation cadence | **Only on compromise.** The binary IS the trust anchor; "update aegis-boot" is the rotation mechanism |
| 5 | Revocation | **None.** Same reasoning — shipping a new binary with a new pubkey is the revocation |

Plus one implicit decision for clarity:

| # | Question | Decision |
|---|----------|----------|
| 6 | Cosign's role after this ADR | **Unchanged.** Cosign keyless continues to sign release artifacts in `release.yml`. It is not the operator-facing trust anchor for attestation manifests or bundle manifests. Operators verify the binary itself via cosign; the binary's embedded minisign pubkey then anchors everything the binary produces or consumes |

---

## 3. Rationale

### 3.1 Why minisign as the primary signing tool

Three properties matter for the operator-facing trust anchor: **no PKI infrastructure**, **small trusted compute base**, **operators already know it**.

- **No PKI.** Minisign is Ed25519 with a single pubkey file. No CA, no chain, no OCSP, no Authenticode, no Apple notarization. A federal-employee solo maintainer with no D-U-N-S cannot obtain those credentials; minisign is one of the few signature formats that works without them.
- **Small TCB.** We already depend on `minisign-verify` in `crates/iso-probe/` (`minisign.rs:27`). Verification adds zero new crates. Compare cosign: pulls Sigstore trust-root fetch + Fulcio cert verification + Rekor transparency log lookups — a large online-only TCB that defeats both consumers' offline-verify requirement.
- **Operator mindshare.** `AEGIS_TRUSTED_KEYS` env var + `.minisig` sidecar pattern already exist for per-ISO verification (`iso-probe/src/minisign.rs:17-22`). An operator who used `aegis-boot add` on a Fedora ISO has already touched this. Introducing a second format (minisign for ISOs, cosign for attestation) would double the mental model for no operator win.

**Objection 1: "Minisign is small-ecosystem; cosign is the industry standard for 2026."** True for container images and CI artifacts, which is exactly the use-case `release.yml` keeps. Not true for offline-verifiable file signatures on a USB stick where the verifier has no internet. The attestation manifest's canonical verify scenario is "field responder, air-gapped laptop, 18 months after flash" — cosign keyless signatures expire with Fulcio cert lifetime (10 minutes) and require online Rekor lookups to re-establish trust. Minisign signatures are forever-valid against a static pubkey.

**Objection 2: "Cosign keyless is already in release.yml — use it for consistency."** Consistency is a goal, not a rule. Cosign's role (signing what we publish to GitHub Releases) is distinct from minisign's role (signing what the *installed binary* emits). The binary has no OIDC token at run time; the only way cosign-sign something on an operator's laptop is with a managed key, which drops the one advantage cosign had (keyless). Split roles, not formats.

**Objection 3: "Two formats = two audit targets."** Yes. And they protect different things: cosign attests "this binary came from this GitHub Actions workflow on this tag"; minisign attests "this manifest was produced by a legitimate aegis-boot binary at runtime." Folding them into one format would require either (a) giving every operator laptop an OIDC token (absurd) or (b) pre-signing manifests at release time (wrong — manifests are per-flash, not per-release). Two formats is the correct shape.

### 3.2 Why one key for both consumers

**Consumer A (attestation) and Consumer B (bundle manifest) make the same trust statement from the operator's perspective**: "this artifact was produced by the aegis-boot project, not forged." Using separate keys would force every operator who wants offline bundle-verify to also import a second pubkey to verify their attestation. Zero operator-visible benefit; doubled key-custody burden for the maintainer.

The threat model that *would* justify two keys is "the attestation signing key leaks but the bundle signing key doesn't" — but since both keys live in the same `~/.secrets/aegis-boot/` directory on the same solo-maintainer laptop, that threat model is fiction. A compromise takes both.

**Objection 1: "Defense in depth — separate keys contain blast radius."** Real if the keys live in separate trust zones (HSM vs laptop, maintainer A vs maintainer B). Not real here — same laptop, same maintainer, same backup. Defense in depth with no depth is security theater.

**Objection 2: "Future-proofing — if we ever delegate attestation signing to a CI job…"** YAGNI. We can fork the keys the day that becomes a real requirement. The ADR is revisable; see "Revision triggers" below.

**Objection 3: "What if one consumer's threat model evolves differently?"** Then we revise the ADR and rotate to two keys — and the one-key → two-key migration is strictly easier than the two-key → unified-key migration (the latter forces every operator to re-import). Pick the cheaper-to-reverse default.

### 3.3 Why baked-into-binary distribution

Three options were on the table: (a) baked into binary via `include_bytes!`, (b) downloaded from a pinned mirror with TOFU, (c) shipped as a multi-key set for rotation.

**(a) baked-in wins** because the operator already trusts the binary — that's what cosign-verify on `aegis-boot-x86_64-linux.sig` establishes at `fetch-image` time. Once the binary is trusted, any pubkey it carries is trusted. There's no additional trust jump; the minisign anchor inherits cosign's provenance.

**(b) TOFU mirror** was considered and rejected. It introduces a new trust-on-first-use window (what mirror URL? when does TOFU lock in? what about operators who never hit the mirror before the first offline verify?), and the mirror itself becomes a key-custody problem (who signs the mirror manifest? — infinite regress).

**(c) multi-key set** (binary ships 3 rolling keys, rotates one per release) was considered and rejected. It optimizes for "rotate every release" which we are explicitly not doing (see 3.4). The complexity is pure cost.

The pubkey lives in `crates/aegis-cli/src/trust_anchor.rs` as:

```rust
// SPDX-License-Identifier: MIT OR Apache-2.0

/// Embedded minisign public key for aegis-boot project signatures.
///
/// This is the trust anchor for:
/// - Attestation manifests written by `aegis-boot flash` (#349)
/// - Bundle manifests downloaded by `aegis-boot fetch-image --direct-install`
///   on macOS/Windows (#367)
///
/// Rotation: see docs/architecture/KEY_MANAGEMENT.md §5.
pub const AEGIS_BOOT_TRUST_ANCHOR: &[u8] =
    include_bytes!("../../../keys/aegis-boot-trust-anchor.pub");
```

The `.pub` file is committed to the repo under `keys/` (public keys are not secrets; committing them is load-bearing — it's how reviewers audit rotations).

**Objection 1: "If the key is compromised, every installed binary is vulnerable until the operator updates."** Correct. This is the load-bearing tradeoff documented in §5 "Rotation means shipping a new binary." The acceptable-window argument: operators already re-run `curl | sh` for every release (that's how they got cosign updates, CVE fixes, etc.), so the key-rotation window equals the existing release-adoption window. No new exposure.

**Objection 2: "Reproducible builds — `include_bytes!` on a file that could change makes the build input-dependent."** True and desirable. The pubkey IS a build input; a rotation IS a new build. Reproducible builds still work per-release (same source + same pubkey → same binary).

**Objection 3: "What if we need to rotate faster than we can ship a new release?"** Then we ship a point-release. The CD pipeline is `.github/workflows/release.yml`; tagged release → signed artifacts → updated install script → operator `curl | sh`. End-to-end is hours, not weeks. For a zero-day compromise, that's acceptable (see §5.2).

### 3.4 Why rotate only on compromise

Per-release rotation was considered and rejected. It creates fragmentation: an operator running v0.13 can't verify an attestation written by v0.14 without pulling v0.14's binary to extract v0.14's pubkey. For a tool whose attestation manifests are meant to be verifiable years later, that's a multi-year key-tracking burden on the operator.

The correct rotation trigger is **evidence of compromise**: maintainer laptop stolen, `.secrets/` directory leaked, key file committed to a public repo, anything that puts the private key outside the maintainer's sole custody.

"Just in case" rotation has negative value here.

**Objection 1: "Industry best practice is to rotate yearly."** Industry best practice is for organizations with PKI, key custodians, and ops runbooks. For a solo maintainer with a single-laptop keypair, yearly rotation is busywork that doubles the "did the key leak?" surface (each rotation is another chance to leak via handling). Rotate when there's a reason.

**Objection 2: "What about cryptographic best-hygiene — keys should expire?"** Ed25519 has no known cryptographic expiration concern at century scale. The threat is *operational* (key handling, backup compromise, laptop loss), and operational rotation follows operational triggers, not a calendar.

**Objection 3: "A never-rotated key advertises 'stable target' to attackers."** The key is a 32-byte seed on a laptop that runs `cargo fmt` all day. It's not an internet-facing surface. The attack isn't "crack the key"; it's "exfiltrate it." Rotation on a schedule doesn't help with exfiltration — detection + rotation on discovery does.

### 3.5 Why no revocation

A compromised key needs a new binary with a new pubkey. That binary ships via the existing release channel; operators update via `curl | sh` or `brew upgrade aegis-boot` or `winget upgrade`. There is no operator population running arbitrary-old binaries that can't update — if they can't update, they can't verify anyway (cosign requires live Sigstore endpoints for cert chain validation, which they'd also fail).

A CRL would need:
- A pinned URL for CRL fetch (new trust problem)
- A signature over the CRL (new key-custody problem)
- Operator-side fetch logic (new code, new failure mode)
- "What if the CRL URL is down?" (new offline-verify problem)

All of this to solve a problem — "operator runs old binary whose key was revoked" — that the existing update mechanism already solves. It's a large implementation cost for a small threat mitigation that's already mostly covered.

**Objection 1: "Without revocation, a compromised key signs forever."** A compromised key signs until the maintainer observes the compromise and ships a new release. That window equals the window before the maintainer notices in any revocation scheme (CRLs don't magically detect compromise; they let you publish detection). The difference isn't detection; it's distribution — and our distribution (binary release) already has an update channel.

**Objection 2: "Air-gapped operators might never pull the new binary."** Correct. They also can't pull the CRL. Revocation doesn't help air-gapped operators; detection-and-rekey via the next manual update is the same end-state.

**Objection 3: "`aegis-boot doctor` could warn on stale trust anchor."** A good follow-up enhancement, not a reason to ship a CRL. Doctor can compare the embedded pubkey against the latest release's pubkey via GitHub API (or a pinned manifest URL) and warn "your trust anchor is older than the current release; consider updating." That's a UX nudge, not a crypto primitive.

---

## 4. Consequences

### 4.1 Positive

- **Single trust anchor.** One pubkey, one format, one env var (`AEGIS_TRUSTED_KEYS` for third-party ISO pubkeys; the project pubkey is embedded and always trusted for project-produced artifacts).
- **Offline-verifiable.** Minisign detached signatures verify against a static pubkey with no network dependency. Field responders, air-gapped labs, and FOIA archivists can verify attestations in 2040.
- **Zero new dependencies.** `minisign-verify` is already in the build. `minisign` CLI is already on the maintainer's laptop for ISO signing.
- **Consistent with existing pattern.** Reuses `.minisig` sidecar convention from `iso-probe/src/minisign.rs`. `aegis-boot flash` writes `<manifest>.minisig` next to `<manifest>.json`; `aegis-boot attest show` verifies inline.
- **Clean separation of concerns.** Cosign answers "did this binary come from our CI?" Minisign answers "did this runtime artifact come from a legitimate binary?" Each format serves its use case.

### 4.2 Negative + mitigations

- **Concrete operational burden: rotation-on-compromise is multi-step.** If the private key leaks:
  1. Maintainer generates a new keypair (`minisign -G -p keys/aegis-boot-trust-anchor.pub -s ~/.secrets/aegis-boot/trust-anchor.key`).
  2. Maintainer commits the new `.pub` to the repo.
  3. Maintainer tags a new release; `release.yml` rebuilds the binary with the new embedded pubkey and cosign-signs it.
  4. Operators run `curl -sSf https://… | sh` (or `brew upgrade` / `winget upgrade`) to pull the new binary.
  5. Any attestation or bundle manifest signed with the old key is now "signed by an untrusted key" to operators on the new binary. Old manifests need to be re-signed by the maintainer with the new key OR marked as historical and accepted under an explicit `--trust-legacy-key <fingerprint>` flag (follow-up feature).

  The rotation window is "hours from detection to tag" + "days-to-weeks for operator adoption." Acceptable for the threat model, but NOT a no-downtime rotation. Document prominently in `SECURITY.md`.

- **No cryptographic expiry forces operator vigilance.** An operator verifying a 10-year-old attestation has no cryptographic signal that the key has since been rotated. Mitigation: the `aegis-boot attest show` output includes the tool version that wrote it; an operator concerned about historical validity can cross-reference the `SECURITY.md` "key rotation log" (added in this ADR's follow-up work) to see if the signing version's key is still current.

- **Committing the `.pub` file to `keys/` pins reviewer attention on rotations.** Every rotation is a visible diff in the repo. Positive for transparency, but requires discipline on the maintainer to explain the rotation in the commit message (template: "security: rotate aegis-boot trust anchor — reason: <compromise trigger>").

- **Minisign's key ID is 8 bytes.** Collision resistance is fine for identification but not for security-critical selection. We never select keys by ID — there's exactly one embedded trust anchor — so this is a non-issue. Documented here so a future reviewer doesn't introduce a "multi-key by ID" design without understanding the limit.

### 4.3 Neutral consequences

- **Adds a `keys/` directory at repo root.** New convention. Populated at first release after this ADR lands. `.gitignore` must explicitly *not* ignore `keys/*.pub` (the file is public by definition).
- **Adds a `crates/aegis-cli/src/trust_anchor.rs` module.** ~20 lines: the `include_bytes!` const, a `parse_trust_anchor()` wrapper returning `minisign_verify::PublicKey`, a unit test asserting the bytes parse.
- **`SECURITY.md` gets a new section: "Key rotation log."** Initial entry: "2026-04-XX: initial keypair generated, fingerprint `RWR…`." Future rotations append.
- **Moving the repo to `github.com/aegis-boot/aegis-boot`** (per #365) does NOT require a key rotation. The minisign pubkey is orthogonal to the GitHub owner slug. Cosign keyless identities DO need to be updated in `release.yml` and in `fetch_image.rs`'s hardcoded identity regex (`fetch_image.rs:260`) — that's the repo-move work, tracked separately.

---

## 5. Implementation Plan (summary — full tickets to file after approval)

1. **Key generation (maintainer, one-time).** Generate keypair; commit `keys/aegis-boot-trust-anchor.pub`; store `~/.secrets/aegis-boot/trust-anchor.key` under GPG-encrypted backup. Record fingerprint in `SECURITY.md`.
2. **`trust_anchor.rs` module.** New file at `crates/aegis-cli/src/trust_anchor.rs`. Exports `AEGIS_BOOT_TRUST_ANCHOR: &[u8]` + `pub fn project_pubkey() -> Result<PublicKey, String>`.
3. **#349 implementation.** `record_flash()` + `record_iso_added()` in `attest.rs` write `<manifest>.json.minisig` alongside the JSON. Signing uses the private key at a path given by `AEGIS_BOOT_SIGNING_KEY` env var; in release builds this is expected to be unset and signing is a no-op with a warning (the maintainer signs release manifests; operator-written manifests are unsigned by default — see alternative §6.3).
4. **#367 Phase D implementation.** Bundle mirror publishes `bundle-manifest.json` + `bundle-manifest.json.minisig`. `fetch-image --direct-install` on macOS/Windows downloads both, verifies the `.minisig` against `trust_anchor::project_pubkey()`, then downloads the individual bundle files and verifies their SHA-256s match the manifest. Failure at any step aborts flash.
5. **`aegis-boot attest show` verifies.** When a `.minisig` sidecar is present, verify inline and surface "signature: verified ✓ (project key)" in the output.
6. **`SECURITY.md` section.** "Key rotation log" + "Compromise response runbook."

---

## 6. Alternatives Considered

### 6.1 Cosign keyless for everything (rejected)

**Proposal:** Use cosign keyless for both #349 and #367, binding every signature to the GitHub Actions workflow identity.

**Rejected because:**
- Operator laptops don't have OIDC tokens — cosign-sign at runtime is impossible without a managed key, which eliminates cosign's "keyless" advantage.
- Cosign verify requires live Sigstore / Fulcio / Rekor endpoints. Breaks the offline-verify requirement (§1.1, §1.2).
- #349's attestation happens at `flash` time on the operator's host; there's no workflow identity to bind to.

Preserved as the signing tool for *release* artifacts (unchanged); explicitly out of scope for *runtime-emitted* or *bundle-mirror* signatures.

### 6.2 Two keys — one for attestation, one for bundle (rejected)

**Proposal:** Separate keypairs for `attestation-signing` and `bundle-mirror-signing`. Compromise of one doesn't compromise the other.

**Rejected because:**
- Both keys live on the same laptop with the same maintainer. Defense-in-depth with no depth.
- Doubles the operator's pubkey-tracking burden for zero threat-model gain.
- One-key → two-key migration is strictly cheaper than two-key → one-key. Pick the reversible default.

Revisit trigger: if attestation signing is ever delegated to a CI job (i.e., one key stays on the maintainer laptop, the other moves to a GitHub Actions secret), split at that point.

### 6.3 Operator-side signing key per-host (rejected)

**Proposal:** Each operator generates their own minisign key; attestation manifests are signed with the operator's key, not the project's. Forensics consumers import operator pubkeys as needed.

**Rejected because:**
- Defeats Consumer B entirely — there's no project anchor to verify a downloaded bundle against.
- Creates an O(N-operators) pubkey-distribution problem for forensics consumers.
- Doesn't solve the threat it claims to solve: the operator laptop is already the host where the manifest is written, so an attacker with host access writes and signs in one step.

Partially preserved as a follow-up: operators MAY *also* sign attestations with their own key as an additional claim (counter-signature). Tracked as out-of-scope for this ADR; future feature.

---

## 7. Revision Triggers

This ADR should be re-opened if any of the following occur:

- **Sigstore adds offline-verifiable keyless signatures** (e.g., long-lived Fulcio certs or Rekor-less verification). Reconsider cosign for everything.
- **Maintainer team grows beyond one person.** Multi-custodian signing (m-of-n) changes the single-laptop threat model and may justify two-key separation.
- **Attestation-signing is delegated to a CI job.** Splits the key-custody zones; two keys become the right answer.
- **Apple / Microsoft code-signing requirements** force Authenticode / notarization for the binary itself. Doesn't change the trust anchor for manifests, but adds a parallel signature layer worth documenting.
- **Cryptographic break of Ed25519 at practical scale.** Not expected. Migration to a PQ signature (ML-DSA) would bump the key format; the ADR's shape (one key, baked-in, rotate-on-compromise) is unchanged.

---

## 8. References

- Tracking issue: [#366](https://github.com/williamzujkowski/aegis-boot/issues/366)
- Consumer A: [#349](https://github.com/williamzujkowski/aegis-boot/issues/349) — signed attestation manifest write
- Consumer B: [#367](https://github.com/williamzujkowski/aegis-boot/issues/367) — cross-platform `--direct-install` trust anchor (Phase D of [#365](https://github.com/williamzujkowski/aegis-boot/issues/365))
- Existing minisign use: `crates/iso-probe/src/minisign.rs`
- Existing cosign use: `.github/workflows/release.yml:75-205`, `crates/aegis-cli/src/fetch_image.rs:173-303`
- Attestation schema: `crates/aegis-wire-formats` (`Attestation`, `SCHEMA_VERSION: 1`)
- Related ADR: [0001](../adr/0001-runtime-architecture.md) — Runtime Architecture
- External prior art: Rufus `src/pki.c` / `src/net.c:345 DownloadSignedFile` (embedded-pubkey + runtime-download-verify pattern)
- External prior art: minisign spec — https://jedisct1.github.io/minisign/
