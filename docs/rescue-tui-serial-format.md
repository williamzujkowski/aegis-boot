# Rescue-TUI Serial Output Contract

This document captures the serial-console strings rescue-tui emits in machine-readable contexts: harness-driven test modes (#675) and other automation hooks. These strings are **load-bearing**: external harnesses (notably [`aegis-hwsim`](https://github.com/aegis-boot/aegis-hwsim)) grep-pin them to convert Skip → Pass on signed-chain regression tests. Wording changes here cascade into a coordinated PR on the consuming repo.

## Cmdline-driven test modes (#675)

When the kernel cmdline carries `aegis.test=<name>`, the initramfs `/init` exports `AEGIS_TEST=<name>` and rescue-tui short-circuits its interactive UI to run the named scripted check, then exits with the test's process exit code.

### `aegis.test=kexec-unsigned`

Companion to [aegis-hwsim `scenarios/kexec_refuses_unsigned.rs`](https://github.com/aegis-boot/aegis-hwsim/pull/78) (epic E5.3). Asserts that under `lockdown=integrity`, `kexec_file_load(2)` rejects an unsigned image.

**What it does**:

1. Stages a 4 KiB run of zeros at `/run/aegis-test-unsigned`.
2. Calls `kexec_file_load(2)` with that file as the kernel fd, no initrd, empty cmdline.
3. Expects `-EKEYREJECTED` (kernel signature gate) or `-EPERM` (lockdown gate). Either is a Pass.
4. If the syscall returns `Ok` — meaning the kernel accepted an unsigned blob — that is a load-bearing Fail; rescue-tui exits non-zero and the harness records the regression.

**Serial landmarks (exact substrings)**:

| Stage | Landmark | Meaning |
|---|---|---|
| Test start | `aegis-boot-test: kexec-unsigned starting` | Confirms the stick has the test mode (not Skip). |
| Pass — signature gate | `aegis-boot-test: kexec-unsigned REJECTED (errno: EKEYREJECTED)` | `KEXEC_SIG` rejected the unsigned image. Expected outcome under `kexec_file_load` with `KEXEC_SIG_FORCE` or under `lockdown=integrity` with the signature path enabled. |
| Pass — lockdown gate | `aegis-boot-test: kexec-unsigned REJECTED (errno: EPERM-lockdown)` | Lockdown blocked the syscall before reaching the signature gate. Same operator-visible property; harness counts as Pass. |
| Pass — other gate | `aegis-boot-test: kexec-unsigned REJECTED (other: <KexecError>)` | Some other `kexec_file_load` error path fired (e.g. `ENOEXEC` from format parsing). Still means "no unsigned load happened"; harness counts as Pass. |
| **Fail — load-bearing regression** | `aegis-boot-test: kexec-unsigned UNEXPECTEDLY-LOADED` | Kernel accepted the unsigned image. Signed-chain regression. |

**Process exit code**: `0` for any Pass landmark; `1` for `UNEXPECTEDLY-LOADED`.

**Why this isn't a real bzImage**: under `lockdown=integrity`, the signature check fires before the format parser. A real (signed) bzImage isn't necessary — and synthesising one would defeat the test, since we want the path that asserts "unsigned content gets rejected." The harness only cares that one of the rejection landmarks fires.

### `aegis.test=mok-enroll`

Companion to [aegis-hwsim `scenarios/mok_enroll_alpine.rs`](https://github.com/aegis-boot/aegis-hwsim/pull/79) (epic E5.4). Asserts that the operator-facing MOK enrollment walkthrough (#202) is intact and emits the load-bearing copy-paste command without drift.

**What it does**:

1. Prints a `MOK enrollment walkthrough starting` header.
2. Prints the canonical 3-step walkthrough body — same text the rescue-tui kexec-failure path renders, sourced from `crate::state::build_mokutil_remedy`.
3. Prints a `MOK enrollment walkthrough complete` footer + exits 0.

This is a static-text mode; there's no kexec, no ISO, no kernel — just the operator-visible recovery prose, so the harness can verify the contract without driving a real unsigned-kernel boot.

**Serial landmarks (exact substrings)**:

| Stage | Landmark | Meaning |
|---|---|---|
| Walkthrough fired | `MOK enrollment walkthrough` | Confirms the stick has the test mode (not Skip). |
| Step marker | `STEP 1/3` | Confirms the walkthrough body printed. |
| **Load-bearing command** | `sudo mokutil --import` | The verbatim copy-paste payload from #202. Drift here leaves an operator at 2 AM with a non-working command line — the harness exists to catch exactly this. |

**Process exit code**: always `0`. The harness validates by grep, not exit status.

**Why we render the no-key variant**: with no real ISO at hand, `build_mokutil_remedy(None)` gives the prose-step-1 form which still contains the load-bearing `sudo mokutil --import` substring (under "aegis-boot will then generate the exact `sudo mokutil --import <path>` command for you"). Operators in the field hit the with-key variant when they have a sibling `.pub`/`.key`/`.der` on the stick; the substring contract holds in both.

### `aegis.test=manifest-roundtrip`

Companion to [aegis-hwsim's E6 attestation-roundtrip scenario](https://github.com/aegis-boot/aegis-hwsim/issues/6) (#695). Mounts the ESP, parses the on-stick attestation manifest via `aegis-wire-formats::Manifest`, and (when populated) compares each `expected_pcrs[]` entry to the live PCR.

**What it does**:

1. Resolves the ESP block device via `/dev/disk/by-label/AEGIS_ESP` (the canonical udev symlink).
2. Mounts read-only at `/run/aegis-test-esp`.
3. Reads `aegis-boot-manifest.json` from the ESP root, parses via the in-tree `Manifest` type.
4. If `expected_pcrs[]` is empty (PR3-era; current shipped behavior per `docs/attestation-manifest.md`), prints the `empty-pcrs` landmark and exits 0. Harness fail-opens — counts as Pass.
5. If populated, iterates each entry: reads `/sys/class/tpm/tpm0/pcr-<bank>/<idx>` and emits a MATCH or MISMATCH landmark. Exit 0 only if every entry matches.

**Serial landmarks (exact substrings)**:

| Stage | Landmark | Meaning |
|---|---|---|
| Test start | `aegis-boot-test: manifest-roundtrip starting` | Confirms the stick has the test mode (not Skip). |
| Manifest parsed | `aegis-boot-test: manifest-roundtrip parsed (schema_version=N, esp_files=N, expected_pcrs=N)` | Manifest read + parsed cleanly. The numeric fields are operator-readable; the harness can grep on the "parsed" head string. |
| **Empty PCRs (current shipped behavior)** | `aegis-boot-test: manifest-roundtrip empty-pcrs (PR3-era; harness fail-opens per attestation-manifest.md contract)` | Pass-via-fail-open — no PCRs to compare against yet. Stays valid until aegis-boot starts populating `expected_pcrs[]`. |
| PCR match (post-population) | `aegis-boot-test: manifest-roundtrip pcr_index=N bank=sha256 MATCH` | One per matching entry. |
| **PCR mismatch (regression)** | `aegis-boot-test: manifest-roundtrip pcr_index=N bank=sha256 MISMATCH (expected=... live=...)` | One per drifting entry. Either the manifest doesn't reflect the current measured boot, or the boot chain has regressed. |
| PCR read failure | `aegis-boot-test: manifest-roundtrip pcr_index=N bank=sha256 READ-FAILED (...)` | Sysfs path unreadable — TPM driver problem, not a chain regression. |
| Pre-comparison failure | `aegis-boot-test: manifest-roundtrip FAILED (esp-find: ...)` / `(esp-mount: ...)` / `(read ...: ...)` / `(parse: ...)` | Couldn't get to the comparison step. Distinct head string per error stage so the harness log is self-explanatory. |

**Process exit code**: `0` for any Pass landmark (including empty-pcrs); `1` for any FAILED / MISMATCH / READ-FAILED.

**Why the empty-pcrs landmark exists**: the attestation-manifest contract pinned in [docs/attestation-manifest.md](attestation-manifest.md) keeps `schema_version` at 1 even when `expected_pcrs[]` starts being populated. Consumers (including this test mode) fail-open on the empty case, so the test mode flips from skip-via-fail-open to active-comparison the moment aegis-boot starts emitting PCR entries — no harness-side change required.

## Stability policy

Strings in the **Serial landmarks** tables are part of aegis-boot's published external contract. They follow these rules:

- **Substring-stable**: harness assertions match by `contains` — additional tokens may be appended (e.g. wrap a numeric errno value), but the head string up through the first parenthesis stays identical across releases.
- **Coordinated changes**: any rename or removal requires a paired PR on the consuming harness in the same release window. The CHANGELOG must call out the contract change.
- **No deletions without notice**: a test mode that is dropped emits a single deprecation landmark for one release before the mode itself is removed.

## Adding a new test mode

1. Add a new `pub fn run_<name>() -> i32` to `crates/rescue-tui/src/test_mode.rs`.
2. Wire it into `dispatch_from_env`'s match.
3. Document the landmarks in this file with the same table shape.
4. Add at least one unit test per landmark (the kexec call is injectable for hermetic testing — see the existing `run_kexec_unsigned_with_kexec` pattern).
5. File a follow-up against the consuming harness with a link to this commit.

Test modes are intentionally lightweight: no TUI rendering, no terminal alt-screen, no operator interaction. They're scripted serial-console output, full stop.
