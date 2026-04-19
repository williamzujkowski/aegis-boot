# aegis-boot first-time tour

Audience: you've installed `aegis-boot` (per [INSTALL.md](INSTALL.md)) and want a guided walk through your first stick. Reading time: ~3 minutes. Hands-on time: ~10 minutes plus dd.

For the *why* behind each step, see [HOW_IT_WORKS.md](HOW_IT_WORKS.md).

## What you need

- An empty USB stick (8 GB+ recommended; aegis-boot itself fits in ~2 GB)
- One ISO you want to be able to boot — Ubuntu / Debian / Fedora installer, Alpine rescue, your own
- `sudo` on a Linux workstation
- (optional) A target machine with UEFI Secure Boot **enforcing** to test on

## The 4-command path

```
$ aegis-boot doctor          # check the host can build a stick
$ aegis-boot init /dev/sdX   # flash + curated rescue bundle
$ aegis-boot fetch ubuntu-24.04-live-server   # add a verified install ISO
$ aegis-boot add ./ubuntu-24.04-live-server.iso /dev/sdX   # copy onto stick
```

That's it. Eject the stick, boot a target machine, the rescue-tui menu shows you Ubuntu + the panic-room ISOs `init` added.

## Step-by-step

### 1. Check the host

```
$ aegis-boot doctor
```

Inspects: are `dd`, `mount`, `umount`, `sudo` available; is your firmware reporting Secure Boot status; do you have `/sys/class/dmi/id` for the hardware-report flow. Pass / Warn / Skip per row, one paragraph per row of detail.

If `doctor` reports any blocker, fix it before flashing — the failure messages are designed to be Google-able.

### 2. Pick a target stick

Plug the USB stick in. Find its `/dev/sdX` path:

```
$ lsblk -o NAME,SIZE,MODEL,SERIAL,TRAN
sda  29.8G SanDisk Cruzer  4C53...9173  usb
sdb  931G  Samsung NVMe    ABC...XYZ    nvme
```

You want the **removable USB** one (column `TRAN=usb`). On a fresh install with one stick attached, that's `/dev/sda` here.

If you're nervous about picking the wrong one, run with no device argument and let `aegis-boot` enumerate + pick interactively (#245 in progress):

```
$ aegis-boot init
Removable USB drives detected:
  [1] /dev/sda  SanDisk Cruzer  29.8 GB   serial: 4C53...9173
Select target [1]: 1
```

### 3. Flash the stick

`init` does the destructive write + sets up a curated bundle of rescue ISOs. **All data on the target device will be erased.**

```
$ sudo aegis-boot init /dev/sda
```

Behind the scenes:

1. Verifies the aegis-boot signed image's signature
2. Refuses if `/dev/sda` isn't removable + USB
3. `dd`s the signed shim/grub/kernel/rescue-tui chain onto partition 1
4. Reads back the first <!-- constants:BEGIN:READBACK_WINDOW -->64 MB<!-- constants:END:READBACK_WINDOW --> and re-verifies the chain's sha256
5. Writes an attestation receipt under `~/.local/share/aegis-boot/attestations/`

After this completes the stick is bootable and the menu will show whatever curated ISOs the `init` profile included.

### 4. Add your own ISO

```
$ aegis-boot add ./ubuntu-24.04-live-server.iso /dev/sda
```

`add` mounts the stick's `AEGIS_ISOS` partition, copies the ISO, copies any sibling `.sha256` / `.minisig` sidecars, and updates the attestation manifest.

Optional metadata that improves the menu (#246):

```
$ aegis-boot add ./ubuntu-24.04-live-server.iso /dev/sda \
    --description "Ubuntu Server 24.04 LTS (live install)" \
    --version 24.04 \
    --category install
```

That writes a `<iso>.aegis.toml` sidecar so the rescue-tui menu shows `Ubuntu Server 24.04 LTS (live install)` instead of `ubuntu-24.04-live-server.iso`.

### 5. Inspect what's on the stick

```
$ aegis-boot list /dev/sda
ISOs on /tmp/aegis-cli-1234-0:

  [✓ sha256] [  minisig]    1.4 GiB  ubuntu-24.04-live-server.iso

1 ISO(s) total. Legend:
  ✓ sha256   sibling <iso>.sha256 present
  ✓ minisig  sibling <iso>.minisig present
```

`✓` columns mean the rescue-tui will show a green VERIFIED verdict before booting that ISO. Missing sidecars mean you'll get a yellow GRAY verdict and a typed-confirmation prompt — your call whether to boot.

### 6. Eject safely

```
$ aegis-boot eject /dev/sda
```

`sync`s outstanding writes, unmounts, then powers down the device so it's safe to physically pull. Pulling a stick mid-write is the most common way to brick a fresh image — eject is cheap insurance.

### 7. Boot a target machine

Plug the stick into a target. Power on. Hit the firmware boot-menu key (`F12`/`F9`/`Esc` depending on vendor). Pick the USB. The rescue-tui appears. Pick an ISO. Press Enter. The chosen ISO's kernel takes over via `kexec` — no second reboot.

If you hit `SignatureRejected` for an ISO, it means the kernel inside that ISO isn't signed by a CA your firmware (or the operator's MOK keyring) trusts. Use `aegis-boot doctor --remedy` for the `mokutil` enrollment recipe.

## What's next

| Want to...                       | Try                                                   |
| -------------------------------- | ----------------------------------------------------- |
| Browse curated ISOs              | `aegis-boot recommend`                                |
| Verify every ISO on a stick      | `aegis-boot verify /dev/sda`                          |
| Show past flash attestations     | `aegis-boot attest list`                              |
| Look up your laptop's compat     | `aegis-boot compat --my-machine`                      |
| Update the signed chain in place | `aegis-boot update /dev/sda` (eligibility-check only) |

For the *why* behind each step, see [HOW_IT_WORKS.md](HOW_IT_WORKS.md).
For machine-readable output for any read-mostly subcommand, append `--json`.
