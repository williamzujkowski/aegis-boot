# `aegis-boot` CLI reference

The `aegis-boot` binary is the operator-facing front end. It wraps the build/flash/inventory operations that previously required running shell scripts and `dd` by hand.

```
aegis-boot — Signed boot. Any ISO. Your keys.

USAGE:
  aegis-boot flash [device]    Write aegis-boot to a USB stick
  aegis-boot list [device]     Show ISOs on the stick
  aegis-boot add <iso> [device] Copy + validate an ISO
  aegis-boot --version         Print version
  aegis-boot --help            This message
```

All three subcommands accept `--help` / `-h` for per-command usage.

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

## Versioning

`aegis-boot --version` reports the workspace version (currently `0.12.0`). The CLI ships in lockstep with the rest of the workspace; `cargo install --path crates/aegis-cli` (or downloading a release binary) will give you a CLI that matches the on-stick rescue-tui.

## See also

- [INSTALL.md](./INSTALL.md) — operator end-to-end walkthrough
- [TROUBLESHOOTING.md](./TROUBLESHOOTING.md) — common errors
- [USB_LAYOUT.md](./USB_LAYOUT.md) — what the CLI is laying down on the stick
