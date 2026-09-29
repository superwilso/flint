#!/usr/bin/env bash
# Cut a Flint release: prepare everything in one pass, then tag, push and watch it publish.
#
#   tools/release.sh v0.2.0              prepare, or (once that is committed) push, tag and publish
#   tools/release.sh v0.2.0-rc1          the same, published as a PRE-release (any `-suffix`)
#   tools/release.sh v0.2.0 --dry-run    say what would change; edit nothing, tag nothing
#   tools/release.sh v0.2.0 --no-watch   tag and push, but do not wait for GitHub to finish
#
# Modelled on Cinder's tools/release.sh, and for the same reason: `git tag && git push` is two
# commands, but a release is a dozen facts that all have to be true at once — the version in
# Cargo.toml, a CHANGELOG section for it (the release page is built from it), README pictures of
# the window as it now is, and every gate green — and each one is the kind of step that gets
# forgotten at the end of a release rather than the kind worth doing by hand.
#
# ── TWO RUNS, ONE COMMIT ────────────────────────────────────────────────────────────────────────
#
#     run 1   bump the version, roll the changelog, re-draw the README's window pictures, run
#             every gate → review the diff, commit (the script prints the command)
#     run 2   everything already matches → re-run the gates, push main, tag, push the tag, and
#             wait for the release workflow to publish; prints the release page at the end
#
# It never commits. Staging and committing stay yours: the diff is the last point at which a human
# looks at what is about to ship. Everything after that commit it does for you.

set -euo pipefail
cd "$(dirname "$0")/.."

TAG="${1:-}"
DRY=""
WATCH=1
for arg in "${@:2}"; do
    case "$arg" in
        --dry-run) DRY=1 ;;
        --no-watch) WATCH="" ;;
        *) printf 'unknown option %s\n' "$arg" >&2; exit 2 ;;
    esac
done

die() { printf '\n\033[1;31m✗ %s\033[0m\n' "$*" >&2; exit 1; }
ok()  { printf '\033[1;32m✓\033[0m %s\n' "$*"; }
note(){ printf '  %s\n' "$*"; }
act() { printf '\033[1;33m~\033[0m %s\n' "$*"; }
joined() { local out="" x; for x in "$@"; do out="${out:+$out, }$x"; done; printf '%s' "$out"; }

