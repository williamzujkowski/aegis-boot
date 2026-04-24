# `aegis-boot` CLI reference

The `aegis-boot` binary is the operator-facing front end. It wraps the build/flash/inventory operations that previously required running shell scripts and `dd` by hand.

> **Looking for the authoritative flag list?** See [`reference/CLI_SYNOPSIS.md`](./reference/CLI_SYNOPSIS.md) — an auto-generated file that captures the exact `--help` output of every subcommand. It's drift-checked against the binary in CI (Phase 3b of [#286](https://github.com/aegis-boot/aegis-boot/issues/286)). This document is the prose companion: examples, exit-code meanings, and "why this flag exists" context that the `--help` output doesn't cover.

```
aegis-boot — Signed boot. Any ISO. Your keys.

USAGE:
  aegis-boot init [device]      One-command rescue stick (flash + fetch + add)
  aegis-boot flash [device]     Write aegis-boot to a USB stick
  aegis-boot list [device]      Show ISOs on the stick
  aegis-boot add <iso> [device] Copy + validate an ISO
  aegis-boot doctor [--stick D] Health check (host + stick)
  aegis-boot recommend [slug]   Curated catalog of known-good ISOs
  aegis-boot fetch <slug>       Download + verify a catalog ISO
  aegis-boot attest [list|show] Attestation receipts for past flashes
  aegis-boot eject [device]     Safely power-off a stick before removal
  aegis-boot update <device>    Check eligibility for in-place signed-chain update
  aegis-boot verify [device]    Re-verify every ISO's sha256 against its sidecar
  aegis-boot compat [query]     Hardware compatibility lookup (verified reports only)
  aegis-boot --version [--json] Print version (--json emits schema_version=1)
  aegis-boot --help             This message
```

All subcommands accept `--help` / `-h` for per-command usage.

### `--json` mode (scripting)

The read-mostly subcommands emit a stable machine-readable document when given `--json`:

| Command             | What you get                                                                 |
| ------------------- | ---------------------------------------------------------------------------- |
| `doctor --json`     | Health rows + score + band + `next_action`                                   |
| `list --json`       | ISO inventory + per-file sha256 verdict + sidecar coverage                   |
| `attest list --json`| All recorded attestation manifests + summary                                 |
| `attest show --json`| Raw manifest verbatim (bit-for-bit reproduction of the on-disk file)         |
| `verify --json`     | Per-ISO verification results (Verified / Mismatch / Unreadable / NotPresent) |
| `recommend --json`  | Full catalog (or single entry with `recommend --json <slug>`)                |
| `update --json`     | Eligibility envelope + host-chain (sha256 per slot) or reason-for-ineligible |
| `compat --json`     | Compat DB entries (or single entry with `compat --json <query>`)             |
| `--version --json`  | `{ schema_version, tool, version }` — scriptable semver lookup               |

Every `--json` output carries `schema_version: 1` at the root so downstream tooling can detect future breaking changes.

The implementation lives in [`crates/aegis-cli`](../crates/aegis-cli) (binary name `aegis-boot`).

---

## `aegis-boot quickstart`

