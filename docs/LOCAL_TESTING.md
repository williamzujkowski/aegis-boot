# Local testing

While GitHub Actions CI is the primary validation gate, you can run the full matrix locally in ~8-15 minutes using KVM + libvirt (varies by hardware — fast NVMe + warm cargo cache hits the low end; cold cache or slower disks land closer to the high end). This is the right dev loop when:

- You want instant feedback before pushing.
- You're iterating on the QEMU-adjacent jobs (`mkusb`, `OVMF SecBoot E2E`, `kexec E2E`) that take several minutes to hit on CI.
- CI is unavailable (rare; outage or quota issue).

## TL;DR — `tools/local-ci.sh`

The fastest path to "did my change just break a QEMU E2E?" without pushing to CI:

```bash
tools/local-ci.sh quick        # cargo fmt + check + clippy + lib tests (~9s on a Framework laptop)
tools/local-ci.sh kexec        # rescue-tui → target-kernel kexec under QEMU (~3 min)
tools/local-ci.sh ovmf-secboot # SB-enforcing signed chain → rescue-tui (~4 min)
tools/local-ci.sh mkusb        # build USB image + boot smoke (~4 min)
tools/local-ci.sh qemu-smoke   # minimal initramfs boot (~2 min)
tools/local-ci.sh thumb-drive --confirm-write /dev/sdX  # real USB (operator-only)
tools/local-ci.sh all          # full suite sans thumb-drive (~12 min)
```

Each subcommand wraps the same `scripts/*.sh` that the corresponding `.github/workflows/*.yml` runs in CI — no reimplementation, just a friendlier dispatcher with prerequisite checks. Toolchain auto-pins to `cargo +1.88.0` if available so local lints match CI's lint set exactly.

### Decision tree — what to run before pushing

| You changed                                | Run locally before push                                  |
| ------------------------------------------ | -------------------------------------------------------- |
| Only `*.md` / `docs/**` / `LICENSE*`       | nothing — CI fast-path skips the E2E suite (see below)   |
| Any Rust source under `crates/`            | `tools/local-ci.sh quick`                                |
| `crates/rescue-tui/` UI or state           | `quick` + `kexec` + `ovmf-secboot`                       |
| `crates/aegis-cli/src/flash.rs` or detect  | `quick` + `mkusb` + `direct-install`                     |
| `crates/iso-probe/` or `iso-parser/`       | `quick` + `kexec`                                        |
| `scripts/build-initramfs.sh` or `/init`    | `quick` + `kexec` + `qemu-smoke`                         |
| `scripts/mkusb.sh` or USB layout           | `quick` + `mkusb`                                        |
| `crates/aegis-trust/` or epoch handling    | `quick` + `update-rotate` (when needed)                  |
| Multi-area refactor                        | `tools/local-ci.sh all`                                  |

### When CI is still authoritative — local can NOT replace these

These checks ship only in CI (different runner OS, different toolchains, or dependent on GitHub-side state):

- **macOS native smoke** (`macos-14`) — needs an Apple-Silicon runner
- **Windows cargo-check** (`windows-2022`) — needs a Windows runner
- **CodeQL** (Rust + Actions analysis) — GitHub-managed scan
- **Reproducible build verification** — two full builds + byte-parity diff (you can run `tools/local-ci.sh quick` to catch the same code, but the byte-parity is CI-only)
- **OpenSSF Scorecard** — GitHub-managed weekly evaluation
- **Secret scanning (gitleaks)**, **SAST (semgrep)**, **cargo-deny advisories** — fast on CI, not worth replicating
- **CycloneDX SBOM generation** — release-artifact concern
- **Doc drift checks** (CLI subcommand, manifest schema, trust-anchors, version, constants, trust-tier) — these *should* run on doc PRs since they catch the case where a doc claims X but the binary surface says Y; they're cheap on CI so no local equivalent shipped

