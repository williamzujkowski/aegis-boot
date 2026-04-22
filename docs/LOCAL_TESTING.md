# Local testing

While GitHub Actions CI is the primary validation gate, you can run the full matrix locally in ~8-15 minutes using KVM + libvirt (varies by hardware — fast NVMe + warm cargo cache hits the low end; cold cache or slower disks land closer to the high end). This is the right dev loop when:

- You want instant feedback before pushing.
- You're iterating on the QEMU-adjacent jobs (`mkusb`, `OVMF SecBoot E2E`, `kexec E2E`) that take several minutes to hit on CI.
- CI is unavailable (rare; outage or quota issue).

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
