<!--
Thanks for the PR! A few asks before you submit.

* Title: conventional commit format. Examples:
    feat(rescue-tui): add high-contrast theme
    fix(kexec-loader): classify ENODATA as SignatureRejected
    docs(install): clarify MOK enrollment flow
  Types: feat, fix, refactor, docs, test, chore, perf, build, ci.

* Branch: feat/<issue>-..., fix/<issue>-..., docs/<topic>, chore/<topic>.

* One concern per PR. Don't bundle a security fix with a refactor.

* See CONTRIBUTING.md for the full bar.
-->

## Summary

<!-- 1-3 sentences. What changes, and why? -->

## Linked issue

<!-- "closes #N" / "refs #N" / "part of #N" -->

## What changed

<!-- Bulleted list of the actual diff. Reviewers can read the diff;
     this list says where to focus their attention. -->

-

## How was this tested?

<!-- Tests added, scripts run, hardware exercised. Required for
     anything beyond a typo / pure-doc change. -->

- [ ] `cargo test --workspace` passes locally
- [ ] `./scripts/dev-test.sh` passes locally (or noted exceptions below)
- [ ] CHANGELOG entry under "Unreleased" if user-visible
- [ ] Docs updated if behavior or interface changed

<!-- Specific test runs / scenarios: -->

## Risk

<!-- Anything reviewers should pay extra attention to:
     - boot-chain, signing, or kexec-touching code
     - changes to mkusb.sh / build-initramfs.sh
     - dependency bumps
     - platform-specific behavior

     If purely additive / docs / tests-only, write "low / additive". -->

## Out of scope

<!-- What this PR explicitly does NOT do (so reviewers don't ask).
     Optional. -->