After Phase 1 of [#580](https://github.com/aegis-boot/aegis-boot/issues/580), the 6 expensive QEMU E2E workflows + reproducible-build skip on docs-only PRs (markdown / `docs/` / issue templates / LICENSE), so a docs-only PR's CI completes in ~2 min instead of ~5+ min.

### Common failure-mode triage

| Symptom                                                       | First place to look                                                |
| ------------------------------------------------------------- | ------------------------------------------------------------------ |
| `qemu-system-x86_64` not found                                | `apt-get install qemu-system-x86` (or distro equivalent)           |
| OVMF firmware not found                                       | `apt-get install ovmf` — check `/usr/share/OVMF/` paths            |
| `cargo +1.88.0` not installed (warns once per run)            | `rustup toolchain install 1.88.0` — matches CI's pinned toolchain  |
| `quick` clippy fails locally but green on CI                  | newer Rust on host adds lints CI's 1.88.0 doesn't have — run with the pin |
| `kexec` / `ovmf-secboot` hangs                                | `TIMEOUT_SECONDS=300 tools/local-ci.sh kexec` (default 180s); also check `/boot/vmlinuz-*` is mode 0644 (mcopy needs to read it) |
| `thumb-drive` aborts with "not flagged removable"             | by design — refusing NVMe even with `--confirm-write`. Use `lsblk` to find the USB; ensure it's the right `/dev/sdX` |

## One-time setup

```bash
sudo apt-get install -y \
    qemu-system-x86 ovmf \
    shim-signed grub-efi-amd64-signed linux-image-generic \
    mtools dosfstools exfatprogs gdisk \
    busybox-static cpio xorriso util-linux

# Make the installed kernel readable by non-root (mcopy needs to read it
# during image construction). Re-run after every kernel update.
sudo chmod 0644 /boot/vmlinuz-* /boot/initrd.img-*

# Add yourself to the kvm and libvirt groups if not already.
sudo usermod -aG kvm,libvirt "$USER"
# Log out and back in for group changes to take effect.
```

## Run everything

```bash
./scripts/dev-test.sh
```

Steps it runs, in order (short-circuits on first failure):

1. `cargo fmt --check` — formatting
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo test --workspace`
4. `scripts/build-initramfs.sh` — produces `out/initramfs.cpio.gz`
5. `scripts/mkusb.sh` — produces `out/aegis-boot.img`
6. QEMU boot of the mkusb image under OVMF SecBoot — asserts SB enforcing + rescue-tui starts
7. `scripts/qemu-kexec-e2e.sh` — asserts the rescue-tui → target-kernel kexec handoff
8. `cargo run --release -p aegis-fitness` — repo / build / artifact health audit

Total runtime on a Framework laptop with warm cargo cache: ~8-10 min. Most of that is step 6 (QEMU cold boot) and step 7 (second QEMU cold boot + initramfs customization + ISO build). Cold cargo cache adds 2-5 min on top.

## Iterating on specific tests

Skip everything and run just the piece you're changing:

| Change affects | Run just |
|---|---|
| Rust code in any crate | `cargo test --workspace` |
| `rescue-tui` UI | `cargo test -p rescue-tui` + manual `cargo run -p rescue-tui` in a real terminal |
| `iso-probe` mount behavior | `sudo -E AEGIS_INTEGRATION_ROOT=1 cargo test -p iso-probe -- --ignored` |
| `kexec-loader` syscall path | `sudo -E AEGIS_INTEGRATION_ROOT=1 cargo test -p kexec-loader -- --ignored` |
| Initramfs assembly | `./scripts/build-initramfs.sh` + inspect with `zcat out/initramfs.cpio.gz \| cpio -t` |
| USB layout | `./scripts/mkusb.sh` + `./scripts/qemu-try.sh` for an interactive boot |
| OVMF SB chain | `./scripts/ovmf-secboot-e2e.sh` |
| kexec handoff | `sudo -E ./scripts/qemu-kexec-e2e.sh` |

## Interactive boot — drive the TUI yourself

```bash
./scripts/build-initramfs.sh
./scripts/mkusb.sh
./scripts/qemu-try.sh
```

Opens an interactive QEMU session. `Ctrl-A X` exits. The TUI runs on the serial console; to test it with a real terminal, reboot under libvirt:

```bash
virt-install --name aegis-boot-dev \
    --memory 2048 --vcpus 2 \
    --os-variant ubuntu22.04 \
    --boot uefi,loader=/usr/share/OVMF/OVMF_CODE_4M.secboot.fd \
    --disk path=out/aegis-boot.img,bus=virtio,format=raw \
    --nographics --console pty,target_type=serial \
    --import
```

(Use `virsh destroy` + `virsh undefine --nvram aegis-boot-dev` to clean up.)

## Dropping ISOs onto the data partition

### One-shot via `qemu-loaded-stick.sh` (recommended)

```bash
mkdir -p test-isos
cp ~/Downloads/ubuntu-24.04.iso test-isos/
./scripts/qemu-loaded-stick.sh                       # default: virtio, serial
./scripts/qemu-loaded-stick.sh -i                    # GTK display
./scripts/qemu-loaded-stick.sh -a sata -i            # AHCI path (real desktops)
./scripts/qemu-loaded-stick.sh -a usb                # usb-storage on xHCI (real USB)
./scripts/qemu-loaded-stick.sh -d ~/iso-stash -k     # custom dir, keep image
```

The script builds a fresh `out/aegis-boot.img`, loop-mounts AEGIS_ISOS,
copies every `.iso` (plus any sibling `.sha256` / `.minisig`) from the
source dir, and boots under OVMF SecBoot. Image size auto-scales to
1.5× the total ISO bytes (min 2 GiB).

`--attach` chooses how the stick image is presented to the VM:

| Mode | What it exercises | Use when |
|---|---|---|
| `virtio` (default) | paravirtual `virtio-blk` | fastest dev loop; only proves the boot chain |
| `sata` | AHCI module path (`ahci.ko`) | matches most desktops + older laptops |
| `usb` | `qemu-xhci` + `usb-storage` | closest to a real USB stick plugged into a host |

All three modes were verified end-to-end starting in v0.7.0 with Alpine 3.20 (4 ISO entries discovered); v0.12.0 added real-hardware shakedown via USB-passthrough on a SanDisk Cruzer 32 GB stick (Alpine refusal + Ubuntu boot, [#109](https://github.com/aegis-boot/aegis-boot/issues/109)).

### Capturing TUI screenshots from the VM

For maintainers refreshing `docs/screenshots/*.png`:

```bash
./scripts/capture-tui-screenshots.sh -d ./test-isos        # ~5 min
./scripts/capture-tui-screenshots.sh -d ./test-isos --keep # keep image for re-runs
```

Boots aegis-boot under QEMU+OVMF SecureBoot, drives `rescue-tui` via QMP `send-key`, and dumps each scripted screen as a PNG via QMP `screendump` + ImageMagick. The 7 captured scenarios cover the list, sort, confirm, help, filter, and selection flows. See `scripts/capture-tui-screenshots.sh --help` for the full list. ([#478](https://github.com/aegis-boot/aegis-boot/issues/478))

`tui-screenshots` (the ANSI-only fixture binary, [#477](https://github.com/aegis-boot/aegis-boot/issues/477)) covers synthetic-only states (parse-failed, SB-blocked Windows, hash mismatch); the VM capture covers boot-chain-validated states.

### Manual, if you want to inspect the partition

```bash
# Attach the image as a loop device
sudo losetup -fP out/aegis-boot.img
LOOP=$(sudo losetup -j out/aegis-boot.img | cut -d: -f1)

# Mount partition 2 (AEGIS_ISOS)
sudo mkdir -p /mnt/aegis-isos
sudo mount "${LOOP}p2" /mnt/aegis-isos

# Copy your ISOs
sudo cp ~/Downloads/ubuntu-24.04.iso /mnt/aegis-isos/

# Clean up
sudo umount /mnt/aegis-isos
sudo losetup -d "$LOOP"
```

Then boot via `qemu-try.sh` — the TUI should list your ISOs.

## When local == CI

The scripts are deliberately identical to what CI invokes. If `dev-test.sh` passes locally, CI should too. If they diverge (different kernel, different OVMF version, different runner arch), file an issue — we want local to match the reference CI environment.

CI status is the merge gate; local testing is the pre-push sanity check. CI runs are visible at the [Actions tab](https://github.com/aegis-boot/aegis-boot/actions).
