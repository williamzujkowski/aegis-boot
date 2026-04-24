# How aegis-boot works (in 5 minutes)

Audience: a Linux-curious sysadmin who's read about Secure Boot but never set it up. If you already know what shim/grub/MOK mean, skip to [USB_LAYOUT.md](USB_LAYOUT.md) for the implementation map.

## What aegis-boot does

aegis-boot writes a USB stick that:

1. Boots into a tiny rescue menu on **any laptop with default firmware** — no BIOS changes, no Secure Boot disable
2. Lets you pick an ISO from the stick (Ubuntu installer, Alpine rescue, Windows installer, your own)
3. Hands off to that ISO without rebooting (`kexec`)

You write the stick **once**. After that, drop ISOs onto the data partition and they show up in the menu next boot. Same workflow as Ventoy, except aegis-boot keeps Secure Boot **enforcing** the whole time.

## Why this is hard without aegis-boot

Every other multi-ISO USB tool — Ventoy, YUMI, MultiBootUSB — asks you to either:

- **Disable Secure Boot in the BIOS** (gives up the protection your firmware provides), or
- **Trust their unsigned bootloader** by enrolling their key into your firmware's MOK keyring (a one-time per-machine ceremony that's awkward to reverse)

Both work. Both also degrade the trust posture of the machine you're trying to boot. aegis-boot ships a stick that boots **out of the box** on every laptop with default firmware because it uses the same signed boot chain real distros use.

## The trust chain in 30 seconds

```
firmware (Microsoft 3rd-party CA, pre-enrolled on every PC since ~2012)
    │
    ▼
shim          ← signed by Microsoft. Verified by firmware.
    │
    ▼
grub          ← signed by Debian. Verified by shim's embedded Debian CA cert.
    │
    ▼
linux kernel  ← signed by Debian. Verified by grub.
    │
    ▼
rescue-tui    ← runs in the verified kernel. Picks an ISO.
    │
    ▼
kexec → ISO   ← grub/shim from the chosen ISO take over.
```

Every link in the chain is signed and verified by the link before it. If anyone tampers with shim, grub, or the kernel on the stick, the firmware refuses to boot the chain at all — you get a Secure Boot violation message instead of a silent compromise.

The trust root (Microsoft 3rd-party CA) is the same one your laptop already trusts to boot Ubuntu / Fedora / Debian / SUSE off their official install media. aegis-boot reuses that trust — it doesn't ask you to add anything new.

## What's on the stick

Two partitions:

```
/dev/sdX1   AEGIS_ESP    ESP (FAT32, <!-- constants:BEGIN:ESP_SIZE_MB -->400<!-- constants:END:ESP_SIZE_MB --> MB)   ← signed shim/grub/kernel chain
/dev/sdX2   AEGIS_ISOS   data (exFAT, rest)     ← your .iso files (#243; opt-in fat32/ext4 also supported)
```

See [docs/USB_LAYOUT.md](USB_LAYOUT.md) for the full partition + filesystem contract (volume-label identifiers, alignment, file-count caps). The numbers above derive from `crates/aegis-cli/src/constants.rs` via the `constants:` marker pipeline (see [ARCHITECTURE.md §constants-drift](ARCHITECTURE.md) for how this doc stays in sync with code).

`AEGIS_ESP` is what the firmware boots from. It's tiny, signed, and **never changes** after `aegis-boot flash` writes it. `AEGIS_ISOS` is yours — drop ISOs in, copy them out, edit metadata sidecars (#246). Tampering with `AEGIS_ISOS` cannot break the boot chain because the boot chain doesn't live there.

When the rescue-tui menu lists your ISOs, it computes their sha256 on the spot and compares against optional `<iso>.sha256` sidecars you write yourself. The verdict (verified ✓ / mismatch ✗ / no sidecar) is shown before you boot — but the **boot decision** itself stays with you. There is no auto-update, no phone-home, no auto-trust.

## Trust tiers in rescue-tui

When you boot the stick and rescue-tui scans `AEGIS_ISOS`, every
`.iso` file gets classified into one of 6 trust tiers. The tier drives
the color, glyph, and whether Enter actually boots — the design
principle is "Secure Boot stays strict; operator attestation relaxes
gracefully" (see [epic #455](https://github.com/aegis-boot/aegis-boot/issues/455)).

<!-- tiers:BEGIN:TRUST_TIER_TABLE -->
| Tier | Verdict             | Glyph | Bootable | Meaning                                    |
| ---- | ------------------- | ----- | -------- | ------------------------------------------ |
| 1    | VERIFIED            | `[+]` | yes      | Hash or sig verified vs trusted source     |
| 2    | UNVERIFIED          | `[ ]` | yes      | No sidecar — bootable with typed confirm   |
| 3    | UNTRUSTED KEY       | `[~]` | yes      | Sig parses, signer untrusted               |
| 4    | PARSE FAILED        | `[!]` | **no**   | iso-parser couldn't extract kernel         |
| 5    | BOOT BLOCKED        | `[X]` | **no**   | Kernel rejected by platform keyring        |
| 6    | HASH MISMATCH       | `[!]` | **no**   | ISO bytes don't match declared hash        |
<!-- tiers:END:TRUST_TIER_TABLE -->

This table is generated from the `TrustVerdict` enum in
[`crates/rescue-tui/src/verdict.rs`](../crates/rescue-tui/src/verdict.rs)
by the `tiers-docgen` devtool — run it after any enum change:

```bash
cargo run -p rescue-tui --bin tiers-docgen
```

## What aegis-boot does NOT do

- **It does not modify your laptop's firmware.** No MOK enrollment, no PK/KEK/db changes. Plug the stick in, boot, eject — your firmware is untouched.
- **It does not force-validate ISOs.** Drop any ISO on, the menu shows it. The hash sidecar is optional. The menu will warn loudly when an ISO has no verification metadata, but the trust call is yours.
- **It does not boot Windows installer ISOs.** Windows uses the NT boot loader, which is incompatible with `kexec_file_load`. `rescue-tui` detects these and shows a redirect to [Rufus](https://rufus.ie), which handles Windows sticks directly. Point aegis-boot at Linux ISOs; use Rufus for Windows.
- **It does not ship a kernel-side hardening kit.** Secure Boot is the only protection; if a kernel CVE appears post-flash, your stick is still vulnerable until you re-flash with a newer build.
- **It does not auto-update.** Stick contents are immutable except by you. Use `aegis-boot update <device>` for in-place signed-chain rotation when a new shim/grub/kernel ships.

## Where to read more

| You want to know...                                                     | Read                                         |
| ----------------------------------------------------------------------- | -------------------------------------------- |
| How to use it, end-to-end                                               | [TOUR.md](TOUR.md)                           |
| Every CLI subcommand                                                    | [CLI.md](CLI.md)                             |
| The signed-chain build process, layer by layer                          | [USB_LAYOUT.md](USB_LAYOUT.md)               |
| Why this architecture (vs Ventoy, vs grub-loopback)                     | [ARCHITECTURE.md](ARCHITECTURE.md)           |
| Which laptops have been verified                                        | [HARDWARE_COMPAT.md](HARDWARE_COMPAT.md)     |
| What to do when something fails                                         | [TROUBLESHOOTING.md](TROUBLESHOOTING.md)     |
| The fixed-cap "I am sometimes asked to boot foreign-CA kernels" story   | [UNSIGNED_KERNEL.md](UNSIGNED_KERNEL.md)     |

If you'd rather walk through it in the terminal, run:

```
aegis-boot tour
```
