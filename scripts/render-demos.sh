#!/usr/bin/env bash
# Render asciinema .cast files to inline-renderable SVG (#348).
#
# Maintainer alignment (#348): inline SVG so GitHub markdown renders
# the demo without an external asciinema.org dependency. Two render
# tools are supported in order of preference; first one available wins:
#
#   1. svg-term-cli (Node-based, npm install -g svg-term-cli)
#      - Single static SVG, animated via SMIL — best size + portability.
#   2. agg (Go-based, github.com/asciinema/agg)
#      - Renders to GIF rather than SVG; bigger files but zero JS.
#
# The first SVG-capable tool on PATH is used. If neither is installed,
# this script prints install instructions and exits non-zero.
#
# Output layout:
#   docs/demos/casts/<n>-<name>.cast    (input from record-demos.sh)
#   docs/demos/<n>-<name>.svg           (inline-renderable artifact)

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CASTS_DIR="$REPO_ROOT/docs/demos/casts"
SVG_DIR="$REPO_ROOT/docs/demos"

if [ ! -d "$CASTS_DIR" ]; then
    echo "error: no $CASTS_DIR — run scripts/record-demos.sh first" >&2
    exit 1
fi

renderer=""
if command -v svg-term >/dev/null 2>&1; then
    renderer="svg-term"
elif command -v agg >/dev/null 2>&1; then
    renderer="agg"
else
    cat <<'EOF' >&2
error: no asciinema render tool found. Install one of:

  npm install -g svg-term-cli           (preferred — produces SVG)
  go install github.com/asciinema/agg@latest   (fallback — produces GIF)

Then rerun this script.
EOF
    exit 1
fi

shopt -s nullglob
casts=("$CASTS_DIR"/*.cast)
if [ ${#casts[@]} -eq 0 ]; then
    echo "warning: no .cast files in $CASTS_DIR — nothing to render" >&2
    exit 0
fi

for cast in "${casts[@]}"; do
    base="$(basename "$cast" .cast)"
    out_svg="$SVG_DIR/$base.svg"
    out_gif="$SVG_DIR/$base.gif"
    case "$renderer" in
        svg-term)
            echo "rendering $cast → $out_svg"
            # --window adds a terminal chrome; --no-cursor keeps the
            # render compact since the cast itself shows the prompt.
            svg-term --in "$cast" --out "$out_svg" --window
            ;;
        agg)
            echo "rendering $cast → $out_gif (svg-term not available; SVG preferred)"
            agg "$cast" "$out_gif"
            ;;
    esac
done

echo
echo "Render done. Output(s) in $SVG_DIR/"
echo "Embed inline in README.md with: ![demo](docs/demos/<name>.svg)"
