# distro-smoke — local cross-distro smoke tests

**What this is:** a Docker-based harness that runs `aegis-boot doctor`
(and, optionally, `scripts/install.sh`) inside fresh containers of
five Linux distros. Catches the class of bugs that only surface when
the distro's `$PATH`, `sudo` policy, or package layout diverges from
Ubuntu — the class that #328 fell into.

**What this is NOT:** real-hardware testing (tracked in #132), a flash
smoke test (no block devices in containers), or a CI job (yet). See
"Promoting to CI" below.

## Scope today

The harness exercises:
1. **`aegis-boot doctor`** — runs against the mounted binary on each
   distro with the distro's baseline deps installed. Surfaces false-FAIL
   rows where a tool IS installed but `which()` misses it.
2. **`scripts/install.sh --no-verify`** — smoke-tests the installer's
   arg parsing + platform detection + download path on each distro.

Not exercised: actual `dd`, `flash`, `fetch-image`, or any real
block-device work. Those need VMs or hardware.

## Quickstart

```bash
# Build the binary first (release, matches what a real user runs).
cargo build --release -p aegis-cli

# Run the matrix.
cd tools/distro-smoke
./run.sh

# Results land in output/<timestamp>/ with one log file per distro
# and a summary.md that lists all findings.
```

Each `./run.sh` invocation creates a new timestamped subdirectory
under `output/`, so previous runs are preserved. Cleanup is a
single `rm -rf output/`.

## Distros

| Distro | Image | Notes |
|---|---|---|
| openSUSE Tumbleweed | `opensuse/tumbleweed:latest` | `/usr/sbin` not in default user PATH → reproduced #328 Bug 1 |
| Ubuntu 24.04 | `ubuntu:24.04` | Baseline — what CI runs on |
| Alpine 3.20 | `alpine:3.20` | musl — tests that our static-musl binary loads |
| Fedora 40 | `fedora:40` | dnf-based; firstboot path |
| Arch | `archlinux:latest` | Rolling release; pacman |

Edit `distros.sh` to add or remove distros.

## Outputs

```
output/<timestamp>/
├── summary.md              # human-readable roll-up across distros
├── summary.json            # machine-readable for automation
├── <distro>.doctor.txt     # full `aegis-boot doctor` stdout+stderr
├── <distro>.doctor.json    # `--json` envelope (for structured diff)
├── <distro>.install.txt    # `install.sh --help` / parse probes
└── <distro>.env.txt        # container PATH, sudo config, which-probe
```

## Cleanup

Nothing on the host persists except `output/` and the cached Docker
images. Containers run with `--rm` so they auto-remove on exit.

```bash
# Wipe run artifacts (keep harness + distro list):
rm -rf tools/distro-smoke/output/

# Wipe Docker image cache (frees ~2-3 GB):
docker rmi opensuse/tumbleweed:latest ubuntu:24.04 alpine:3.20 fedora:40 archlinux:latest

# Wipe the entire harness (if the approach didn't pan out):
rm -rf tools/distro-smoke/
# Then revert the commit that added it.
```

Every file created by the harness is either inside `output/` or one of
the three committed files (`run.sh`, `distros.sh`, this README).
`MANIFEST.md` lists every artifact + its purpose so a future reader
knows what's vestigial.

## Promoting to CI

When a run reveals a class of bug that unit tests can't catch, the
matrix is worth running on every PR. Wire `./run.sh` into
`.github/workflows/ci.yml` as a new job (requires Docker-in-Docker
or the GHA runner's Docker daemon) and gate on its exit code. Copy
the distro list verbatim — the harness is the spec.

Until then: run locally before cutting a release.