Shortest-path command from stick-in-hand to booted rescue-tui (#352 UX-1). A thin wrapper around `aegis-boot init --profile minimal --yes --direct-install` — preset so a net-new operator types one command and gets Alpine 3.20 Standard (~200 MiB) signed-boot-chain ready to use.

### Usage

```bash
aegis-boot quickstart /dev/sdc          # device arg required — no auto-detect
aegis-boot quickstart --help
```

The device argument is **required**. Auto-detection was explicitly rejected in the #352 consensus vote because single-candidate heuristics can misclassify a mounted USB and result in data loss.

For a different distro or a larger kit, use `aegis-boot init --profile <name>` or `aegis-boot flash /dev/sdX && aegis-boot add <slug>` (the catalog-slug form from #352 UX-4).

---

## `aegis-boot init`

One-command rescue stick. Composes `doctor → flash → fetch + add` in sequence using a named **profile** — a constant bundle of catalog slugs. The simplest path from empty stick to rescue-ready, producing a single attestation manifest that spans the entire run.

### Usage

```bash
aegis-boot init                         # auto-detect drive, panic-room profile
aegis-boot init /dev/sdc                # explicit device
aegis-boot init /dev/sdc --yes          # unattended (skips prompts)
aegis-boot init --profile panic-room    # explicit profile (same default)
aegis-boot init --help
```

### Options

| Flag               | Effect                                                                    |
| ------------------ | ------------------------------------------------------------------------- |
| `--profile <name>` | Profile to install (default: `panic-room`)                                |
| `--yes`, `-y`      | Skip interactive confirmations; destructive (overrides doctor BROKEN too) |
| `--no-doctor`      | Skip the `doctor` preflight check (not recommended)                       |
| `--no-gpg`         | Skip GPG signature verification on fetched ISOs (not recommended)         |

### What it does

1. **Doctor preflight.** Runs `aegis-boot doctor --stick <device>`. Exits if the score falls into BROKEN (any `FAIL` check) unless `--yes` is passed.
2. **Flash.** Runs `aegis-boot flash <device> --yes` — wipes, writes the signed boot image, and records a new attestation manifest.
3. **Fetch + add each ISO** in the profile. Each `fetch` is idempotent via the `$XDG_CACHE_HOME/aegis-boot/<slug>/` cache, and each `add` appends an `IsoRecord` to the single attestation manifest from step 2.

If any step fails, `init` stops immediately and prints a context line. Re-running picks up where it left off (fetch is cached; flash and add are idempotent on the same stick).

### Profiles

| Name         | ISOs (count) | Size (approx.) | Purpose                                                         |
| ------------ | ------------ | -------------- | --------------------------------------------------------------- |
| `panic-room` | 3            | ~5 GiB         | Emergency recovery kit (default)                                |
| `minimal`    | 1            | ~200 MiB       | Fastest — Alpine only                                           |
| `server`     | 3            | ~6 GiB         | Enterprise server rescue (RHEL family + Ubuntu Server)          |

**`panic-room`** contains:

- `alpine-3.20-standard` — 200 MiB, minimal, fast boot for basic rescue
- `ubuntu-24.04-live-server` — 3 GiB, familiar tooling, server-class rescue
- `rocky-9-minimal` — 2 GiB, enterprise RHEL-family rescue

Fits on a 16 GB stick with headroom for operator-added ISOs.

**`minimal`** contains:

- `alpine-3.20-standard` — 200 MiB

Use when "I just need a known-good Linux userspace to poke at this disk" and bandwidth / time / verification overhead should all be as small as possible.

**`server`** contains:

- `ubuntu-24.04-live-server` — 3 GiB
- `rocky-9-minimal` — 2 GiB
- `almalinux-9-minimal` — 1.5 GiB

For operators whose targets are servers, not laptops. No desktop / live session. All three are RHEL-family-or-Ubuntu-family enterprise minimal installers signed by a vendor our operators trust.

### Exit codes

- `0` — stick is ready with the profile installed
- `1` — step failure (doctor, flash, fetch, or add); see preceding error
- `2` — invalid arguments or unknown profile

### Example

```bash
$ aegis-boot init /dev/sdc --yes
aegis-boot init — Emergency recovery kit — Alpine 3.20 + Ubuntu 24.04 Server + Rocky 9

Plan:
  1. doctor preflight (host + stick health)
  2. flash /dev/sdc
  3. fetch + add each ISO in the profile:
       - alpine-3.20-standard
       - ubuntu-24.04-live-server
       - rocky-9-minimal

--- doctor preflight ---
...

--- flash stick ---
...

--- alpine-3.20-standard ---
Fetching Alpine Linux 3.20 Standard into ~/.cache/aegis-boot/alpine-3.20-standard
...

=== aegis-boot init: DONE ===
Profile 'panic-room' is ready on the stick (3 ISO(s) added).

Next steps:
  1. Eject: sudo sync && sudo eject /dev/sdX
  2. Boot the target machine (UEFI boot menu → USB entry).
  3. In rescue-tui, pick an ISO and press Enter.

Inspect attestation: aegis-boot list
```

---

## `aegis-boot flash`

Writes a freshly built `aegis-boot.img` to a USB stick.

### Usage

```bash
sudo aegis-boot flash             # auto-detect removable drives
sudo aegis-boot flash /dev/sdc    # explicit device
sudo aegis-boot flash --help
```

### What it does

1. **Drive detection** — scans `/sys/block/sd*` for devices with `removable=1`. Skips NVMe, loop, and any system drive that isn't flagged removable. Reads model + size from sysfs.
2. **Selection prompt** — shows numbered list, asks `[Y/n]` if exactly one drive, `[1-N]` otherwise. Pressing Enter on the single-drive prompt accepts.
3. **Typed confirmation** — requires you to type the literal string `flash`. `y`, `yes`, `Y` are *not* accepted. This is intentional friction because `dd` to the wrong device destroys it.
4. **Build** — invokes `scripts/mkusb.sh` with `OUT_DIR=<repo>/out` and `DISK_SIZE_MB=<full stick capacity>`.
5. **Write** — invokes `sudo dd if=out/aegis-boot.img of=/dev/sdX bs=4M oflag=direct conv=fsync status=progress`.
6. **Sync + partprobe** — flushes caches, asks the kernel to re-read the partition table.

### Exit codes

- `0` — success
- `1` — drive not found, build failed, or `dd` failed
- `2` — usage error (unknown subcommand)

### Requirements

- Repo root present (the binary `find_repo_root()`s for `Cargo.toml + crates/`). For now the CLI assumes the repo is on disk; standalone packaging is tracked separately.
- `bash`, `sudo`, `dd`, `partprobe`, plus all the build prereqs in [BUILDING.md](../BUILDING.md).

---

## `aegis-boot list`

Inventories ISOs on the stick.

### Usage

```bash
aegis-boot list                       # auto-find mounted AEGIS_ISOS
aegis-boot list /dev/sdc              # mount partition 2 of device, list, unmount
aegis-boot list /mnt/aegis-isos       # use existing mount path
aegis-boot list --help
```

### What it does

1. **Mount resolution** — see [Mount resolution rules](#mount-resolution-rules) below.
2. **Scan** — reads the mount directory, separates `.iso` files (case-insensitive on the extension) from sidecars.
3. **Pair** — for each ISO, checks for sibling `<iso>.sha256` / `<iso>.SHA256SUMS` (either counts) and `<iso>.minisig`.
4. **Print** — table with `[✓ sha256] [✓ minisig]  size  name` rows, sorted by name.

If the CLI mounted the partition itself (`temporary: true`), it unmounts on exit.

### Output sample

```
Attestation:
  flashed   : 2026-04-16T13:30:00Z by william
  ISOs added: 1 recorded since flash
  manifest  : /home/william/.local/share/aegis-boot/attestations/e1ae0864-...-2026-04-16T13-30-00Z.json

ISOs on /mnt/aegis-isos:

  [✓ sha256] [✓ minisig]    1.6 GiB  ubuntu-24.04.2-live-server-amd64.iso
  [  sha256] [  minisig]    198 MiB  alpine-3.20.3-x86_64.iso

2 ISO(s) total. Legend:
  ✓ sha256   sibling <iso>.sha256 present
  ✓ minisig  sibling <iso>.minisig present
  (missing sidecars mean the ISO will show GRAY verdict in rescue-tui)
```

The attestation header is shown only when an attestation matching this stick is on disk (matched by GPT disk GUID). Silent on miss — operators may have flashed the stick on a different host.

---

## `aegis-boot add`

Copies an ISO + any sibling sidecars onto the stick.

### Usage

```bash
aegis-boot add ~/Downloads/ubuntu.iso             # auto-find mount
aegis-boot add ~/Downloads/ubuntu.iso /dev/sdc    # mount + copy + unmount
aegis-boot add ~/Downloads/ubuntu.iso /mnt/aegis-isos
aegis-boot add --help
```

### What it does

1. **Validate source** — file must exist and be readable.
2. **Mount resolution** — same rules as `list`.
3. **Free-space check** — requires `iso_size + 10 MiB headroom` available on the target. Refuses (exit 1) if not, before touching anything.
4. **Copy ISO** — `sudo cp <src> <mount>/<basename>`.
5. **Copy sidecars** — for each of `.sha256`, `.SHA256SUMS`, `.minisig`, if `<src>.<ext>` exists, copy it to `<mount>/<basename>.<ext>`. Reports the count.
6. **`sync`** — flush before the (possible) auto-unmount.

### Sidecar conventions

| Suffix | Purpose | Format |
|---|---|---|
| `<iso>.sha256` | Single-file SHA-256 checksum | `<hex>  <iso>` |
| `<iso>.SHA256SUMS` | Multi-file checksums (some distros publish this form) | `<hex>  <filename>` per line |
| `<iso>.minisig` | minisign signature | minisign format |

If no sidecar is found, the operator gets an explicit notice in the output, and `rescue-tui` will show GRAY verdict + require a typed `boot` confirmation at boot time. We never silently accept an unverified ISO.

### `--scan` — retroactive sidecar generation (#479)

When an operator drag-and-drops `.iso` files onto `AEGIS_ISOS` from their host OS, those ISOs show up in rescue-tui as tier 2 (BareUnverified) — bootable, but requiring a typed-confirmation challenge. Running:

```bash
sudo aegis-boot add --scan /dev/sda2        # device path
sudo aegis-boot add --scan /mnt/aegis-isos  # mount path
sudo aegis-boot add --scan                  # auto-detect AEGIS_ISOS
```

walks the mount for `.iso` files, streams sha256 for each bare ISO, and writes coreutils-compatible `<iso>.sha256` sidecars. Output:

```
Scanning /mnt/aegis-isos for ISOs without sidecars...
  [✓] ubuntu-24.04-live-server.iso  (2.4 GiB) — sha256 written
  [✓] alpine-standard-3.20.iso  (200 MiB) — sha256 written
  [-] debian-12.iso — already verified; skipped
  [!] my-custom.iso — existing .sha256 MISMATCH (expected a1b2…, actual 0123…); NOT overwritten
  [~] ubuntu-24.04-live-server.iso — no .minisig (tier-1 requires operator's signing key; stays at tier-2+)
Done: 2 upgraded, 1 already verified, 1 tamper-flagged, 2 missing minisig (of 4 ISOs).
```

**Safety invariants:**

- **Never overwrites an existing sidecar.** A mismatch between the declared hash and the computed one is surfaced as a tamper signal — inspect the ISO before booting.
- **Atomic writes.** Sidecars go through write-to-temp-then-rename so a half-written file never lands on the stick.
- **No minisig generation.** Creating a minisig sidecar would require the operator's private signing key, which aegis-boot never touches. Tier-3 (KeyNotTrusted) ISOs stay at tier-3 until the operator signs them out-of-band.
- **Per-ISO attestation.** Each newly-upgraded ISO gets an attestation entry under `~/.local/share/aegis-boot/attestations/` — same audit trail as a full `aegis-boot add`.

---

## Mount resolution rules

Both `list` and `add` use the same logic to figure out where `AEGIS_ISOS` is:

| Argument given | Behavior |
|---|---|
| (none) | Read `/proc/mounts`; find a line whose mount point contains `AEGIS_ISOS`. Use that. |
| `/dev/sdX` | Use partition 2 (`/dev/sdX2`). Mount it to a tempdir under `/tmp` with `-t vfat -o rw,codepage=437,iocharset=cp437`. Unmount on exit. |
| `/some/path` (existing dir) | Use as-is. Don't mount or unmount. |
| `/some/path` (does not exist) | Error. |

Why the explicit `codepage=437,iocharset=cp437`? Because the kernel's default `iocharset=utf8` is a separate module (`nls_utf8`) that we *do* ship in the rescue initramfs but is not always loaded on the operator's host kernel. Using cp437 avoids the dependency on the workstation side. The on-stick filenames are still readable from any modern host.

---

## `aegis-boot doctor`

Diagnostic health check for both the host workstation and (optionally) an aegis-boot stick.

### Usage

```bash
aegis-boot doctor                       # auto-detect a single removable drive
aegis-boot doctor --stick /dev/sdc      # inspect a specific drive
aegis-boot doctor --json                # schema_version=1 machine-readable output
aegis-boot doctor --help
```

### What it reports

**Host checks:**
- `operating system` — Linux today (macOS/Windows tracked in [#123](https://github.com/aegis-boot/aegis-boot/issues/123))
- `machine identity` — vendor + model + firmware read from `/sys/class/dmi/id/*` (Linux only). Informational — gives operators filing a `hardware-report` the exact strings to paste.
- `compat DB coverage` — cross-checks the DMI identity against the in-binary `COMPAT_DB`. Pass = documented, Warn = not yet in the DB with a link to the hardware-report template.
- `command: dd` / `sudo` / `sgdisk` / `lsblk` — the prerequisites for `flash` and stick inspection
- `command: curl` / `sha256sum` / `gpg` — prerequisites for `aegis-boot fetch`
- `Secure Boot (host)` — `mokutil --sb-state` first, falling back to reading `/sys/firmware/efi/efivars/SecureBoot-*` directly
- `removable USB drives` — list / count

**Stick checks (when a drive is provided or auto-detected):**
- `partition table` — runs `sgdisk -p` and verifies the GPT contains both an ESP and an `AEGIS_ISOS` partition
- `AEGIS_ISOS contents` — if mounted, counts ISOs + sidecars; warns if no sidecars present (TUI verdict will be GRAY)

### Output

```
aegis-boot doctor — host + stick health check

Host checks:
  [✓ PASS] operating system                  Linux (supported)
  [✓ PASS] machine identity                  Framework Laptop (A6) — firmware: INSYDE Corp. 03.19 (09/18/2025)
  [! WARN] compat DB coverage                not yet in compat DB — file a report at https://…/hardware-report.yml
  [✓ PASS] command: dd                       /usr/bin/dd (required to write the stick)
  [✓ PASS] command: sudo                     /usr/bin/sudo (required for dd / mount)
  [✓ PASS] command: sgdisk                   /usr/sbin/sgdisk (verifies stick partition table after flash)
  [✓ PASS] command: lsblk                    /usr/bin/lsblk (lists removable drives for `flash` auto-detect)
  [! WARN] Secure Boot (host)                disabled on this host (target machine SB state is what matters)
  [✓ PASS] removable USB drives              /dev/sda (Cruzer, 29.8 GB)

Stick checks:
  [✓ PASS] partition table: /dev/sda         GPT with ESP + AEGIS_ISOS partitions — looks like an aegis-boot stick
  [! WARN] AEGIS_ISOS contents               2 ISO(s), no sidecars — TUI will show GRAY verdict

  Health score: 93/100 (EXCELLENT)
```

### Exit codes

- `0` — healthy (PASS or only WARN items)
- `1` — at least one FAIL — the report ends with a `NEXT ACTION` line telling the operator what to do
- `2` — usage error (unknown flag etc.)

### Score weighting

PASS = 10 points / WARN = 7 points / FAIL = 0 points / SKIP = not counted. Final score is `weight * 100 / total`, rounded. Bands: 90+ EXCELLENT, 70+ OK, 40+ DEGRADED, below 40 BROKEN.

The `NEXT ACTION` line is set by the *first* FAIL row that has one — it's the single most important thing to fix before retrying.

---

---

## `aegis-boot recommend`

Browse the curated catalog of known-good ISOs that have been validated (or are vouched for by the project) under aegis-boot. Catalog entries point at the project's own canonical download URL + signed SHA256SUMS, so the trust anchor stays with the upstream project — aegis-boot just curates and helps you find the recipe.

### Usage

```bash
aegis-boot recommend                       # browse the table
aegis-boot recommend ubuntu-24.04-live-server   # show download + verify recipe
aegis-boot recommend ubuntu                # prefix match (only if unambiguous)
aegis-boot recommend --help
```

### Table view

```
Curated ISO catalog (6 entries):

  SLUG                          NAME                                       SIZE  SECURE BOOT
  ----------------------------  --------------------------------------  -------  ----------------------------
  alpine-3.20-standard          Alpine Linux 3.20 Standard              198 MiB  ✗ unsigned (MOK needed)
  archlinux-current             Arch Linux (current monthly)            1.2 GiB  ✗ unsigned (MOK needed)
  clonezilla-live-stable        Clonezilla Live (stable)                380 MiB  ✓ signed (Clonezilla / DRBL)
  ...
  ubuntu-24.04-live-server      Ubuntu Server 24.04.2 LTS               2.5 GiB  ✓ signed (Canonical CA)
  ubuntu-24.04-desktop          Ubuntu Desktop 24.04.2 LTS              5.7 GiB  ✓ signed (Canonical CA)
```

The `SECURE BOOT` column tells you whether the ISO's kernel will boot under enforcing Secure Boot without operator intervention:
- ✓ **signed** — boots; the named CA is in shim's built-in keyring
- ✗ **unsigned (MOK needed)** — boots only after the operator MOK-enrolls the distro's signing key (see [UNSIGNED_KERNEL.md](./UNSIGNED_KERNEL.md))

### Detail view (single entry)

`aegis-boot recommend <slug>` prints the project's canonical download URL, the URL of the project's signed `SHA256SUMS`, the URL of the GPG/minisign signature on `SHA256SUMS`, and a copy-pasteable recipe to download + verify + add to your stick.

For unsigned-kernel entries (Alpine / Arch / NixOS), the recipe also includes the MOK-key placement step.

### Why no SHA-256 in the catalog?

Distros release point versions on a cadence that doesn't track our commits. Pinning a hash in the catalog would make most entries wrong within weeks of every release. The catalog points at the project's *signed* SHA256SUMS instead — whoever the project trusts to sign their releases is who we trust here. The trust anchor is the project's release-signing key, not aegis-boot's catalog.

### Adding a new entry

The curation criteria + how-to-propose process are documented separately in [docs/CATALOG_POLICY.md](./CATALOG_POLICY.md). Short version: HTTPS-served canonical URL, project-published signed SHA256SUMS, real operator value, stable URL, honest SB posture.

### See also: `aegis-boot fetch <slug>`

The manual recipe (curl, gpg, sha256sum, then `aegis-boot add`) is shipped — but the next section describes `aegis-boot fetch <slug>`, which automates the same recipe end-to-end.

---

## `aegis-boot fetch`

Downloads + verifies a catalog ISO. Resolves a slug from the catalog, downloads the ISO + the project's signed `SHA256SUMS` + the GPG signature on `SHA256SUMS`, runs `sha256sum -c` against the ISO, runs `gpg --verify` on the signature, and reports the verified path.

### Usage

```bash
aegis-boot fetch ubuntu-24.04-live-server
aegis-boot fetch --out ~/Downloads alpine-3.20-standard
aegis-boot fetch --no-gpg ubuntu-24.04-live-server   # SHA-256 only (NOT recommended)
aegis-boot fetch --help
```

### What it does

1. **Slug → catalog entry**: same lookup as `recommend` (exact + unique-prefix).
2. **Download** the ISO, SHA256SUMS, and SHA256SUMS signature into a per-slug cache directory (`$XDG_CACHE_HOME/aegis-boot/<slug>/` by default; `--out` overrides). Skips files already present so re-runs are cheap.
3. **SHA-256 verification**: runs `sha256sum -c <SHA256SUMS> --ignore-missing` and asserts the line for our specific ISO ends with `: OK`.
4. **GPG signature verification**: runs `gpg --verify <SHA256SUMS.sig> <SHA256SUMS>` and reports one of:
   - **OK** — signature valid against a key in your keyring
   - **Unknown key** — signature present, signer not yet trusted; gpg's full output is shown so you can decide whether to import the key. Non-fatal: `aegis-boot fetch` exits 0 because the SHA-256 itself was valid against the project-published checksum file.
   - **BAD signature** — fatal; the SHA256SUMS file appears tampered. Exit 1.
   - **gpg missing** — fatal; install hint shown. Exit 1 (or pass `--no-gpg`).
5. **Print the `aegis-boot add` line** with the absolute ISO path. Does NOT auto-add — operator may want a specific stick.

For unsigned-kernel entries (Alpine, Arch, NixOS) the success message also reminds the operator to place the distro's signing public key on the stick post-add.

### Why shell out to system tools?

`fetch` calls `curl`, `sha256sum`, and `gpg` via `Command` rather than pulling in `reqwest` + `sha2` + `gpgme` as Rust deps. Trade-offs:
- **+** Static-musl binary stays small (~855 KiB).
- **+** Trust boundary is explicit and inspectable — operators see what's invoked; `aegis-boot doctor` reports prerequisite tools.
- **−** Operators need curl + sha256sum + gpg installed (universal on Linux distros).

### Exit codes

- `0` — verified ISO ready to add (including the unknown-key GPG case)
- `1` — download / verification failed
- `2` — usage error

---

---

## `aegis-boot attest`

Attestation receipts for flashed sticks. Every `aegis-boot flash` writes a JSON manifest recording exactly what went onto the stick (image SHA-256, target device + GUID + size + model, image size) and the host environment that wrote it (operator, kernel, Secure Boot state, timestamp). Manifests live in `$XDG_DATA_HOME/aegis-boot/attestations/` (or `~/.local/share/aegis-boot/attestations/`).

### Usage

```bash
aegis-boot attest list                          # list all stored attestations
aegis-boot attest show <FILE>                   # pretty-print one
aegis-boot attest --help
```

### Why

Every other USB-imaging tool is silent after flash. The attestation receipt is the artifact that operationalizes aegis-boot's "prove what's on the stick" claim:

- **Forensics / IR** gets chain-of-custody. "What was on this stick when it was deployed?" is a cryptographic-grade question, not a vibes question.
- **Sysadmin fleets** get per-stick inventory. The manifest filename includes the disk GUID, so 200 sticks from a school-district refresh produce 200 retrievable receipts.
- **Security review** gets an audit trail. Whose user account on which workstation flashed which image with which Secure Boot state when?

### Manifest schema (v1)

```json
{
  "schema_version": 1,
  "tool_version": "0.17.0",
  "flashed_at": "2026-04-16T12:34:56Z",
  "operator": "william",
  "host": {
    "kernel": "Linux 6.17.0-20-generic",
    "secure_boot": "disabled"
  },
  "target": {
    "device": "/dev/sdc",
    "model": "SanDisk Cruzer Blade",
    "size_bytes": 32010240000,
    "image_sha256": "abcdef...",
    "image_size_bytes": 1073741824,
    "disk_guid": "7DD588C9-3A85-48CF-822F-BFBC4D8DD784"
  },
  "isos": []
}
```

Forward-compatibility: unknown fields are tolerated by the parser. Future schema versions are additive; breaking changes bump `schema_version`.

### Cryptographic signing

v1 manifests are unsigned. The trust anchor is "you ran this command on this host, the timestamps and hashes are evidence." Cryptographic signing — TPM PCR attestation + minisign — is tracked under [epic #139](https://github.com/aegis-boot/aegis-boot/issues/139) and will land alongside the TPM measured-boot work as additional fields, not a schema rewrite.

### What's NOT in v1 (deferred)

- **On-stick copy** at `/EFI/aegis-attestation.json`: needs an ESP-mount step after `dd`. Tracked.
- **`aegis-boot attest verify`**: depends on the signing scheme above.

### Append on `aegis-boot add`

When `aegis-boot add` succeeds, it appends an `IsoRecord` to the matching attestation:

```json
{
  "filename": "ubuntu-24.04.2-live-server-amd64.iso",
  "sha256": "abcdef...",
  "size_bytes": 2724333568,
  "sidecars": ["sha256", "minisig"],
  "added_at": "2026-04-16T13:00:00Z"
}
```

Matching logic: the destination mount path → owning device (from `/proc/mounts`) → strip partition suffix (`/dev/sdc2 → /dev/sdc`, `/dev/nvme0n1p3 → /dev/nvme0n1`) → disk GUID via `sgdisk -p` → newest manifest in the attestations dir whose filename starts with that GUID. If GUID can't be resolved, falls back to "most recent attestation overall" with a warning (correct for the common single-stick workflow; ambiguous in multi-stick sessions). Failure to update the attestation does NOT fail the add.

---

## `aegis-boot eject`

Safely power-off a USB stick before physical removal. Bundles the `sync + blockdev --flushbufs + udisksctl power-off / eject` recipe into one command. Pulling a stick without syncing can leave the `AEGIS_ISOS` exFAT / FAT32 / ext4 state dirty, which downstream presents as "file ends mid-ISO" on the next boot or sha256 mismatch during verification.

### Usage

```bash
aegis-boot eject                    # auto-detect removable drive
aegis-boot eject /dev/sdc           # explicit device
aegis-boot eject --help
```

### Behavior

1. **`sync`** — flush filesystem-level dirty buffers (specifically against the target device if supported, else global).
2. **`sudo -n blockdev --flushbufs`** — flush the block-device cache. Skipped with a warning if sudo is unavailable; the sync in step 1 still ran.
3. **`udisksctl power-off`** (if installed) — polkit-friendly power-off, no sudo needed. Falls back to `eject /dev/sdX` if udisksctl isn't present.

On success: "Done. Safe to remove /dev/sdX."

If step 3 fails, the stick is still synced and safe to remove — the CLI prints the manual recipe (`sudo eject` or `udisksctl power-off -b`) for the operator to finish by hand.

### Exit codes

- `0` — synced + powered-off
- `1` — could not auto power-off (stick still safe to remove — CLI prints fallback recipe)
- `2` — invalid arguments

### Why not force-unmount?

Force-unmount of a busy partition (fuser / lsof-integrated) is deliberately out of scope. If the AEGIS_ISOS partition is busy, the operator needs to know why (a file manager has it open? a background rescan?) — silently forcing the unmount would mask the cause and risk data loss.

---

## `aegis-boot update`

Check whether a stick is eligible for a non-destructive in-place signed-chain update. Today this is an eligibility check only — the atomic file-replace step is tracked under [#181](https://github.com/aegis-boot/aegis-boot/issues/181).

### Usage

```bash
sudo aegis-boot update /dev/sdc        # human-readable report
sudo aegis-boot update /dev/sdc --json # schema_version=1 machine-readable
aegis-boot update --help
```

### What it reports

- **Disk GUID + attestation-manifest match.** The stick is eligible only if its GPT disk GUID is the same one recorded in the last attestation manifest — this catches case of "operator picked the wrong device."
- **Host-chain preview.** Resolves `SHIM_SRC` / `GRUB_SRC` / `KERNEL_SRC` / `INITRD_SRC` from the host (the same defaults `mkusb.sh` uses) and reports the sha256 of each slot. Operators see what *would* be replaced before opting in.
- **Ineligibility reason.** When ineligible, names the specific mismatch (device missing / no attestation / GUID differs / …).

### Exit codes

- `0` — eligible; host-chain preview printed
- `1` — ineligible; `--json` carries the exact reason
- `2` — usage error

---

## `aegis-boot verify`

Re-run sha256 verification on every ISO on the stick against its `.sha256` sidecar. Complements the TUI's per-ISO verdict column with a batch audit mode.

### Usage

```bash
aegis-boot verify                      # auto-find mounted AEGIS_ISOS
aegis-boot verify /dev/sdc             # mount partition 2, verify, unmount
aegis-boot verify /mnt/aegis-isos      # use existing mount path
aegis-boot verify --json               # schema_version=1 machine-readable
aegis-boot verify --help
```

### Verdict states

Each ISO resolves to exactly one verdict:

- **Verified** — sha256 matches a sidecar's expected value (sidecar type recorded: `.sha256`, `SHA256SUMS`, or `SHA256SUMS.gpg`)
- **Mismatch** — sha256 differs from sidecar; ISO is compromised or sidecar is stale
- **Unreadable** — sidecar exists but couldn't be parsed / ISO can't be read; operator must check the filesystem
- **NotPresent** — no sidecar at all; TUI shows GRAY verdict, operator must fetch a fresh copy

### Exit codes

- `0` — every ISO verified (including NotPresent-only)
- `1` — one or more Mismatch or Unreadable verdicts — audit trail is broken

---

## `aegis-boot bug-report`

Generate a ready-to-paste bug-report bundle. Composes `aegis-boot doctor --json` + DMI + removable-drive detection + new system captures (`uname`, `/proc/cmdline`, filtered `lsmod`, `dmesg` tail, `lspci`, `lsusb`, `lsblk`) into a single markdown or JSON document. Privacy-preserving by default — hostname, username, DMI and drive serials are replaced with deterministic synthetic tokens (`host-ab12cd`, `serial-ef34gh`).

### Usage

```bash
aegis-boot bug-report                       # markdown to stdout (redacted)
aegis-boot bug-report --output report.md    # file; format inferred from extension
aegis-boot bug-report --output r.json       # JSON envelope (schema_version=1)
aegis-boot bug-report --format json         # force JSON regardless of extension
aegis-boot bug-report --dump-mapping map.tsv  # write real↔synthetic map locally
aegis-boot bug-report --help
```

### Privacy model

Redaction is ON by default. Affected fields:

- hostname (`/etc/hostname`, `$HOSTNAME`) → `host-<6hex>`
- username (`$USER`) → `user-<6hex>`
- DMI / drive serials → `serial-<6hex>`

Unredacted fields (by design — correlating per-vendor behavior is the whole point):

- DMI vendor / product / BIOS version / BIOS date
- kernel version, module names, `/proc/cmdline`
- `aegis-boot` version and every `doctor` verdict

Turning redaction off requires both `--no-redact` AND the explicit `--i-accept-pii-in-output` confirmation flag. Without the confirmation, the tool exits 2 with a PII warning.

`--dump-mapping PATH` writes the real↔synthetic mapping to a local file. **Never share that file** — it de-anonymizes any bundle generated in the same run. The mapping is not persisted to disk unless you explicitly pass `--dump-mapping`.

### Output envelope

```json
{
  "schema_version": 1,
  "aegis_boot_version": "0.15.0",
  "generated_at": "2026-04-20T12:34:56Z",
  "redacted": true,
  "system": { "os_release_pretty": "...", "uname": "...", "hostname": "host-ab12cd", "user": "user-ef34gh" },
  "firmware": { "sys_vendor": "...", "product_name": "...", "bios_version": "...", ... },
  "kernel": { "cmdline": "...", "modules_storage_usb": ["..."], "dmesg_tail": ["..."] },
  "storage": { "removable_drives": ["..."], "lsblk": "...", "lsusb": "...", "lspci_storage": "..." },
  "aegis_state": { "doctor_score": 96, "doctor_band": "EXCELLENT", "doctor_rows": [...] }
}
```

### `--include-stick PATH` (#342 Phase 3a)

When the operator experiences a rescue-tui failure on a target machine, rescue-tui writes anonymous Tier-A microreports to `AEGIS_ISOS/aegis-boot-logs/` ([#342 Phase 2](https://github.com/aegis-boot/aegis-boot/issues/342)). To include them in the bundle, plug the stick back into your workstation and pass `--include-stick` pointing at its mount:

```bash
aegis-boot bug-report --include-stick /media/$USER/AEGIS_ISOS
```

Each Tier-A log is anonymous by construction (vendor family + BIOS year + classified failure class + opaque SHA-256 of the full error text) so no extra redaction gate is needed. Malformed log files are silently skipped with a stderr warning; missing `aegis-boot-logs/` directory is a no-op with an informational message.

### Deferred to later phases of #342

- Clipboard output (`wl-copy` / `xclip` / `pbcopy`)
- `tar.zst` bundle
- Phase 3b: rescue-tui consent screen + Tier B full structured log
- `--sign` — cosign keyless attestation bundle over the report

### Exit codes

- `0` — bundle written successfully
- `1` — I/O error (couldn't read source, couldn't write output)
- `2` — usage error (unknown flag, `--no-redact` without confirmation)

---

## `aegis-boot compat`

Look up a machine in the in-binary hardware-compatibility database. Every row is a verified outcome filed against real hardware or the QEMU reference environment — no speculation.

### Usage

```bash
aegis-boot compat                      # full table
aegis-boot compat thinkpad             # fuzzy match vendor or model
aegis-boot compat "q35 ovmf"           # whitespace-tokenized, all tokens must match
aegis-boot compat --json               # schema_version=1 full catalog
aegis-boot compat --json thinkpad      # schema_version=1 single entry
aegis-boot compat --help
```

### Levels

- **verified** (`✓`) — full `flash → boot → kexec` chain under enforcing Secure Boot with a signed distro
- **partial** (`~`) — chain mostly works but one step has a caveat (reserved for future community reports)
- **reference** (`≡`) — virtualized (QEMU + OVMF), the CI floor — not a physical-hardware claim

### Feedback loop with `doctor`

`aegis-boot doctor` reads DMI identity from `/sys/class/dmi/id/*` and cross-checks the in-binary DB automatically. If your machine is undocumented, `doctor` will emit a WARN row with a direct link to the [hardware-report issue template](../.github/ISSUE_TEMPLATE/hardware-report.yml).

### Exit codes

- `0` — match found or full table printed
- `1` — no match for the given query (stderr prints the report-template URL)

---

## `aegis-boot fetch-image`

Download a released `aegis-boot.img` (or `aegis-boot-hybrid.iso`) from GitHub Releases, verify its **[Sigstore cosign](https://docs.sigstore.dev/) keyless signature** against this repo's release workflow identity, and place it in the operator's chosen output directory. Intended for the "I just want to flash without building from source" path — paired with `aegis-boot flash <device> --image <path>` it closes the build-from-source loop for unprivileged operators.

Verification uses the same cosign-keyless chain as every other aegis-boot release artifact: `cosign verify-blob --certificate-identity-regexp '^https://github\.com/aegis-boot/aegis-boot/.+@refs/tags/v.+$' --certificate-oidc-issuer 'https://token.actions.githubusercontent.com'`. If `cosign` isn't on `PATH` the command fails closed with a clear diagnostic — see [crates/aegis-cli/src/fetch_image.rs](../crates/aegis-cli/src/fetch_image.rs) for the canonical invocation. Operators preferring manual verification run the equivalent recipe from [docs/RELEASE_NOTES_FOOTER.md](RELEASE_NOTES_FOOTER.md) against the downloaded file.

### Usage

```bash
aegis-boot fetch-image                         # latest release, cwd output
aegis-boot fetch-image --version v0.15.0       # pin a specific release
aegis-boot fetch-image --out ~/Downloads       # override output dir
aegis-boot fetch-image --format iso            # hybrid .iso instead of .img
aegis-boot fetch-image --dry-run --json        # resolve + print plan; no download
aegis-boot fetch-image --help
```

### Behavior

- Resolves the release tag (defaults to `latest`) against `api.github.com/repos/.../releases`.
- Downloads both the artifact and its `.minisig` detached signature.
- Verifies the signature using the release pubkey bundled in the binary — a failed verification aborts with non-zero before the artifact is placed.
- On success, writes both files (asset + `.minisig`) to the output dir and prints the full local paths.

### Exit codes

- `0` — fetched + verified
- `1` — signature verification failed, or the release tag didn't expose the expected asset
- `2` — usage error (bad flag combination)

---

## `aegis-boot completions`

Emit shell completion scripts for `bash`, `zsh`, or `fish` on stdout. Pipe to the appropriate completion directory for your shell.

### Usage

```bash
aegis-boot completions bash > /etc/bash_completion.d/aegis-boot
aegis-boot completions zsh  > "${fpath[1]}/_aegis-boot"
aegis-boot completions fish > ~/.config/fish/completions/aegis-boot.fish
```

### Exit codes

- `0` — script emitted
- `2` — unknown or missing shell argument

---

## `aegis-boot man`

Emit the rendered man page on stdout. Mostly useful for `aegis-boot man | less` on hosts where the page hasn't been installed into `/usr/share/man`.

### Usage

```bash
aegis-boot man | less
aegis-boot man > /tmp/aegis-boot.1   # save for later viewing with `man -l`
```

The body is the same man page rendered at build time from [`man/aegis-boot.1.in`](../man/aegis-boot.1.in) with the current version + release date substituted (Phase 1b of [#286](https://github.com/aegis-boot/aegis-boot/issues/286)).

### Exit codes

- `0` — always (the page is embedded at compile time; there is nothing to fail on)

---

## `aegis-boot tour`

Run the 30-second in-terminal product tour — a scripted walkthrough of what `init`, `flash`, `list`, and `doctor` do without actually touching a device. Intended as the "show me around" entrypoint for operators evaluating aegis-boot before committing to flashing a real stick.

### Usage

```bash
aegis-boot tour
aegis-boot tour --help
```

### Exit codes

- `0` — tour completed (or the user hit Ctrl-C cleanly)

---

## Versioning

`aegis-boot --version` reports the workspace version (currently `0.17.0`). The CLI ships in lockstep with the rest of the workspace; `cargo install --path crates/aegis-cli` (or downloading a release binary) will give you a CLI that matches the on-stick rescue-tui.

`aegis-boot --version --json` emits the same info as a stable envelope for scripted install verification or Homebrew-style version matching:

```json
{
  "schema_version": 1,
  "tool": "aegis-boot",
  "version": "0.17.0"
}
```

The wire shape is defined by [`aegis-wire-formats::Version`](../crates/aegis-wire-formats/src/lib.rs) and pinned via JSON Schema at [`reference/schemas/aegis-boot-version.schema.json`](./reference/schemas/aegis-boot-version.schema.json) (Phase 4b-1 of [#286](https://github.com/aegis-boot/aegis-boot/issues/286)). Scripted consumers that parsed the previous single-line output with a JSON library see no shape change — only whitespace differs.

## See also

- [INSTALL.md](./INSTALL.md) — operator end-to-end walkthrough
- [TROUBLESHOOTING.md](./TROUBLESHOOTING.md) — common errors
- [USB_LAYOUT.md](./USB_LAYOUT.md) — what the CLI is laying down on the stick
- [HARDWARE_COMPAT.md](./HARDWARE_COMPAT.md) — curated community compat table (`aegis-boot compat` mirrors this)
