# ADR 0003: Cross-reboot last-booted persistence

**Status:** PROPOSED
**Date:** 2026-04-20
**Tracking issue:** [#375](https://github.com/williamzujkowski/aegis-boot/issues/375)
**Supersedes scope-wise:** nothing yet; extends the tmpfs-only design in `crates/rescue-tui/src/persistence.rs` (lines 3–11)
**Related:** [#123](https://github.com/williamzujkowski/aegis-boot/issues/123) (misleading "SHIPPED" claim), [#132](https://github.com/williamzujkowski/aegis-boot/issues/132) (acceptance-criteria mismatch caught in real-hardware validation 2026-04-21), [#342](https://github.com/williamzujkowski/aegis-boot/issues/342) (two-stage tmpfs→disk pattern reused here), [#277](https://github.com/williamzujkowski/aegis-boot/issues/277) / attestation manifests in `crates/aegis-cli/src/attest.rs`, [#366](https://github.com/williamzujkowski/aegis-boot/issues/366) (ADR 0002, key management — companion ADR landed same day)

> Numbering: ADR 0002 is claimed by the #366 key-management ADR that merged first on `docs/366-key-management-adr`. This ADR takes 0003 per the coordination rule in §11.

---

## 1. Context

Aegis-boot's rescue-tui has a persistence module that remembers the operator's last ISO selection so that a failed kexec returning to rescue-tui can pre-position the cursor on the row the operator was working with. That module lives at `crates/rescue-tui/src/persistence.rs` and is wired from `crates/rescue-tui/src/main.rs:820-862` (`apply_persisted_choice` on startup, `save_last_choice` at kexec-confirm time).

**What ships today, verbatim from the module docstring (`persistence.rs:3-11`):**

> Storage: JSON at `$AEGIS_STATE_DIR/last-choice.json` (defaults to `/run/aegis-boot`). `/run` is a tmpfs; state is lost at reboot, which is exactly what we want for a rescue environment.
>
> Persistence across reboots would require writing to the boot media, which is out of scope here — that's a TPM/NVRAM story for a later ADR.

This is the ADR the docstring points at.

**Scope mismatch caught during #132 real-hardware validation** (see `docs/validation/REAL_HARDWARE_REPORT_132.md` §"Spec mismatch"): #132's acceptance criteria call for rescue-tui's cursor to land on the last-booted ISO *after a full reboot of the stick*, but the shipped implementation only survives within a single boot session (i.e., a failed kexec looping back into rescue-tui, which never loses `/run`). Issue #123's claim "Pre-selection on next boot — SHIPPED" is accurate only for within-session loops. For the cross-reboot claim to become true, the state must move from tmpfs onto the boot media.

This ADR scopes the cross-reboot design. It is a deliberately narrow successor ADR: it does **not** re-open the within-session design, which is correct and stays.

### 1.1 Why this keeps tripping people

The `/run/aegis-boot` location looks disk-backed at a glance — the path is stable, the file is real JSON, and `load()`/`save()` round-trip cleanly in tests. Only the `/run` convention (a Linux tmpfs) tells you state vaporizes on reboot. Three reviewers and one release have shipped believing "pre-selection on next boot" works end-to-end. A proper ADR + a renamed constant (§5) will stop the confusion.

---

## 2. Decision

Write `last-choice.json` to a hidden directory on the **AEGIS_ISOS data partition** — the same operator-writable partition that already holds ISOs, sidecars, and (per #342) failure microreports. Use the **two-stage tmpfs → AEGIS_ISOS migration pattern** already proven in `crates/rescue-tui/src/failure_log.rs`. Keep the payload minimal (ISO path + timestamp). Keep the load-side failure semantics unchanged (any error → `None` → fresh-start fallback).

Concretely:

| Question                     | Answer                                                                    |
| ---------------------------- | ------------------------------------------------------------------------- |
| Storage target               | `AEGIS_ISOS/.aegis-state/last-choice.json` (hidden dir, exFAT hidden attr) |
| Write protocol               | Two-stage: tmpfs staging → `rename(2)` onto AEGIS_ISOS + `fsync` on dir   |
| Payload                      | `{ iso_path: <relative-to-mount>, saved_at: RFC3339, schema_version: 1 }` |
| Cmdline-override persistence | **NO** — security smell; re-enter every boot if you want non-default     |
| Attestation cross-reference  | **NO** — keep orthogonal; attestation is audit, last-choice is UX         |
| Load failure mode            | Return `None` (unchanged from today); log at debug; boot as fresh start  |
| Save failure mode            | Log at warn; **never** fail the boot or kexec                             |

This is the minimum viable cross-reboot design. Every expansion (TPM binding, signed last-choice, cmdline persistence) is explicitly deferred.

---

## 3. Threat model

This ADR's correctness argument depends on the trust boundaries below being accurate. If any of them shifts, re-open.

**Trusted parties:**
- **Stick holder** — whoever has physical possession of the aegis-boot USB. We already trust them to choose which ISO to boot; adding "which ISO they booted last" conveys zero new capability.
- **Signed boot chain** (shim → GRUB → rescue kernel → kexec-loader) — validated per ADR 0001. We do not persist state into this chain.

**Semi-trusted:**
- **exFAT driver + filesystem integrity on AEGIS_ISOS** — the operator's data partition. Already trusted to hold ISOs; rename-over + fsync gives us the same durability story the filesystem offers anyone else.

**Untrusted / out of scope:**
- **Stick theft or borrow** — if someone steals the stick, they already see every ISO filename on AEGIS_ISOS. A 200-byte `last-choice.json` adds no meaningful disclosure. *Contrarian objection 1: "but it leaks which ISO the operator uses most." Answer: the set is already visible in the ISO directory; `last-choice` narrows it by one index, and the operator could equally re-order ISOs by mtime. Risk accepted.*
- **Tampered last-choice file** — an attacker with write access to AEGIS_ISOS can rewrite `last-choice.json`. The attack payoff is "pre-select the wrong row"; the operator still confirms with Enter before kexec, and every ISO still undergoes the Phase-1 sha256 verify against its sidecar on discovery (see `iso-probe: hash verified` in `REAL_HARDWARE_REPORT_132.md:61`). Tampering cannot influence what gets kexec'd, only what cursor starts highlighted. *Contrarian objection 2: "what about a malicious ISO path that points outside AEGIS_ISOS?" Answer: Phase 1's `apply_persisted_choice` already uses `iter().position()` to look up the stored path in the already-discovered ISO list; unknown paths return `None` and fall back to fresh start (see `main.rs:828-838`). The attack has no purchase.*
- **TPM sealing, remote attestation of last-choice** — out of scope. Tracked implicitly under #139.

**Rule of Two check:** the writer is rescue-tui running as PID 1 inside initramfs, already has write access to AEGIS_ISOS, does not hold secrets, and does not process untrusted external input beyond the operator's Enter keypress. All three legs of the rule are not simultaneously present; no human-approval gate needed.

---

## 4. Consequences

### Positive

- **Closes the #123 scope gap.** After this ADR ships in code, the README / issue-tracker claim "Pre-selection on next boot — SHIPPED" becomes true literally, not just within-session.
- **Closes #132's unsatisfied acceptance criterion** via the Phase 3 real-hardware E2E test in §8.
- **Reuses a reviewed, shipped pattern.** The tmpfs→AEGIS_ISOS migration in `failure_log.rs:186-261` has been through review, two rescue-tui releases, and real-hardware validation (`REAL_HARDWARE_REPORT_132.md:60`). We inherit its durability story for free.
- **No new dependency.** No TPM, no `tpm2-tools`, no UEFI NVRAM ioctl surface. The design works on every stick regardless of host firmware.
- **Testable.** Existing `persistence.rs` test suite (roundtrip, missing-file, corrupt, nested-parent) extends trivially; the new assertions are "file appears under `AEGIS_ISOS/.aegis-state/`" and "old tmpfs location is empty after migration."

### Negative

- **exFAT has no journal.** Every `write` is physical on the flash cells. *Contrarian objection 3: "this will wear the stick." Flash-wear math: one save = ~200 bytes user data, one NAND page write = ~4 KiB minimum (write amplification to a full page). One operator session = one save = one 4 KiB page. A consumer USB stick's NAND endurance budget is conservatively 1,000 P/E cycles per block × ~4,000 blocks on a 32 GB stick ≈ 4 × 10⁶ page-writes of headroom. Even at one reboot/day for 10 years = 3,650 writes. Write amplification is negligible at this scale. Risk assessed, accepted, documented.*
- **fsync semantics on exFAT are implementation-defined.** Linux's exfat.ko honors `fsync(2)` on file + directory since 5.7; we rely on that. Older kernels in an operator's legacy dev host are not our problem because the *writer* is the rescue kernel we ship, which is ≥ 6.14 per `REAL_HARDWARE_REPORT_132.md:123`. The reader (the rescue kernel on the next boot) is also ours. Host-side writes (`aegis-boot flash`, `aegis-boot add`) do not touch `last-choice.json`.
- **Mid-write stick removal is possible.** Mitigation: write staging file to tmpfs, rename onto AEGIS_ISOS, fsync dir. Atomicity guarantee: either the old file is intact or the new file is intact. Partial file is unreachable under POSIX rename semantics on Linux + exfat.ko (rename is atomic within a directory). If stick removal happens *between* rename and dir fsync, the new name exists but the directory entry may not be flushed; next boot sees old state; this is acceptable — we fall back to fresh-start on any read error.
- **The existing tmpfs path becomes a staging area, not the authority.** Anyone reading the current `persistence.rs` will see a behavior change. Mitigated by Phase 1 below (explicit `default_state_dir()` rename).

---

## 5. Alternatives considered

### 5.1 TPM NVRAM (rejected)

*Contrarian objection 4: "TPM-sealed last-choice is the security-correct answer."* Partially true — a TPM-sealed receipt would survive reboot *and* prevent tampering. But:

- **Hardware dependency.** Aegis-boot's product stance is "drop it on any UEFI box with a USB port." Introducing a TPM requirement for a UX nicety regresses the portability story.
- **`tpm2-tools` footprint.** Including `tpm2-tools` or the `tss2` crates in the initramfs is measurable kilobytes and a real build-and-review cost. Cost/benefit is upside-down for a 200-byte UX hint.
- **Sealed-against-what?** Sealing to PCRs that the rescue kernel controls means the seal unlocks only under the same rescue kernel — fine for UX, useless as a security primitive (a compromised rescue kernel trivially unseals). The only non-trivial sealing policy is TPM-bound to a *different* PCR set, which re-introduces the hardware-portability problem.

Revisit if/when aegis-boot gains a "TPM-attested mode" (epic #139).

### 5.2 UEFI NVRAM variables (rejected)

- **Per-machine, not per-stick.** UEFI NVRAM lives in the firmware, not on the USB. Booting the same stick on a second machine would lose state. Violates the "portable stick" product premise.
- **Write contention with firmware.** Some consumer firmwares rate-limit NVRAM writes or fail mysteriously under Secure Boot enforcing. We already fight firmware quirks enough.
- **Requires `efivarfs` write access, which lockdown-integrity may restrict.** Our rescue kernel boots with `lockdown=integrity` per ADR 0001; efivarfs writes are allowed but auditing is painful.

### 5.3 ESP (rejected)

*Contrarian objection 5: "ESP is small but it's already mounted and writable."* No — it's **mounted**, but writing operator state to the signed-chain partition is a trust-separation violation:

- The ESP holds the shim, GRUB image, grub.cfg, and kernel/initrd that are part of the verified boot surface. Any write we do there shares a filesystem with artifacts whose integrity we care about. A bug that corrupts `last-choice.json` and happens to stride into `grub.cfg` becomes a brick, not a UX regression.
- vfat's free-space allocator is no stronger than exfat's here, so the alleged fsync advantage is mythical.
- Product posture: "ESP is for the boot chain; AEGIS_ISOS is for the operator." Don't cross the streams for 200 bytes.

### 5.4 AEGIS_ISOS, flat file at partition root (rejected sub-variant)

A file literally at `AEGIS_ISOS/last-choice.json` would clutter the directory the operator sees when they mount the stick on a laptop. The `.aegis-state/` hidden subdirectory (plus exFAT's `hidden` attribute on the dir) is cheap and keeps the operator's file listing clean.

---

## 6. Detailed design

### 6.1 Storage layout

```
AEGIS_ISOS/                                (exFAT, operator-writable)
├── ubuntu-24.04-desktop-amd64.iso
├── ubuntu-24.04-desktop-amd64.iso.sha256
├── aegis-boot-logs/                        (#342 failure microreports)
│   └── ...
└── .aegis-state/                           (NEW — hidden dir, exFAT hidden attr)
    └── last-choice.json                    (NEW — this ADR)
```

The `.aegis-state/` directory is created lazily on first save; missing dir on load returns `None`.

### 6.2 File schema (v1)

```json
{
  "schema_version": 1,
  "iso_path": "ubuntu-24.04-desktop-amd64.iso",
  "saved_at": "2026-04-20T18:04:22-04:00"
}
```

- `iso_path` is **relative to the AEGIS_ISOS mount point**, not absolute. Absolute paths embed `/run/media/aegis-isos/` which is a rescue-kernel convention; relative paths survive mount-point changes and are robust against the stick being mounted at a different prefix by `aegis-boot add` on a host machine.
- `saved_at` is RFC3339 with offset. Informational only; we do not use it to expire state today. Recording it makes the retention/rotation knob cheap to add later without a schema bump.
- `schema_version: 1` gives us a forward-compatibility story matching `aegis-wire-formats` conventions (see `FAILURE_MICROREPORT_SCHEMA_VERSION` in `failure_log.rs:31`).

No `cmdline_override` field. The existing in-memory `LastChoice { cmdline_override }` field stays for within-session use but is **not serialized** to the cross-reboot file. Rationale: a persisted cmdline override is operator-supplied data that bypasses the usual "re-enter and re-confirm" loop. Persisting it lets stale / one-off debug flags silently leak into the next session's boot. The operator re-types if they need it.

### 6.3 Write protocol (two-stage)

```
1. Serialize LastChoice to JSON.
2. Write to /run/aegis-boot/last-choice.json.staging (tmpfs, atomic).
3. fsync the staging file.
4. Check AEGIS_ISOS mounted + writable; if no → stop. tmpfs copy is the
   authority until next rescue-tui run migrates it.
5. Copy staging → AEGIS_ISOS/.aegis-state/last-choice.json.tmp
6. fsync the .tmp file.
7. rename(2) .tmp → last-choice.json   (atomic within directory).
8. fsync the .aegis-state/ directory to flush the rename.
9. Delete the tmpfs staging file.
```

Any step failing after 1–3 leaves the tmpfs copy intact, so within-session recall still works. Any failure after step 7 leaves the AEGIS_ISOS copy intact, which is the authoritative cross-reboot state.

### 6.4 Load protocol

```
1. If AEGIS_ISOS/.aegis-state/last-choice.json exists and parses → return.
2. Else if /run/aegis-boot/last-choice.json exists and parses → return.
   (within-session fallback — compatible with today's behavior)
3. Else → None.
```

Step 2 preserves the within-session behavior for operators whose AEGIS_ISOS mount failed (#132 Bug 1 class of failures). We do not want a cross-reboot redesign to regress the within-session UX the module was built to deliver.

### 6.5 Migration from the old layout

One-shot: on first rescue-tui run after this ADR ships, read `/run/aegis-boot/last-choice.json` (if present) and promote it to the AEGIS_ISOS layout. No versioned state-file migration needed — the in-memory `LastChoice` type is unchanged; the old file either parses with the new code or gets ignored (the corrupt-returns-None path already exists at `persistence.rs:64-73`).

---

## 7. Non-goals

- **TPM-sealed last-choice.** See §5.1.
- **Signed last-choice file.** A simple HMAC with a key that lives on the same stick adds no security. Proper signing requires a key not on the stick, which is the TPM / remote-attestation story — and now also the #366 / ADR 0002 key-management story.
- **Cmdline-override persistence.** See §6.2.
- **Multi-stick-identity tracking** (e.g., "this stick's last choice was X on machine A, Y on machine B"). The stick itself is the identity unit.
- **Expiry / TTL on last-choice.** Zero operator demand; adds a clock-skew failure mode. Revisit if the field proves useful.
- **UI "[last]" indicator in the List row.** Covered by #132's acceptance criteria and implemented in Phase 3, but orthogonal to the persistence mechanism decided here.

---

## 8. Implementation plan

Three sub-issues under #375, merged in order. Each is independently reviewable; the order is chosen so that merge-between-phases keeps rescue-tui working.

### Phase 1 — `persistence.rs` target-dir abstraction (~1 day, file-local)

- Rename `default_state_dir()` → `tmpfs_staging_dir()` (it's still `/run/aegis-boot`).
- Add `aegis_isos_state_dir()` returning `/run/media/aegis-isos/.aegis-state/`.
- Keep the `AEGIS_STATE_DIR` env-var override; make it override both paths for tests.
- Update `main.rs` call sites (`apply_persisted_choice` at line 823, `save_last_choice` at line 850) to use the new helpers but still write to tmpfs only (behavior unchanged).
- Purpose: gets the renaming churn out of the way so Phase 2 is a pure behavior change.
- Acceptance: existing tests pass untouched; new test covering `aegis_isos_state_dir()` path resolution.

### Phase 2 — two-stage write + promote-on-load (~2 days)

- Implement write protocol §6.3 in `persistence.rs::save_persistent()`.
- Implement load protocol §6.4 in `persistence.rs::load()`.
- Hook migration §6.5 into rescue-tui startup: first load attempt on new layout; if absent but old tmpfs copy present, write it through and delete the old.
- Unit tests: explicit staging→promote→read on a `tempfile::tempdir()` acting as AEGIS_ISOS; mid-write failure injection via a pre-created read-only target; `rename` fall-through when target dir missing.
- Integration smoke: rescue-tui under `ratatui::TestBackend` with a fake AEGIS_ISOS root.
- Acceptance: round-trip across a simulated reboot (drop `/run`, keep AEGIS_ISOS) returns the prior choice.

### Phase 3 — Real-hardware E2E test that closes #132 (~1 day)

- Extend `docs/validation/REAL_HARDWARE_REPORT_132.md` test procedure (now that Bug 1 exfat-modprobe is fixed per commit e27bb91): boot stick, select Ubuntu, confirm, reboot stick, observe cursor on Ubuntu row.
- Add a CI-reachable variant under `ci/direct-install-e2e.yml`: simulate reboot by restarting the QEMU instance with persistent USB-passthrough state; assert that `rescue-tui` logs `restored last choice` (the existing `tracing::info!` at `main.rs:839-843` is already wired).
- Close #132 and #375 referencing Phase 3's merged PR.

---

## 9. Open questions

- **Should `AEGIS_ISOS/.aegis-state/` get the `hidden` exFAT attribute, or is the leading-dot convention enough?** Leaning "leading dot only" — Linux clients ignore the exFAT hidden bit anyway, and setting it requires an `exfatprogs` call the rescue kernel doesn't currently have. Keep as a Phase-2 review-time question.
- **Cross-ADR: does #366 / ADR 0002's key-management story want to piggyback `.aegis-state/` for anything?** The directory is a natural spot for per-stick state that isn't signed-chain. Leave to ADR 0002's follow-ups; this ADR claims only `last-choice.json`.

---

## 10. References

- `crates/rescue-tui/src/persistence.rs` (current, in-scope-for-change)
- `crates/rescue-tui/src/main.rs:820-862` (call sites)
- `crates/rescue-tui/src/failure_log.rs:1-26, 186-261` (two-stage pattern being reused)
- `crates/aegis-cli/src/attest.rs:3-11` (attestation store on operator host, explicitly orthogonal)
- `docs/validation/REAL_HARDWARE_REPORT_132.md` (scope-mismatch discovery, Phase 3 test basis)
- ADR 0001 `docs/adr/0001-runtime-architecture.md` (lockdown-integrity + signed-chain context)
- ADR 0002 `docs/architecture/KEY_MANAGEMENT.md` (#366, landed same day — this ADR's non-goal "signed last-choice" is in its domain)

---

## 11. Numbering note

This file lives at `docs/architecture/LAST_BOOTED_PERSISTENCE.md` per the directing issue #375. The parallel #366 key-management ADR created `docs/architecture/` and claimed ADR 0002 by merging first on `docs/366-key-management-adr`. Per the coordination rule (file the next unused number if there's a conflict), this ADR takes **0003**. `docs/adr/0001-runtime-architecture.md` remains the earliest; future ADRs under `docs/architecture/` continue from 0004.