[ -n "$TAG" ] || die "usage: tools/release.sh vX.Y.Z[-rcN] [--dry-run] [--no-watch]"
[[ "$TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] \
    || die "tag must look like v1.2.3 or v1.2.3-rc1 — the release workflow only triggers on 'v*'"
VER="${TAG#v}"; VER="${VER%%-*}"
PRE=""
[[ "$TAG" == *-* ]] && PRE=1

BRANCH="$(git rev-parse --abbrev-ref HEAD)"
[ "$BRANCH" = main ] || die "releases are cut from main; this is '$BRANCH'"

# A tag that exists only on THIS machine is a push that did not go through: it was never published,
# so it is re-made at the release commit. A tag origin has is a release people may already have
# downloaded — never move it.
STALE_TAG=""
if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
    REMOTE_TAG="$(git ls-remote --tags origin "refs/tags/$TAG")" \
        || die "$TAG exists here and origin could not be asked whether it was pushed — check the network"
    [ -n "$REMOTE_TAG" ] && die "$TAG is already on origin — pick a new version"
    STALE_TAG=1
    note "$TAG exists only on this machine (a push that did not go through) — it is re-made at the release commit"
fi

CHANGED=()
DIRTY_AT_START=""
[ -n "$(git status --porcelain)" ] && DIRTY_AT_START=1

# ── 1. the workspace version must match the tag ─────────────────────────────────────────────
# One number for all four crates (`version.workspace = true`), and Cargo.lock carries it for each.
HAVE="$(sed -n '/^\[workspace.package\]/,/^\[/ s/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)"
[ -n "$HAVE" ] || die "Cargo.toml has no [workspace.package] version line — did its shape change?"
if [ "$HAVE" != "$VER" ]; then
    if [ -z "$DRY" ]; then
        sed -i "/^\[workspace.package\]/,/^\[/ s/^version = \"$HAVE\"$/version = \"$VER\"/" Cargo.toml
        for crate in flint flint-core flint-gui sensme-helper; do
            sed -i "/^name = \"$crate\"$/,/^version = / s/^version = \".*\"$/version = \"$VER\"/" Cargo.lock
        done
    fi
    act "version $HAVE → $VER (Cargo.toml + Cargo.lock)${DRY:+  [would]}"
    CHANGED+=("version")
else
    ok "version $HAVE matches $TAG"
fi

# ── 2. roll the changelog's Unreleased section into this version ────────────────────────────
# The release page's "What's new" IS this section (tools/render_release_notes.sh), so a tag whose
# notes are still filed under Unreleased would publish with nothing there — the renderer refuses.
# Rolled once: on the second run the `## X.Y.Z` heading exists and this is a no-op.
CL=CHANGELOG.md
TODAY="$(date +%F)"
if grep -q "^## $VER\\b" "$CL"; then
    ok "changelog already has a $VER section"
elif ! grep -q '^## Unreleased' "$CL"; then
    die "CHANGELOG.md has neither a '## $VER' nor an '## Unreleased' section — write the entry for $TAG first"
else
    BODY="$(awk '/^## Unreleased/{f=1;next} /^## /{f=0} f' "$CL" | tr -d '[:space:]')"
    [ -n "$BODY" ] || die "CHANGELOG.md's Unreleased section is empty — write the entry for $TAG first"
    if [ -z "$DRY" ]; then
        awk -v ver="$VER" -v today="$TODAY" '
            !done && /^## Unreleased/ { print "## Unreleased"; print ""; print "## " ver " — " today; done = 1; next }
            { print }
        ' "$CL" > "$CL.tmp" && mv "$CL.tmp" "$CL"
    fi
    act "changelog: Unreleased → $VER — $TODAY (and a fresh Unreleased above it)${DRY:+  [would]}"
    CHANGED+=("changelog")
fi

# ── 3. the README points at the latest release ──────────────────────────────────────────────
# Only for a final release: a pre-release is not "latest" on GitHub, and the link says it is.
README=README.md
if [ -z "$PRE" ]; then
    LINK_HAVE="$(sed -n 's#.*\[v\([0-9.]*\)\](https://github.com/superwilso/flint/releases/latest).*#\1#p' "$README" | head -1)"
    if [ -z "$LINK_HAVE" ]; then
        note "README has no [vX.Y.Z](…/releases/latest) link — leaving it alone"
    elif [ "$LINK_HAVE" != "$VER" ]; then
        [ -z "$DRY" ] && sed -i "s#\[v$LINK_HAVE\](https://github.com/superwilso/flint/releases/latest)#[v$VER](https://github.com/superwilso/flint/releases/latest)#" "$README"
        act "README release link v$LINK_HAVE → v$VER${DRY:+  [would]}"
        CHANGED+=("README link")
    else
        ok "README links v$VER"
    fi
fi

# ── 4. the README's pictures of the window must be the window this release ships ────────────
# They are `flint gui-preview` output — the same command list the window paints — so re-drawing
# them is exact, and anything that moved joins the one release commit.
note "building flint and re-drawing the window pictures …"
cargo build -q -p flint || die "flint does not build"
PICS=(
    "docs/window.svg:planned:"
    "docs/window-dark.svg:planned:--dark"
    "docs/window-check.svg:check:"
    "docs/window-palettes.svg:palettes:"
    "docs/window-palette-new.svg:palette-new:"
)
SHOTS_TMP="$(mktemp -d)"
trap 'rm -rf "$SHOTS_TMP"' EXIT
STALE_PICS=()
for p in "${PICS[@]}"; do
    IFS=: read -r file state flag <<<"$p"
    out="$SHOTS_TMP/$(basename "$file")"
    # shellcheck disable=SC2086
    "${CARGO_TARGET_DIR:-target}/debug/flint" gui-preview "$out" --state "$state" $flag >/dev/null \
        || die "gui-preview --state $state failed"
    if ! cmp -s "$out" "$file"; then
        STALE_PICS+=("$file")
        [ -z "$DRY" ] && cp "$out" "$file"
    fi
done
if [ ${#STALE_PICS[@]} -gt 0 ]; then
    act "window pictures re-drawn: $(joined "${STALE_PICS[@]}")${DRY:+  [would]}"
    CHANGED+=("window pictures")
else
    ok "window pictures match the window"
fi

# ── 5. the gates — the same ones CI runs, plus the two Windows builds a release ships ───────
# Run here, before anything is tagged, because the release workflow finding a failure means a tag
# on origin with no release behind it.
note "running the gates …"
cargo fmt --all -- --check >/dev/null 2>&1 || die "cargo fmt --check fails — run cargo fmt --all"
ok "formatted"
cargo clippy -q --workspace --all-targets -- -D clippy::correctness -D clippy::suspicious >/dev/null 2>&1 \
    || die "clippy's correctness or suspicious lints fail — cargo clippy --workspace --all-targets"
ok "clippy clean"
cargo test -q --workspace >/tmp/flint-release-test.log 2>&1 \
    || { tail -20 /tmp/flint-release-test.log; die "tests fail — see /tmp/flint-release-test.log"; }
ok "tests pass ($(grep -o '[0-9]* passed' /tmp/flint-release-test.log | awk '{s+=$1} END {print s}'))"
for state in fresh ready planned working "done" scanning player check filtered sensme likes palettes settings signed-in; do
    "${CARGO_TARGET_DIR:-target}/debug/flint" gui-preview "$SHOTS_TMP/gui-$state.svg" --state "$state" >/dev/null \
        || die "the window cannot be drawn in state $state"
done
ok "the window draws in every state"
# The workflow builds these on Windows with MSVC; mingw here catches a Windows-only compile error
# (the window is Win32 and never compiled on Linux otherwise) before it costs a tag.
if rustup target list --installed 2>/dev/null | grep -qx x86_64-pc-windows-gnu; then
    cargo build -q --release --target x86_64-pc-windows-gnu -p flint \
        || die "flint does not build for Windows"
    ok "the window (flint-window.exe) and the command line (flint.exe) build for Windows"
else
    note "no x86_64-pc-windows-gnu target here — the Windows build is left to the workflow"
fi
if rustup target list --installed 2>/dev/null | grep -qx i686-pc-windows-gnu; then
    cargo build -q --release --target i686-pc-windows-gnu -p sensme-helper \
        || die "sensme-helper.exe does not build for 32-bit Windows"
    ok "sensme-helper.exe builds for 32-bit Windows"
else
    note "no i686-pc-windows-gnu target here — the helper build is left to the workflow"
fi
# The release page, rendered now with this tag, so a template or changelog problem shows here and
# not as a failed publish. In a dry run the changelog has not been rolled yet, so it is skipped.
if [ -z "$DRY" ]; then
    RELEASE_TAG="$TAG" tools/render_release_notes.sh --preview "$TAG" >"$SHOTS_TMP/body.md" \
        || die "the release notes do not render — tools/render_release_notes.sh --preview $TAG"
    ok "release notes render ($(wc -l <"$SHOTS_TMP/body.md") lines, What's new from CHANGELOG $VER)"
fi

# ── 6. stop and hand the diff over, or publish ──────────────────────────────────────────────
if [ -n "$DRY" ]; then
    if [ ${#CHANGED[@]} -gt 0 ]; then
        printf '\n\033[1;33m--dry-run: preparing %s would change: %s.\033[0m\n' "$TAG" "$(joined "${CHANGED[@]}")"
        printf 'Nothing was edited and nothing was tagged. Re-run without --dry-run to do it.\n\n'
    else
        printf '\n\033[1;33m--dry-run: everything is already prepared for %s. Nothing was tagged.\033[0m\n\n' "$TAG"
    fi
    exit 0
fi

if [ -n "$(git status --porcelain)" ]; then
    printf '\n'
    if [ ${#CHANGED[@]} -gt 0 ]; then
        ok "prepared $TAG: $(joined "${CHANGED[@]}")"
    else
        note "the tree was already dirty before this run — nothing here changed it"
    fi
    printf '\n\033[1mReview, then commit — everything %s needs is in this one diff:\033[0m\n\n' "$TAG"
    git status --short | sed 's/^/    /'
    printf '\n    git add -A && git commit -m "release: prepare %s"\n' "$TAG"
    printf '    tools/release.sh %s\n\n' "$TAG"
    [ -n "$DIRTY_AT_START" ] && printf '  (the tree already had uncommitted work when this started — check the list above\n   is only what you meant to ship.)\n\n'
    exit 0
fi
ok "working tree clean — everything for $TAG is committed"

# ── 7. push main, then tag and push the tag ─────────────────────────────────────────────────
# main first: a tag pointing at a commit origin/main does not have is a release built from a
# commit nobody can find on the branch.
git fetch -q origin main || die "could not fetch origin/main — check the network"
git merge-base --is-ancestor origin/main HEAD \
    || die "origin/main has commits this main does not — pull them in first (git pull --rebase)"
if [ "$(git rev-parse HEAD)" != "$(git rev-parse origin/main)" ]; then
    git push origin main || die "pushing main was refused — nothing was tagged"
    ok "pushed main"
else
    ok "main is already on origin"
fi

if [ -n "$STALE_TAG" ]; then
    git tag -d "$TAG" >/dev/null
    note "removed the unpushed $TAG left by an earlier run"
fi
git tag -a "$TAG" -m "Flint $TAG"
ok "tagged $TAG"
git push origin "$TAG" \
    || die "the tag push was refused, so $TAG exists only here — fix what it says and run tools/release.sh $TAG again (the tag is re-made)"
ok "pushed $TAG"

REPO="$(git remote get-url origin | sed -e 's#.*github.com[:/]##' -e 's#\.git$##')"
PAGE="https://github.com/$REPO/releases/tag/$TAG"

# ── 8. watch it publish ─────────────────────────────────────────────────────────────────────
if [ -z "$WATCH" ] || ! command -v gh >/dev/null || ! gh auth status >/dev/null 2>&1; then
    printf '\nGitHub is now building the release. Watch it:\n    https://github.com/%s/actions\n' "$REPO"
    printf 'It appears at:\n    %s\n\n' "$PAGE"
    exit 0
fi
note "waiting for the release workflow to start …"
RUN=""
for _ in $(seq 1 30); do
    RUN="$(gh run list -R "$REPO" --workflow release.yml --branch "$TAG" --limit 1 --json databaseId -q '.[0].databaseId' 2>/dev/null || true)"
    [ -n "$RUN" ] && break
    sleep 4
done
[ -n "$RUN" ] || die "no release run appeared for $TAG after two minutes — https://github.com/$REPO/actions"
gh run watch "$RUN" -R "$REPO" --exit-status >/dev/null \
    || die "the release workflow failed — gh run view $RUN -R $REPO --log-failed"
ok "release workflow finished"
gh release view "$TAG" -R "$REPO" --json assets -q '.assets[].name' | sed 's/^/    /'
printf '\n\033[1;32m%s is out%s:\033[0m\n    %s\n\n' "$TAG" "${PRE:+ (pre-release)}" "$PAGE"
