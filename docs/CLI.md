# `aegis-boot` CLI reference

The `aegis-boot` binary is the operator-facing front end. It wraps the build/flash/inventory operations that previously required running shell scripts and `dd` by hand.

```
aegis-boot — Signed boot. Any ISO. Your keys.

USAGE:
  aegis-boot flash [device]     Write aegis-boot to a USB stick
  aegis-boot list [device]      Show ISOs on the stick
  aegis-boot add <iso> [device] Copy + validate an ISO
  aegis-boot doctor [--stick D] Health check (host + stick)
  aegis-boot recommend [slug]   Curated catalog of known-good ISOs
  aegis-boot fetch <slug>       Download + verify a catalog ISO
  aegis-boot --version          Print version
  aegis-boot --help             This message
```

All subcommands accept `--help` / `-h` for per-command usage.

The implementation lives in [`crates/aegis-cli`](../crates/aegis-cli) (binary name `aegis-boot`).

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
ISOs on /mnt/aegis-isos:

  [✓ sha256] [✓ minisig]    1.6 GiB  ubuntu-24.04.2-live-server-amd64.iso
  [  sha256] [  minisig]    198 MiB  alpine-3.20.3-x86_64.iso

2 ISO(s) total. Legend:
  ✓ sha256   sibling <iso>.sha256 present
  ✓ minisig  sibling <iso>.minisig present
  (missing sidecars mean the ISO will show GRAY verdict in rescue-tui)
```

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
aegis-boot doctor --help
```

### What it reports

**Host checks:**
- `operating system` — Linux today (macOS/Windows tracked in [#123](https://github.com/williamzujkowski/aegis-boot/issues/123))
- `command: dd` / `sudo` / `sgdisk` / `lsblk` — the prerequisites for `flash` and stick inspection
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
Curated ISO catalog (13 entries):

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

## Versioning

`aegis-boot --version` reports the workspace version (currently `0.12.0`). The CLI ships in lockstep with the rest of the workspace; `cargo install --path crates/aegis-cli` (or downloading a release binary) will give you a CLI that matches the on-stick rescue-tui.

## See also

- [INSTALL.md](./INSTALL.md) — operator end-to-end walkthrough
- [TROUBLESHOOTING.md](./TROUBLESHOOTING.md) — common errors
- [USB_LAYOUT.md](./USB_LAYOUT.md) — what the CLI is laying down on the stick
