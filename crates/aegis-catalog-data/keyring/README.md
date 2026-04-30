# Vendor keyring (#655 Phase 2B PR-B)

PGP public keys for the upstream vendors in `aegis_catalog::CATALOG`.
Used by `aegis-fetch` to verify the signed-chain on every catalog ISO
download (clearsigned CHECKSUM, detached `.gpg` on SUMS, detached
`.asc` on the ISO itself — see `aegis_catalog::SigPattern`).

## Layout

```
keyring/
├── fingerprints.toml      # pinned primary-fingerprint set per vendor
├── README.md              # this file
├── <vendor>.asc           # ASCII-armored OpenPGP public-key bundle
└── <vendor>.txt           # human-readable metadata sidecar (UIDs,
                           # creation/expiration dates, fingerprints)
```

Eight vendors land in PR-B with a real `.asc`: AlmaLinux, Alpine,
Debian, Fedora, Kali, Manjaro, Rocky, Ubuntu. Six more are
referenced by `CATALOG` but await keyring population (LinuxMint, MX,
SystemRescue, GParted, System76, openSUSE) — see
`fingerprints.toml` for the per-vendor TODO list.

## Trust model

- Each `.asc` file is committed to the repo as the trust anchor.
  HTTPS-from-vendor + reviewer scrutiny of the `.txt` sidecar is the
  bootstrap; subsequent integrity is enforced by git commit history
  (any post-commit tamper is visible to anyone with a repo clone).
- `fingerprints.toml` pins the set of primary-key fingerprints
  expected in each `.asc`. The loader (`aegis_fetch::VendorKeyring`)
  asserts set-equality at load time — a `.asc` whose fingerprint set
  differs from the pin is rejected.
- `aegis-fetch` is verify-only. No private keys exist in the
  process; the `rsa` Marvin Attack timing sidechannel
  (`RUSTSEC-2023-0071`) doesn't apply.

## Rotation

Don't hand-edit `.asc` or `.txt` files. The `catalog-refresh.yml`
workflow runs weekly and:

1. Fetches each vendor's release-signing key from its canonical
   upstream URL (recorded per-vendor in the workflow).
2. Computes the new primary-fingerprint set.
3. If the set matches the `fingerprints.toml` pin: exits clean.
4. If it differs (rotation / addition / expiration): opens an
   auto-PR with the new `.asc`, `.txt`, and `fingerprints.toml`
   diff. Auto-merge is NEVER enabled — a maintainer reviews the
   rotation against the vendor's published security advisory
   (link included in the PR body) before merging.

## Reviewing a rotation PR

Open the auto-PR's diff and check:

- The `.txt` sidecar shows User IDs that match the vendor's
  documented release-signing identity.
- Creation / expiration dates are sane (no past-dated key, no
  far-future expiration that would indicate a forged key).
- The new fingerprint matches what the vendor publishes on their
  security page (linked in the PR body).
- For multi-key bundles (Fedora, Manjaro), each individual key's
  metadata matches a known-good identity.

If anything looks off, close the PR and ping the maintainer. The
weekly run will re-open it next week with the same diff if the
issue is upstream's; that gives time for the vendor to clarify.
