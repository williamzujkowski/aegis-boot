# Operator install guide

End-to-end: get aegis-boot onto a USB stick, drop ISOs onto it, boot a target machine, and pick an ISO to kexec into. Reading time: ~5 minutes. Hands-on time: ~10 minutes plus dd.

This guide assumes Linux on the operator workstation (where you write the stick). macOS / Windows flashing is tracked in [#123](https://github.com/aegis-boot/aegis-boot/issues/123). A native macOS arm64 (Apple Silicon) CLI binary is shipped with each release as of Phase A1 of [#365](https://github.com/aegis-boot/aegis-boot/issues/365) — see [§ macOS (Apple Silicon)](#macos-apple-silicon) below.

## Before you start

Have on hand:
- A USB stick (8 GB or larger recommended; aegis-boot itself fits in 2 GB but you want headroom for ISOs)
- One or more `.iso` files you want to be able to boot
- A target machine with UEFI Secure Boot **enforcing** (the whole point of aegis-boot — if you've disabled SB, you're paying for protections you're not getting)
- `sudo` on your workstation (we shell out to `dd`, `mount`, `umount`)

## Step 0 — install the operator CLI

Pick your channel:

```bash
# Option A — cosign-verified install one-liner
curl -sSL https://raw.githubusercontent.com/aegis-boot/aegis-boot/main/scripts/install.sh | sh

# Option B — Homebrew (macOS Apple Silicon only)
brew tap aegis-boot/aegis-boot https://github.com/aegis-boot/aegis-boot
brew install aegis-boot

# Option C — build from source (any platform with Rust 1.88+)
cargo install --git https://github.com/aegis-boot/aegis-boot --bin aegis-boot --path crates/aegis-cli
```

Option A (`curl | sh`) works on Linux + macOS. Option B (Homebrew) is macOS-only — Linux operators have better native channels (apt/dnf/cargo/curl-sh); the Linux brew bottle was dropped per a 2026-04-24 consensus vote on brew-shrink.

Option A downloads the latest release's `aegis-boot-x86_64-linux` (or `aegis-boot-aarch64-apple-darwin` on macOS arm64) static binary, verifies its Sigstore cosign signature against this repo's `release.yml` workflow identity, and installs to `/usr/local/bin` (root) or `~/.local/bin` (non-root). The installer itself does NOT need root unless you're installing to `/usr/local/bin`. To inspect first: `curl -sSL ... -o install.sh && less install.sh && sh install.sh`.

Option B (Homebrew) auto-installs the Brew-tracked runtime deps (`curl`, `gnupg`, `gptfdisk`). Runs the same cosign-verifiable macOS arm64 binary; verify the cosign signature manually if you want to confirm — see [Formula/README.md](../Formula/README.md).

### macOS (Apple Silicon)

A native `aegis-boot-aarch64-apple-darwin` binary ships with every release (Phase A1 of [#365](https://github.com/aegis-boot/aegis-boot/issues/365)). Install via Homebrew (recommended) or direct download:

```bash
# Option A — Homebrew (no Gatekeeper interaction)
brew tap aegis-boot/aegis-boot https://github.com/aegis-boot/aegis-boot
brew install aegis-boot

# Option B — direct download (see Gatekeeper note below)
curl -sSfLO https://github.com/aegis-boot/aegis-boot/releases/latest/download/aegis-boot-aarch64-apple-darwin
chmod +x aegis-boot-aarch64-apple-darwin
```

**Gatekeeper note (direct-download path only).** The macOS binary is currently *ad-hoc codesigned* but **not notarized** — notarization is tracked as Phase A2 of [#365](https://github.com/aegis-boot/aegis-boot/issues/365) and gated on the project enrolling in the Apple Developer Program. In practice:

- `brew install` operators see no warning — Homebrew fetches over HTTPS without setting the `com.apple.quarantine` extended attribute, so Gatekeeper's first-launch policy never fires.
- Direct-download operators (browser download, or a `curl` invocation that inherits quarantine) may see "cannot be opened because the developer cannot be verified" on first launch. Clear quarantine explicitly:
  ```bash
  xattr -d com.apple.quarantine ./aegis-boot-aarch64-apple-darwin
  ```
  Or right-click → Open once in Finder. After first launch, Gatekeeper remembers the approval.

What works on macOS today: `aegis-boot list`, `aegis-boot doctor`, drive detection, and `aegis-boot flash --image PATH` (against a pre-built `.img` fetched via `aegis-boot fetch-image`). Image *building* still requires Linux — see the Quickstart table in [README.md](../README.md).

macOS x86_64 (Intel) pre-built binaries remain deferred — see [#365](https://github.com/aegis-boot/aegis-boot/issues/365).

### Windows

**Recommended path — [Rufus](https://rufus.ie) + the pre-built `.img`:**

1. Download the signed `aegis-boot-<version>.img` from [GitHub Releases](https://github.com/aegis-boot/aegis-boot/releases).
2. Verify the signature with the release-page checksum / cosign bundle (see [§ Verification](#verification)).
3. Open Rufus, select the `.img`, select your USB stick, click Start.
4. Done — the stick is an aegis-boot stick. Plug it into the target machine and boot.

No Windows CLI install needed for the typical operator path. Rufus is battle-tested (100M+ downloads), handles every quirky Windows storage edge case we'd otherwise have to reimplement, and is already the tool Windows sysadmins reach for.

**Advanced path — native Windows CLI (CI / automation):**

If you need to script flashes or run in CI, `aegis-boot` compiles on Windows:

```powershell
# From an elevated PowerShell with rustup installed:
cargo install --path crates/aegis-cli
aegis-boot flash --direct-install 1 --out-dir .\out --yes
```

The Windows `flash --direct-install` path uses native `diskpart` + `Format-Volume` + `windows-rs` direct-disk writes ([#419](https://github.com/aegis-boot/aegis-boot/issues/419) epic, closed 2026-04-24). Pre-built `aegis-boot.exe` releases are tracked in [#365](https://github.com/aegis-boot/aegis-boot/issues/365) but not currently prioritized — most operators shouldn't need the CLI on Windows.

### NixOS / Nix

aegis-boot ships a Nix flake. The `aegis-bootctl` derivation bakes the runtime tools (`sgdisk`, `mkfs.fat`, `mkfs.exfat`, `mcopy`, `curl`, `gnupg`) into the binary's PATH via `makeWrapper`, so there's nothing to install separately:

```bash
# One-shot run (no persistent install):
nix run github:aegis-boot/aegis-boot -- flash /dev/sdX --yes

# Persistent user install:
nix profile install github:aegis-boot/aegis-boot
aegis-boot --version
```

Or consume from a system flake:

```nix
# flake.nix
{
  inputs.aegis-boot.url = "github:aegis-boot/aegis-boot";
  outputs = { self, nixpkgs, aegis-boot, ... }: {
    nixosConfigurations.mymachine = nixpkgs.lib.nixosSystem {
      modules = [
        aegis-boot.nixosModules.aegis-boot
        { programs.aegis-boot.enable = true; }
      ];
    };
  };
}
```

The flake pins to `nixos-unstable`; downgrade to a specific channel in your own system flake if you want a frozen snapshot. Cosign verification of the binary is not currently wired through the Nix build (the flake builds from source instead of verifying a signed release artifact) — if that matters for your threat model, use the `install.sh` path + `nix-ld`.

Sanity check:

```bash
aegis-boot --version       # → aegis-boot v0.17.0
aegis-boot doctor          # 0–100 health score for host + stick
```

If `aegis-boot doctor` reports anything FAIL, fix that first — its NEXT ACTION line tells you exactly what.

Check whether your machine is in the hardware-compatibility database:

```bash
aegis-boot compat --my-machine   # DMI auto-lookup, Linux only
```

If it's not (common until the database grows), the Warn row from `doctor` will already tell you to run `aegis-boot compat --submit`, which gathers the same DMI values and generates a pre-filled GitHub issue URL — one click, four fields already populated. See [HARDWARE_COMPAT.md](./HARDWARE_COMPAT.md) for the criteria the database uses.

Optional — install shell completions for tab-complete on subcommands, catalog slugs, and compat-DB vendors:

```bash
# bash
aegis-boot completions bash | sudo tee /etc/bash_completion.d/aegis-boot >/dev/null
# zsh
aegis-boot completions zsh > ~/.zsh/completions/_aegis-boot
```

## The one-command path (recommended for new users)

If you want the whole "empty stick → rescue-ready with sensible ISOs" experience in one command:

```bash
sudo aegis-boot init /dev/sdc
```

That composes the four steps below (flash + three catalog fetches + adds) behind a single verb using the default `panic-room` profile (Alpine 3.20 + Ubuntu 24.04 Server + Rocky 9). See [`aegis-boot init`](./CLI.md#aegis-boot-init) for flags and alternative profiles.

The rest of this guide walks through the same flow step-by-step — useful when you want a custom ISO set, or when a step fails and you need to resume at a specific stage.

## Step 1 — write aegis-boot to the stick

```bash
sudo aegis-boot flash
```

What happens:
1. The CLI scans `/sys/block/sd*` for removable drives and shows you what it found:
   ```
   Detected removable drives:
     [1] /dev/sdc  SanDisk Cruzer Blade  29.8 GB  (1 partitions)
   ```
2. It asks you to confirm. If only one removable drive is present, `[Y/n]`. If more than one, `[1-N]`.
3. It asks for a typed `flash` confirmation (not `y/n`) — this is intentional friction because `dd` is destructive:
   ```
   ALL DATA ON /dev/sdc (SanDisk Cruzer Blade, 29.8 GB) WILL BE ERASED.
   Type 'flash' to confirm: flash
   ```
4. It builds the image (`scripts/mkusb.sh`), `dd`s it to the stick with `oflag=direct conv=fsync status=progress`, syncs, and runs `partprobe`.

Want to point at a specific drive instead of auto-detect? `sudo aegis-boot flash /dev/sdc`.

If `aegis-boot flash` says "No removable USB drives detected", the kernel doesn't see the stick as removable. Replug it; check `lsblk -o NAME,TRAN,RM`; the stick should show `RM=1` and `TRAN=usb`.

## Step 2 — add ISOs to the stick

The stick now has two partitions: `AEGIS_ESP` (signed boot chain, leave it alone) and `AEGIS_ISOS` (where you drop ISOs). The CLI handles the mount/copy/unmount cycle for you:

```bash
aegis-boot add ~/Downloads/ubuntu-24.04.2-live-server-amd64.iso
```

What happens:
1. The CLI reads `/proc/mounts` to find a currently-mounted `AEGIS_ISOS`. If none, it mounts the stick's partition 2 to a temporary directory.
2. It checks free space (refuses if `iso_size + 10 MiB headroom` doesn't fit).
3. It copies the ISO and any sibling sidecars: `<iso>.sha256`, `<iso>.SHA256SUMS`, `<iso>.minisig`. These let `rescue-tui` show a GREEN verification verdict instead of GRAY.
4. It unmounts the temporary mount (if it created one).

Verify:
```bash
aegis-boot list
```

```
ISOs on /tmp/aegis-cli-12345-0:

  [✓ sha256] [  minisig]    1.6 GiB  ubuntu-24.04.2-live-server-amd64.iso
  [  sha256] [  minisig]    198 MiB  alpine-3.20.3-x86_64.iso

2 ISO(s) total. Legend:
  ✓ sha256   sibling <iso>.sha256 present
  ✓ minisig  sibling <iso>.minisig present
  (missing sidecars mean the ISO will show GRAY verdict in rescue-tui)
```

Sidecars are optional but strongly recommended — see [TROUBLESHOOTING.md § "Why is my ISO GRAY?"](./TROUBLESHOOTING.md#why-is-my-iso-gray-instead-of-green).

## Step 3 — boot from the stick

1. Plug the stick into the target machine.
2. Open the firmware boot menu (commonly `F12`, `F11`, `Esc`, or `Del` at POST — varies by vendor).
3. Pick the USB entry. It will show as "USB" or the stick's vendor name.
4. shim verifies grub, grub verifies the kernel, the kernel runs `/init`, and `rescue-tui` starts.

If the firmware refuses to show the USB entry, your boot mode might be CSM/Legacy instead of UEFI — see [TROUBLESHOOTING.md § "Stick won't appear in boot menu"](./TROUBLESHOOTING.md#stick-wont-appear-in-the-firmware-boot-menu).

## Step 4 — pick an ISO

`rescue-tui` shows your ISOs with verification status:

```
aegis-boot v0.17.0    SB:enforcing  TPM:available

  ▸ ubuntu-24.04.2-live-server-amd64.iso        1.6 GiB  [✓ sha256]
    alpine-3.20.3-x86_64.iso                     198 MiB  [no verify]

[↑↓/jk] Move  [Enter] Boot  [/] Filter  [?] Help
```

Navigate with arrows (or `j/k`), press `Enter` to advance to the Confirm screen.

The Confirm screen shows the verdict (GREEN / YELLOW / GRAY), the discovered kernel + initrd paths, and any installer warnings. **If the ISO is an installer image** (Ubuntu live-server, Fedora netinst, Windows, etc.), the screen explicitly warns:

> Warning: This ISO contains an installer. If the ISO's own boot menu default is 'Install', DISKS ON THIS MACHINE MAY BE ERASED.

Press `Enter` again to commit. The TUI prints a handoff banner ("Booting … screen may go blank briefly") and invokes `kexec_file_load(2)`. The selected kernel takes over.

## Common outcomes

- ✅ **Signed kernel boots cleanly.** You see `kexec_core: Starting new kernel` followed by the ISO's own boot.
- ⚠️ **Unsigned kernel refused (`errno 61 ENODATA`).** The Error screen shows you the `mokutil --import` command. See [UNSIGNED_KERNEL.md](./UNSIGNED_KERNEL.md).
- ❌ **Cross-distro kexec quirk.** Some kernels refuse to kexec other-vendor kernels. See [docs/compatibility/iso-matrix.md](./compatibility/iso-matrix.md) for the per-distro table.

## Updating the stick later

The signed boot chain (ESP) doesn't change between releases for the same `mkusb.sh` output — only the ISO set on `AEGIS_ISOS` changes day-to-day. So:

- **Add or remove ISOs:** `aegis-boot add` / `rm` from a host mount. No reflash.
- **Update aegis-boot itself** (new release): rerun `sudo aegis-boot flash`. This rewrites the whole stick, including erasing your ISO set — back up `AEGIS_ISOS` first if you care about its contents.

## Where to go next

- [docs/CLI.md](./CLI.md) — full `aegis-boot` CLI reference
- [docs/TROUBLESHOOTING.md](./TROUBLESHOOTING.md) — common errors and fixes
- [docs/UNSIGNED_KERNEL.md](./UNSIGNED_KERNEL.md) — booting Alpine / Arch / NixOS via MOK enrollment
- [docs/USB_LAYOUT.md](./USB_LAYOUT.md) — what's actually on the stick (ESP + AEGIS_ISOS scheme)
