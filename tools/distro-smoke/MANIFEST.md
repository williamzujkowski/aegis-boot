# distro-smoke MANIFEST

Everything the harness creates or modifies. If you delete `tools/distro-smoke/`
and revert the commit that added it, nothing below remains.

## Committed files (in repo)

| Path | Purpose | Owner |
|---|---|---|
| `tools/distro-smoke/README.md` | How to run / clean up | harness |
| `tools/distro-smoke/MANIFEST.md` | This file | harness |
| `tools/distro-smoke/run.sh` | Orchestrator (one-shot runner) | harness |
| `tools/distro-smoke/distros.sh` | Per-distro probe recipes | harness |
| `tools/distro-smoke/.gitignore` | Excludes `output/` from VCS | harness |

## Generated per-run (gitignored)

| Path | Purpose | When removed |
|---|---|---|
| `tools/distro-smoke/output/<run-id>/<distro>.log` | Combined stdout+stderr per distro | `rm -rf output/` |
| `tools/distro-smoke/output/<run-id>/summary.md` | Cross-distro roll-up | `rm -rf output/` |

## Host-side Docker state

Containers all run with `--rm`, so no stray containers accumulate. The
following images are cached when the harness first pulls them:

| Image | Size (approx) | Remove with |
|---|---|---|
| `opensuse/tumbleweed:latest` | ~500 MB | `docker rmi opensuse/tumbleweed:latest` |
| `ubuntu:24.04` | ~80 MB | `docker rmi ubuntu:24.04` |
| `alpine:3.20` | ~10 MB | `docker rmi alpine:3.20` |
| `fedora:40` | ~200 MB | `docker rmi fedora:40` |
| `archlinux:latest` | ~500 MB | `docker rmi archlinux:latest` |

Total: ~1.3 GB if you keep all five cached.

## Full cleanup (wipe this harness entirely)

```bash
rm -rf tools/distro-smoke/
docker rmi opensuse/tumbleweed:latest ubuntu:24.04 alpine:3.20 fedora:40 archlinux:latest 2>/dev/null || true
# Then revert the commit that added tools/distro-smoke/.
```

That returns the repo and host to the pre-harness state.
