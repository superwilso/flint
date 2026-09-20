#!/usr/bin/env bash
# render_release_notes.sh — build the GitHub release body, with the real checksums inlined.
#
#   tools/render_release_notes.sh <SHA256SUMS file> [output]     write the body (default stdout)
#   tools/render_release_notes.sh --preview                      render with placeholder sums
#
# The same file the release workflow runs, so `--preview` shows exactly what will be published.
# That is the rule this pattern exists for: a document that is only ever rendered during a release
# is one that is wrong for the first time during a release. Borrowed wholesale from Cinder's
# `tools/render_release_notes.sh`, which is where it was learned the hard way — that repository
# shipped a workflow whose comment promised inlined checksums above a step that did not inline
# them, and nobody saw it because a release body is invisible until it is published.
set -uo pipefail
cd "$(dirname "$0")/.." || { echo "cannot reach the repo root" >&2; exit 2; }

TEMPLATE=".github/release-notes.md"
MARKER="{{SHA256SUMS}}"

if [ "${1:-}" = "--preview" ]; then
    SUMS="$(mktemp)"
    trap 'rm -f "$SUMS"' EXIT
    for f in flint-windows-x64.exe sensme-helper-x86.exe flint-linux-x64; do
        printf '%s  %s  (placeholder — nothing built yet)\n' \
            "0000000000000000000000000000000000000000000000000000000000000000" "$f" >> "$SUMS"
    done
    OUT="-"
else
    SUMS="${1:-}"
    OUT="${2:--}"
fi

[ -n "$SUMS" ] || { echo "usage: tools/render_release_notes.sh <SHA256SUMS> [out] | --preview" >&2; exit 2; }
[ -f "$TEMPLATE" ] || { echo "missing template: $TEMPLATE" >&2; exit 2; }
[ -f "$SUMS" ]     || { echo "missing checksums: $SUMS" >&2; exit 2; }
[ -s "$SUMS" ]     || { echo "checksum file is empty: $SUMS" >&2; exit 1; }

# FAIL rather than publish a release whose verification section is blank. A body with an empty code
# block under "Verifying the download" is worse than no section at all — it looks like the check
# was done.
grep -qF "$MARKER" "$TEMPLATE" || {
    echo "template $TEMPLATE no longer contains the $MARKER line — nothing to substitute" >&2
    exit 1
}

render() {
    # Drop the HTML comment header: it is guidance for whoever edits the template, not for the
    # people reading the release. Everything from the first '## ' onward is the body.
    awk -v sums="$SUMS" -v marker="$MARKER" '
        !started && /^## / { started = 1 }
        !started { next }
        index($0, marker) {
            while ((getline line < sums) > 0) print line
            close(sums)
            next
        }
        { print }
    ' "$TEMPLATE"
}

if [ "$OUT" = "-" ]; then
    render
    exit 0
fi

render > "$OUT" || exit 1

# COUNT THE HASHES rather than just checking the file was non-empty. A SHA256SUMS that exists and
# has bytes in it can still be junk — a truncated upload, an error message, a file listing — and
# the body would publish a code block that looks like verification and is not.
n="$(grep -c '^[0-9a-f]\{64\} \{1,2\}' "$OUT")"
if [ "$n" -lt 1 ]; then
    echo "rendered $OUT but it contains no sha256 lines — refusing to publish an empty" >&2
    echo "verification section (is $SUMS actually sha256sum output?)" >&2
    exit 1
fi
echo "wrote $OUT ($(wc -l < "$OUT") lines, $n checksums)"
