
## Verifying these artifacts

All artifacts are signed with [Sigstore cosign keyless](https://docs.sigstore.dev/) (no per-key management — the signing certificate is bound to the GitHub Actions workflow that produced the release). To verify:

```bash
TAG=$(gh release view --json tagName -q .tagName)   # or set explicitly
cd "$(mktemp -d)"
gh release download "$TAG" --pattern '*'

# Verify any artifact (here, the operator CLI):
cosign verify-blob \
  --certificate-identity-regexp '^https://github\.com/aegis-boot/aegis-boot/\.github/workflows/release\.yml@refs/tags/v.+$' \
  --certificate-oidc-issuer 'https://token.actions.githubusercontent.com' \
  --signature aegis-boot-x86_64-linux.sig \
  --certificate aegis-boot-x86_64-linux.pem \
  aegis-boot-x86_64-linux

# Cross-check every artifact's hash:
sha256sum -c SHA256SUMS
```

The cosign certificate is bound to the `release.yml` workflow at the tag's ref — verifying it confirms the artifact was produced by *this repository's* release workflow, not a copycat.
