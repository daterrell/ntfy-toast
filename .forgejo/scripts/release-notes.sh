#!/bin/bash
# Print the release notes (Markdown) for a formal release or a development build.
#
#   release-notes.sh formal v1.2.3     # "What's new" since the previous formal release
#   release-notes.sh dev 1.2.4-dev.3   # what this build of main contains
#
# Env: FORGEJO_URL (web URL of the repo, for PR links), GITHUB_URL (for the compare link).
# Needs the repo's history and tags (checkout with fetch-depth: 0).
set -euo pipefail

kind=$1
ref=$2
forgejo=${FORGEJO_URL:-https://forgejo.terrell.ltd/dave/ntfy-toast}
github=${GITHUB_URL:-https://github.com/daterrell/ntfy-toast}

# "Merge pull request 'Title' (#7) from branch into main" -> "- Title ([PR 7](…/pulls/7))".
# Explicit links: a bare "#7" would point at GitHub's own (unrelated) issue numbers.
changes() {
  git log --first-parent --format='%s' "$1" | sed -E \
    -e "s|^Merge pull request '(.*)' \(#([0-9]+)\) from .*$|- \1 ([PR \2]($forgejo/pulls/\2))|" \
    -e '/^- /!s/^/- /'
}

install_notes() {
  cat <<'EOF'
## Install

1. Download `ntfy-toast.exe` below and move it somewhere permanent, e.g. `%LOCALAPPDATA%\Programs\ntfy-toast\`.
2. Run `ntfy-toast.exe init`, then edit `%APPDATA%\ntfy-toast\config.toml` to set your topics.
3. Run `ntfy-toast.exe install` to start it now and at every login.

## Updating

ntfy-toast offers new releases itself: a notification with an **Install** button. You can also run `ntfy-toast.exe update`.

The exe isn't code-signed, so Windows SmartScreen may warn on first run: choose **More info → Run anyway**.
Verify the download with `ntfy-toast.exe.sha256` (`Get-FileHash ntfy-toast.exe`).
EOF
}

case $kind in
  formal)
    prev=$(git describe --tags --abbrev=0 --match 'v[0-9]*' --exclude '*-dev.*' "$ref^" 2>/dev/null || true)
    echo "## What's new"
    echo
    # Only merges (the PRs) when there are any; otherwise every commit.
    range=${prev:+$prev..}$ref
    if [ -n "$(git log --first-parent --merges --format=%H "$range")" ]; then
      git log --first-parent --merges --format='%H' "$range" | while read -r c; do changes "$c^!"; done
    else
      changes "$range"
    fi
    echo
    install_notes
    if [ -n "$prev" ]; then
      echo
      echo "**Full changelog:** [$prev...$ref]($github/compare/$prev...$ref)"
    fi
    ;;
  dev)
    sha=$(git rev-parse --short HEAD)
    cat <<EOF
Built automatically from \`main\` at [\`$sha\`]($forgejo/commit/$(git rev-parse HEAD)).
This is a development build, not a formal release: self-update offers it only to copies
with \`channel = "dev"\` in the \`[update]\` section of \`config.toml\`.

## Changes in this build

EOF
    changes "HEAD^!"
    echo
    install_notes
    ;;
  *)
    echo "usage: $0 formal <tag> | dev <version>" >&2
    exit 2
    ;;
esac
