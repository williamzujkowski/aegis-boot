# Booting unsigned kernels

aegis-boot enforces the UEFI Secure Boot chain of trust all the way to the ISO's kernel. When `kexec_file_load(2)` is invoked on an ISO whose kernel isn't signed by a CA the firmware trusts, the kernel returns `ENODATA` (errno 61) and aegis-boot refuses to boot it. This is **correct behaviour** — it's the kernel enforcing the same policy that shim+grub already enforced for our own rescue kernel. A stick that happily boots unsigned kernels under Secure Boot would be a backdoor.

This document explains what the operator's options are.

## Why this matters

Ventoy's approach — one shared MOK key enrolled globally, one initramfs that bypasses `KEXEC_SIG` — effectively disables Secure Boot for anything Ventoy boots on that machine. Once enrolled, the key persists in NVRAM. aegis-boot intentionally does not do this.

Under aegis-boot, **the operator chooses per-ISO** whether to trust an unsigned kernel, via an explicit MOK enrollment that names the specific signing key.

## Option 1 — use a distro-signed ISO

The simplest path. These distros ship kernels signed by a UEFI CA that shim's built-in keyring trusts:

| Distro | Verified under aegis-boot v0.12.0 |
|---|---|
| Ubuntu 24.04+ | ✅ `kexec_core: Starting new kernel` (Canonical CA, real-hardware shakedown #109) |
| Fedora 39+ | likely (Fedora CA, unverified) |
| Debian 12+ | likely (Debian CA, unverified) |
| RHEL / Rocky / AlmaLinux | likely (Red Hat CA); may hit `CrossDistroKexecRefused` quirk |

Drop a signed ISO onto `AEGIS_ISOS` → rescue-tui discovers it → pick + boot. No enrollment required.

## Option 2 — enroll the distro's signing key via MOK

For distros that ship unsigned kernels — **Alpine, Arch, NixOS** — you need the distro's signing public key in the Machine Owner Keys (MOK) database. Aegis-boot's Error screen will show a `mokutil --import` command if you place the key alongside the ISO.

### Steps

1. Obtain the distro's kernel-signing public key. This is typically published alongside the kernel release:
   - Alpine: `https://alpinelinux.org/keys/ncopa.pub` (or follow their signing-key rotation notes)
   - Arch: `https://archlinux.org/master-keys/` for the release-signing master keys

2. Copy the ISO + its signing key onto the `AEGIS_ISOS` partition:
   ```
   cp alpine-3.20.3-x86_64.iso /mnt/aegis-isos/
   cp alpine-signing-key.pub  /mnt/aegis-isos/alpine-3.20.3-x86_64.iso.pub
   ```

3. Boot aegis-boot, pick the ISO, watch the Error screen. It will print:
   ```
   Enroll this ISO's signing key:
     sudo mokutil --import /run/media/aegis-isos/alpine-3.20.3-x86_64.iso.pub
   ```

4. Drop to the rescue shell (from the List screen, navigate to the `[#] rescue shell` entry and press Enter), copy that `mokutil` command, reboot to your normal system, and run it there. `mokutil` will prompt for a one-time password.

5. Reboot — shim's MOK Manager intercepts at firmware stage, asks for the password, enrolls the key permanently.

6. Boot aegis-boot again and try the ISO. It should succeed.

### What NOT to do

- **Don't run `mokutil --disable-validation`.** That literally disables Secure Boot enforcement. Don't.
- **Don't enroll a "master" key that the distro uses to sign many things.** Enroll only the minimum key needed to verify the specific kernel you want to boot.
- **Don't rely on Ventoy's shared key.** If you've already enrolled Ventoy's MOK, your machine now trusts anything Ventoy boots. Consider `mokutil --delete` for that key.

## Option 3 — relaxed kexec mode (not shipped)

Initially scoped (#126) as a `aegis.relaxed=1` cmdline opt-in that would fall back to `kexec_load(2)` on ENODATA. Investigation showed this doesn't actually help under Secure Boot + kernel lockdown (which Ubuntu's shim enables): `kexec_load(2)` is blocked by the same lockdown. It would only help on non-SB hosts, which don't need aegis-boot in the first place.

**Decision:** not shipping. The honest paths are Options 1 and 2 above. #126 closed with this note.

## Validation

As of v0.12.0 (real-hardware shakedown, [#109](https://github.com/williamzujkowski/aegis-boot/issues/109)):

- Alpine 3.20.3 (unsigned kernel) → `errno 61 (ENODATA)` with the guidance above. ✅ expected, correct.
- Ubuntu 24.04.2 (Canonical-signed) → `kexec_core: Starting new kernel`. ✅ boots through.

Both outcomes are correct. aegis-boot isn't magic — it's a careful, transparent orchestrator of the Linux kexec_file_load signature policy.
