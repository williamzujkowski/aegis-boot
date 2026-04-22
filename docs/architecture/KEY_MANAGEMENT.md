# ADR 0002: Key Management for Signed Attestation + Bundle Trust Anchor

**Status:** ACCEPTED (revision 3)
**Date:** 2026-04-21
**Tracking issue:** [#366](https://github.com/williamzujkowski/aegis-boot/issues/366)
**Revision history:**
- 2026-04-20 — initial proposal (rev 1)
- 2026-04-21 — rev 2 adds Decision 7 (Key Epoch), §3.6 (historical anchors), and §5.1 (rotation rehearsal) after consensus vote surfaced revocation-circularity, temporal-deadlock, and rotation-atrophy objections (vote result: 60% approve, below supermajority threshold)
- 2026-04-21 — rev 3 closes rev-2's residual TOFU window on fresh installs by adding `MIN_REQUIRED_EPOCH` binary-embedded floor to Decision 7; extends §5.1 rehearsal with a signing-handoff audit step to waive the signing-oracle split trigger (§6.2)
- 2026-04-21 — **ACCEPTED** on rev 3 via higher-order consensus vote (architect ✓88%, security ✓90%, devex ✓87%, ai_ml ✓90%, pm ✓88%, contrarian ✗95% → 83.3% approve, supermajority cleared). Unblocks #349 (Phase 3b attestation signing) and #367 (Phase D bundle trust anchor). Vote verification hashes preserved in PR #385.
**Consumers:** [#349](https://github.com/williamzujkowski/aegis-boot/issues/349) (signed attestation manifest), [#367](https://github.com/williamzujkowski/aegis-boot/issues/367) (cross-platform `--direct-install` trust anchor, Phase D of [#365](https://github.com/williamzujkowski/aegis-boot/issues/365))
**Supersedes:** portion of `crates/aegis-cli/src/attest.rs` header comment (lines 22–28) that defers signing to "epic #139"; this ADR claims the design.

> **Status note.** This ADR is ACCEPTED as of rev 3 (2026-04-21, higher-order supermajority vote on #366). Future substantive changes should be tracked as a follow-up ADR that supersedes this one rather than in-place revision — the decision log stays linear. The `Alternatives considered` section names the three strongest contrarian paths; revisit triggers in §7 name the conditions under which each becomes worth re-opening.

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

- **Individual-account maintainer.** No D-U-N-S number, no Authenticode certificate, no Apple Developer Program enrollment. Rules out any solution that requires corporate PKI or a business-entity identity.
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

And one added in rev 2 to close the revocation-circularity gap surfaced by the first consensus vote:

| # | Question | Decision |
|---|----------|----------|
| 7 | Post-compromise replay defense | **Two-layer rollback floor.** (a) Binary-embedded `MIN_REQUIRED_EPOCH` constant: every release hardcodes the epoch active at the time the binary was compiled. Any manifest with `key_epoch < MIN_REQUIRED_EPOCH` is rejected unconditionally (protects fresh installs and systems with wiped trust-state). (b) Monotonic `seen-epoch` counter persisted to `$XDG_STATE_HOME/aegis-boot/trust/seen-epoch`: tracks the highest epoch this installation has observed post-install, and rejects any manifest below that (protects ongoing operators from rollback after install). The two layers compose: `reject if manifest.epoch < max(MIN_REQUIRED_EPOCH, seen-epoch)`. See §3.5 |

---

## 3. Rationale

### 3.1 Why minisign as the primary signing tool

Three properties matter for the operator-facing trust anchor: **no PKI infrastructure**, **small trusted compute base**, **operators already know it**.

- **No PKI.** Minisign is Ed25519 with a single pubkey file. No CA, no chain, no OCSP, no Authenticode, no Apple notarization. A solo maintainer on an individual account with no D-U-N-S cannot obtain those credentials; minisign is one of the few signature formats that works without them.
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

### 3.5 Why no CRL — and why a Key Epoch counter instead (rev 2)

A traditional CRL would need a pinned URL (new trust problem), a signature over the CRL (new key-custody problem), operator-side fetch logic (new code, new failure mode), and a "what if the CRL URL is down?" offline-verify answer. All of this to solve a problem — "operator runs old binary whose key was revoked" — that the existing update mechanism (`curl | sh`, `brew upgrade`, `winget upgrade`) already solves. Rev 1 stopped there.

The first consensus vote surfaced a real gap rev 1 missed: **revocation circularity**. If the private key leaks, the attacker can sign a malicious `v9.9.9` binary that ships with replayed-old-pubkey logic, or that silently accepts rollback. Air-gapped operators pulling that build can't distinguish it from a legitimate rotation. A CRL doesn't actually fix this (see above), but *something* has to — "just ship a new binary" breaks down the moment the attacker can also ship a new binary.

**Rev 2 addition (extended in rev 3): Key Epoch counter with a two-layer floor (Decision 7).**

Every signed manifest (attestation manifest for #349, bundle manifest for #367) carries an integer `key_epoch` field. The canonical keypair has an associated epoch, bumped on every rotation. Rev 3 defines *two* rollback defenses that compose:

**Layer 1 — binary-embedded floor (`MIN_REQUIRED_EPOCH`).** Every release hardcodes the epoch active at compile time. At binary build, `build.rs` reads `keys/canonical-epoch.json` and emits:

```rust
// crates/aegis-cli/src/trust_anchor.rs
pub const MIN_REQUIRED_EPOCH: u32 = env!("AEGIS_MIN_REQUIRED_EPOCH").parse().unwrap();
```

Any manifest whose `key_epoch < MIN_REQUIRED_EPOCH` is rejected unconditionally. This closes the TOFU window on fresh installs and on systems whose `seen-epoch` state file has been lost/wiped — the binary itself refuses to accept epochs older than the state of the world when that binary was released.

**Layer 2 — monotonic `seen-epoch`.** Binaries persist the highest observed epoch to `$XDG_STATE_HOME/aegis-boot/trust/seen-epoch`. This layer protects running installations from downgrade *after* install, in case the project rotates the key again post-install and the operator hasn't yet upgraded.

**Composed rule:** `reject if manifest.epoch < max(MIN_REQUIRED_EPOCH, seen-epoch)`. Either layer catches an attacker-replayed old signature; together they close both the pre-install and post-install windows.

**Verification flow.** On verify, the binary:
1. Verifies the minisign signature against the appropriate historical anchor (§3.6). FAIL → reject.
2. Reads the manifest's `key_epoch`.
3. Computes `floor = max(MIN_REQUIRED_EPOCH, seen-epoch)`. If `manifest.epoch < floor` → reject with `KeyEpochRollback`.
4. If `manifest.epoch > seen-epoch` → write the new value to `seen-epoch` (trust-on-first-use for the forward direction only; the floor is never mutable downward).

A key compromise becomes a recoverable event: maintainer rotates, bumps epoch, ships a new binary whose `MIN_REQUIRED_EPOCH` equals the new epoch. Fresh installs from that release onward are immune to replayed old-key signatures out of the box. Existing installations get the same immunity as soon as their `seen-epoch` advances past the rotation (either from receiving a new-epoch manifest or from `aegis-boot doctor` surfacing the mismatch and prompting update).

The `seen-epoch` file losing its value only costs Layer 2's ongoing-protection; Layer 1's embedded floor still fires. `aegis-boot doctor` reports both the local `seen-epoch` and the binary's `MIN_REQUIRED_EPOCH`, and if online, compares both against `https://github.com/aegis-boot/aegis-boot/raw/main/keys/canonical-epoch.json` so the operator can notice staleness in any layer.

**Objection 1 (preserved from rev 1): "Without revocation, a compromised key signs forever."** Still true in the absolute sense — nothing invalidates old signatures cryptographically. But the Epoch counter prevents *useful* forgery: an attacker with the pre-rotation key can sign 2019 attestations or replay old bundles, but cannot forge a current-state artifact that any updated operator will accept. The threat we actually need to stop ("attacker signs new-looking artifacts to fool current operators") is addressed.

**Objection 2 (preserved): "Air-gapped operators might never pull the new binary."** They also never observe the epoch bump. Correct, and unchanged by rev 2 — air-gapped operators live at whatever epoch their binary last saw. Document as a known-not-fixed class.

**Objection 3 (preserved): "`aegis-boot doctor` could warn on stale trust anchor."** Preserved as a follow-up UX nudge. Rev 2 adds concrete substance to it: doctor compares `seen-epoch` against a published-canonical-epoch manifest at `https://github.com/aegis-boot/aegis-boot/raw/main/keys/canonical-epoch.json`, fetched over HTTPS (signed by the same trust anchor — recursive but self-consistent).

### 3.6 Why historical-anchors list instead of single-pubkey bake-in (rev 2)

The first consensus vote surfaced a second gap: **temporal deadlock**. A field responder in 2028 using a post-rotation 2028 binary cannot verify a 2026 manifest signed with the pre-rotation 2026 key. Rev 1's "the binary IS the trust anchor" model collapses under its own rotation story — every rotation creates a verification epoch, and the ADR's forensics use-case ("verify 10 years later") stops working.

**Rev 2 addition: the trust anchor is a LIST, not a single pubkey.**

`crates/aegis-cli/src/trust_anchor.rs` exposes:

```rust
/// All trust anchors this binary recognizes, newest first.
/// The HEAD of the list is the active key (used for new signatures + epoch checks).
/// The TAIL is historical keys, kept around for verifying pre-rotation artifacts.
pub const AEGIS_BOOT_TRUST_ANCHORS: &[TrustAnchor] = &[
    TrustAnchor {
        epoch: 2,
        pubkey: include_bytes!("../../../keys/aegis-boot-trust-anchor-ep2.pub"),
        valid_from: "2027-03-15",
        reason: "scheduled rotation per §5.1 rehearsal cadence",
    },
    TrustAnchor {
        epoch: 1,
        pubkey: include_bytes!("../../../keys/aegis-boot-trust-anchor-ep1.pub"),
        valid_from: "2026-04-21",
        reason: "initial keypair",
    },
];
```

On verify, the binary walks the list and accepts the first pubkey for which the signature validates. The Epoch counter (§3.5) prevents rollback of an attacker-controlled old key from tricking operators — a pre-rotation manifest is accepted ONLY if its declared epoch equals the epoch of the pubkey that verified it, AND that epoch is not lower than `seen-epoch` for manifests claiming to be current.

Historical-anchor entries are append-only. Rotations ADD to the list; the active key moves to HEAD. Old operators with an older binary can only verify up to the newest anchor their binary carries, which is the expected behavior (they need to upgrade to verify anything newer than their binary's release). New operators with a new binary can verify everything back to epoch 1. Field-forensics at year-10 works as long as someone kept a binary shipped after the manifest was written.

**Trade-off surfaced:** the binary grows by ~48 bytes per retained historical anchor (Ed25519 pubkey + epoch + metadata). At any reasonable rotation cadence (once a year, catastrophically; zero-to-five times expected lifetime) this is negligible. We do not cap the list.

**Post-compromise purge:** if a key is known-compromised, the ADR explicitly decides it STAYS in the historical list. Removing it would break verification of legitimate pre-compromise artifacts; the Epoch-counter prevents the compromised key from being used to sign NEW-looking current-state artifacts. An optional `--trust-legacy-key <epoch>` flag (follow-up) can let an operator require manual opt-in per-historical-key if they want a stricter posture.

---

## 4. Consequences

### 4.1 Positive

- **Single trust anchor *list*** (rev 2). One format, one verification path, one active signing key. Historical keys are retained in-binary for year-N forensics (§3.6).
- **Offline-verifiable.** Minisign detached signatures verify against static pubkeys with no network dependency. Field responders, air-gapped labs, and FOIA archivists can verify attestations in 2040 as long as they have *any* binary shipped after the manifest was signed.
- **Rollback-resistant on fresh AND ongoing installs** (rev 3). The Key Epoch counter (§3.5) defends via two layers: Layer 1 is a binary-embedded `MIN_REQUIRED_EPOCH` floor set at release-compile time, immune to local-state wipes; Layer 2 is the monotonic `seen-epoch` file for ongoing rollback protection after install. Rev 2 had only Layer 2, which left a TOFU window on fresh installs. Rev 3 closes it.
- **Recovery-path exercised** (rev 2). Quarterly rotation rehearsals (§5.1) keep the runbook tested without burdening operators with actual rotations.
- **Zero new dependencies.** `minisign-verify` is already in the build. `minisign` CLI is already on the maintainer's laptop for ISO signing.
- **Consistent with existing pattern.** Reuses `.minisig` sidecar convention from `iso-probe/src/minisign.rs`. `aegis-boot flash` writes `<manifest>.minisig` next to `<manifest>.json`; `aegis-boot attest show` verifies inline.
- **Clean separation of concerns.** Cosign answers "did this binary come from our CI?" Minisign answers "did this runtime artifact come from a legitimate binary?" Each format serves its use case.

### 4.2 Negative + mitigations

- **Rotation-on-compromise is still multi-step, but now well-exercised** (rev 2). The rotation path runs quarterly (§5.1) in rehearsal mode, so the first real compromise is the maintainer's Nth execution of a familiar checklist, not their first. Production rotation steps: generate new keypair; append new pubkey to `keys/` with bumped epoch; update `trust_anchor.rs` and `canonical-epoch.json`; tag release; cosign-sign; operators upgrade. The rotation window is hours from detection to tag + days-to-weeks for operator adoption — acceptable for the threat model, not a no-downtime rotation. Documented in `SECURITY.md`.

- **Old manifests remain verifiable across rotations** (rev 2 improvement). Old attestations keep verifying under their original epoch's anchor in the historical-anchors list; the Epoch counter prevents that old anchor from being used to forge new-looking artifacts. The rev 1 mitigation ("re-sign or use `--trust-legacy-key`") is no longer required for normal operation — it becomes a stricter opt-in for operators who want every historical anchor to require manual trust.

- **Two orthogonal failure modes, both mitigated.** (a) A post-compromise attacker who replays an old key: blocked by Epoch counter. (b) A field responder verifying a 10-year-old attestation: works automatically if they're running any binary released after the attestation, because that binary carries the epoch-N anchor. The rev 1 "no cryptographic expiry forces operator vigilance" framing was a bug, not a feature; rev 2 treats historical verification as a first-class requirement.

- **Committing the `.pub` file to `keys/` pins reviewer attention on rotations.** Every rotation is a visible diff in the repo. Positive for transparency, but requires discipline on the maintainer to explain the rotation in the commit message (template: "security: rotate aegis-boot trust anchor — reason: <compromise trigger>").

- **Minisign's key ID is 8 bytes.** Collision resistance is fine for identification but not for security-critical selection. We never select keys by ID — there's exactly one embedded trust anchor — so this is a non-issue. Documented here so a future reviewer doesn't introduce a "multi-key by ID" design without understanding the limit.

### 4.3 Neutral consequences

- **Adds a `keys/` directory at repo root.** New convention. Populated at first release after this ADR lands. `.gitignore` must explicitly *not* ignore `keys/*.pub` (the file is public by definition). Contains one `.pub` per epoch, plus `canonical-epoch.json`.
- **Adds a `crates/aegis-cli/src/trust_anchor.rs` module.** ~60 lines (rev 2, was ~20 in rev 1): the historical-anchors list, epoch-counter logic, `verify_manifest()` wrapper, `seen-epoch` read/write, unit tests for rollback refusal + historical-anchor walking.
- **Adds a small state file under `$XDG_STATE_HOME/aegis-boot/trust/seen-epoch`** (rev 2). Contents: single integer. Losing it resets to epoch=0 (trust-on-first-use of whatever epoch is next observed); no catastrophic failure mode.
- **`SECURITY.md` gets three new sections**: "Key rotation log" (append-only, epoch-indexed), "Compromise response runbook", and "Rotation rehearsal checklist" (rev 2).
- **Adds `docs/architecture/rotation-rehearsal-log.md`** (rev 2) — dated log of quarterly rehearsal outcomes.
- **Moving the repo to `github.com/aegis-boot/aegis-boot`** (per #365) does NOT require a key rotation. The minisign pubkey is orthogonal to the GitHub owner slug. Cosign keyless identities DO need to be updated in `release.yml` and in `fetch_image.rs`'s hardcoded identity regex (`fetch_image.rs:260`) — that's the repo-move work, tracked separately.

---

## 5. Implementation Plan (summary — full tickets to file after approval)

1. **Key generation (maintainer, one-time).** Generate keypair; commit `keys/aegis-boot-trust-anchor-ep1.pub`; store `~/.secrets/aegis-boot/trust-anchor.key` under GPG-encrypted backup. Record fingerprint + epoch=1 in `SECURITY.md`. Write initial `keys/canonical-epoch.json` (`{"epoch": 1}`) alongside the pubkey.
2. **`trust_anchor.rs` module + `build.rs` hookup.** New file at `crates/aegis-cli/src/trust_anchor.rs`. Exports `AEGIS_BOOT_TRUST_ANCHORS: &[TrustAnchor]` (historical-anchors list, HEAD active) + `MIN_REQUIRED_EPOCH: u32` (rev 3, sourced from `keys/canonical-epoch.json` via `build.rs` at build time) + `pub fn verify_manifest(bytes, sig) -> Result<VerifiedManifest, VerifyError>` (walks the list, enforces epoch floor via `max(MIN_REQUIRED_EPOCH, seen-epoch)` against on-disk `seen-epoch`). A unit test asserts the rollback path rejects correctly in both fresh-install (no seen-epoch) and ongoing (seen-epoch set) configurations.
3. **Wire-format extension.** Add `key_epoch: u32` field to `aegis_wire_formats::Attestation` and the new `aegis_wire_formats::BundleManifest`. Bump `SCHEMA_VERSION` to 2 on each. Back-compat: binaries reading a `schema_version=1` manifest treat `key_epoch=0` as "pre-epoch manifest, accept if signature verifies under epoch=1 anchor."
4. **#349 implementation.** `record_flash()` + `record_iso_added()` in `attest.rs` write `<manifest>.json.minisig` alongside the JSON, embedding the current `key_epoch` in the manifest body. Signing uses the private key at a path given by `AEGIS_BOOT_SIGNING_KEY` env var; in release builds this is expected to be unset and signing is a no-op with a warning (the maintainer signs release manifests; operator-written manifests are unsigned by default — see alternative §6.3).
5. **#367 Phase D implementation.** Bundle mirror publishes `bundle-manifest.json` (with `key_epoch`) + `bundle-manifest.json.minisig`. `fetch-image --direct-install` on macOS/Windows downloads both, calls `trust_anchor::verify_manifest()`, then downloads the individual bundle files and verifies their SHA-256s match the manifest. Failure at any step aborts flash.
6. **`aegis-boot attest show` verifies.** When a `.minisig` sidecar is present, verify inline and surface "signature: verified ✓ (project key epoch N)" in the output; surface `KeyEpochRollback` as a red error with the mismatched epochs.
7. **`aegis-boot doctor` trust-anchor row.** New check: "Trust anchor: current epoch N (verified against GitHub-hosted canonical-epoch.json)." Warns on `seen-epoch < canonical-epoch`.
8. **`SECURITY.md` additions.** "Key rotation log" (epoch, date, fingerprint, reason — append-only); "Compromise response runbook"; "Rotation rehearsal checklist" (see §5.1).

### 5.1 Rotation rehearsal cadence (rev 2)

The first consensus vote surfaced **rotation atrophy** as a failure mode: untested production-critical code paths rot, and the first real compromise is a stressful, mistake-prone first-execution. Rev 1 said "rotate on compromise only"; rev 2 keeps that *production* policy but adds a **non-production rehearsal cadence** so the rotation machinery stays exercised.

**Policy:** every calendar quarter, the maintainer executes the rotation runbook on a throwaway branch. The rehearsal produces a tagged test build, verifies end-to-end cosign+minisign against a temporary keypair, and is then discarded — no commits to `keys/`, no epoch bump, no release. The rehearsal's output is a dated entry in `docs/architecture/rotation-rehearsal-log.md` with: date, branch, any steps that failed or surfaced friction, PR link to any runbook improvements that shipped.

**Rehearsal checklist** (lives in `SECURITY.md` as prose, summarized here):

1. Branch `chore/rotation-rehearsal-YYYYQN` from main.
2. Generate a temp keypair: `minisign -G -p /tmp/rehearsal.pub -s /tmp/rehearsal.sec`.
3. Replace `keys/aegis-boot-trust-anchor-ep-current.pub` with the temp pubkey (HEAD of list); bump epoch.
4. Run the full CI suite locally (`./scripts/dev-test.sh`) + the attestation/bundle sign+verify integration tests.
5. Tag a test release (`v0.X.Y-rehearsal-YYYYQN`) — verify `release.yml` bakes the new pubkey, cosign signs the binary, and a test operator install can verify a new attestation.
6. **Signing-handoff audit (rev 3).** Before each signing operation during the rehearsal, the maintainer verifies: (a) the sha256 of the artifact being fed to `minisign -Sm` matches the sha256 produced by an independent `cargo build --release` run on the same commit (i.e., the thing being signed is the thing that was built); (b) the signing command prompts for the private-key passphrase interactively (never sourced from a file or env var that could leak); (c) if a hardware token is in use, the token's touch-to-sign is actually required (no cached authorization). Each rehearsal log entry records the audit outcome. This step substitutes for the rev-1 two-key-separation proposal under §6.2's addendum: the rehearsal's audit verifies the signing surface's integrity quarterly, which is what the two-key split would have protected against via compartmentalization.
7. Revert the branch (`git reset --hard main`); delete temp keys; log the rehearsal with any drift observed in `docs/architecture/rotation-rehearsal-log.md`.

**Why quarterly, not monthly, not yearly.** Monthly rehearsal is too frequent to sustain for a solo maintainer and dilutes the "this is a real exercise" signal. Yearly is too infrequent to catch runbook rot — a new release channel added in January is untested until next January. Quarterly splits the difference and aligns with typical security-cycle cadences.

**This is a rehearsal, not a real rotation.** The production epoch does not bump; the committed keys do not change; operators see nothing. The only artifact is the log entry and any incidental runbook/tooling improvements the rehearsal surfaces.

**If a rehearsal quarter is missed.** Log the gap; do not silently skip. A missed rehearsal means the runbook is that much closer to rot, which is information worth preserving.

---

## 6. Alternatives Considered

### 6.1 Cosign keyless for everything (rejected)

**Proposal:** Use cosign keyless for both #349 and #367, binding every signature to the GitHub Actions workflow identity.

**Rejected because:**
- Operator laptops don't have OIDC tokens — cosign-sign at runtime is impossible without a managed key, which eliminates cosign's "keyless" advantage.
- Cosign verify requires live Sigstore / Fulcio / Rekor endpoints. Breaks the offline-verify requirement (§1.1, §1.2).
- #349's attestation happens at `flash` time on the operator's host; there's no workflow identity to bind to.

Preserved as the signing tool for *release* artifacts (unchanged); explicitly out of scope for *runtime-emitted* or *bundle-mirror* signatures.

### 6.2 Two keys — one for attestation, one for bundle (rejected, with caveat added in rev 2)

**Proposal:** Separate keypairs for `attestation-signing` and `bundle-mirror-signing`. Compromise of one doesn't compromise the other. First-vote objection: "key overload violates compartmentalization — a signing-oracle bug in attestation code would compromise bundle trust."

**Rejected because:**
- Both keys live on the same laptop with the same maintainer. Defense-in-depth with no depth.
- Doubles the operator's pubkey-tracking burden for zero threat-model gain in the *current* architecture.
- One-key → two-key migration is strictly cheaper than two-key → one-key. Pick the reversible default.

**Rev 2 addendum — answering the signing-oracle angle.** The objection is valid *if* aegis-boot ever grows a code path that signs attacker-controlled input. It does not today: signing is a local filesystem operation invoked exclusively by the maintainer (for the project key) or the operator for their own host's attestations (where the operator already has full host control). There is no network-exposed signing surface. The signing-oracle threat requires the key to sign input that another actor influences — which is exactly the "attestation-signing delegated to a CI job" trigger in §7.

Revisit triggers: (a) signing moves off the maintainer laptop to any service that accepts external input, OR (b) attestation signing is delegated to a CI job. At that point, split into root-signs-subordinates per the two-tier model.

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
- **Attestation-signing is delegated to a CI job** — for example, a signing service that accepts operator-submitted manifests over a network. That surface exposes the key to remote inputs and resurrects the "signing oracle" threat. At that point, split into a root key (offline, signs subordinate keys) + subordinate signing key (online, rotatable independently) per the two-tier model the first consensus vote advocated.
- **Apple / Microsoft code-signing requirements** force Authenticode / notarization for the binary itself. Doesn't change the trust anchor for manifests, but adds a parallel signature layer worth documenting.
- **Cryptographic break of Ed25519 at practical scale.** Not expected. Migration to a PQ signature (ML-DSA) would bump the key format; the ADR's shape (historical-anchors list, epoch counter, rotate-on-compromise) is unchanged.
- **Rehearsal log shows ≥2 consecutive missed quarters.** Triggers a meta-review: either the cadence is wrong for the project's pace, or the runbook friction is too high and the rehearsal is being skipped for the wrong reason. Either way, the cadence decision (§5.1) is worth revisiting.
- **Epoch counter false-positives at scale.** If operator reports surface "KeyEpochRollback was rejected but should have been accepted" with non-trivial frequency, the trust-on-first-use-of-forward-epoch model may be too strict; consider a grace window or a per-source epoch store.

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
