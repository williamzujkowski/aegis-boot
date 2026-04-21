#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Re-validate all URLs in the `aegis-boot recommend` catalog.
#
# Reads iso_url / sha256_url / sig_url fields from
# crates/aegis-cli/src/catalog.rs via grep and runs `curl -sI` on
# each. Reports status per URL and exits 1 if any URL returns
# something other than 200 or a 30x redirect.
#
# Runs per-entry (slug line → next Entry {) so a broken URL is
# reported with its slug for context.
#
# Intended to run on a weekly CI schedule (see
# .github/workflows/catalog-revalidate.yml) and via workflow_dispatch
# for ad-hoc checks. Also runnable locally:
#   bash scripts/catalog-revalidate.sh
#
# Exit codes:
#   0 — all URLs OK
#   1 — at least one URL broken
#   2 — usage / script error (catalog file missing, curl unavailable)

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CATALOG="$ROOT_DIR/crates/aegis-cli/src/catalog.rs"

if [[ ! -f "$CATALOG" ]]; then
    echo "error: catalog not found at $CATALOG" >&2
    exit 2
fi
command -v curl >/dev/null 2>&1 || {
    echo "error: curl not found in PATH" >&2
    exit 2
}

# Parse: for each Entry { ... } block, extract slug + iso_url +
# sha256_url + sig_url. awk is fine for this; the catalog is small.
awk '
/slug:/ {
    match($0, /"[^"]+"/); slug = substr($0, RSTART+1, RLENGTH-2)
}
/iso_url:/ {
    match($0, /"[^"]+"/); iso = substr($0, RSTART+1, RLENGTH-2)
}
/sha256_url:/ {
    match($0, /"[^"]+"/); sha = substr($0, RSTART+1, RLENGTH-2)
}
/sig_url:/ {
    match($0, /"[^"]+"/); sig = substr($0, RSTART+1, RLENGTH-2)
    if (slug != "" && iso != "" && sha != "" && sig != "") {
        print slug "|iso|" iso
        print slug "|sha256|" sha
        print slug "|sig|" sig
        slug=""; iso=""; sha=""; sig=""
    }
}
' "$CATALOG" > /tmp/catalog-urls.$$

total=$(wc -l < /tmp/catalog-urls.$$)
fail=0
printf '%-32s %-8s %-12s %s\n' "SLUG" "KIND" "STATUS" "URL"
printf '%-32s %-8s %-12s %s\n' "----" "----" "------" "---"

while IFS='|' read -r slug kind url; do
    # Range-GET the first byte rather than HEAD: many CDNs (esp.
    # sourceforge, archlinux, kali) reject HEAD with 403/404 while
    # serving GETs fine. --range 0-0 asks for byte 0 only so we
    # don't download multi-GB ISOs. 200 + 206 Partial Content both
    # mean "URL exists".
    http_code=$(curl -sS --range 0-0 --max-time 30 \
        --location --user-agent "aegis-boot-catalog-revalidate/1.0" \
        --write-out '%{http_code}' \
        --output /dev/null "$url" 2>/dev/null || echo "000")
    case "$http_code" in
        200|206)  status="OK"      ;;
        2*)       status="OK$http_code" ;;
        3*)       status="REDIR"   ;;
        4*|5*)    status="HTTP$http_code"; fail=$((fail+1)) ;;
        000)      status="TIMEOUT"; fail=$((fail+1)) ;;
        *)        status="UNK$http_code"; fail=$((fail+1)) ;;
    esac
    printf '%-32s %-8s %-12s %s\n' "$slug" "$kind" "$status" "$url"
done < /tmp/catalog-urls.$$

rm -f /tmp/catalog-urls.$$

# `total` is the number of URL triples printed (one per valid Entry
# block), so catalog entries = total / 3.
entries=$((total / 3))
echo
echo "Checked $total URLs across $entries catalog entries."
if (( fail > 0 )); then
    echo "BROKEN: $fail URL(s) failed" >&2
    exit 1
fi
echo "All URLs OK."
