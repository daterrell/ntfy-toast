#!/bin/bash
# Create the Forgejo release for $TAG (or reuse one made on the website) and attach the
# given files, replacing any with the same name. ntfy-toast's self-update reads these.
#
# Env: API (e.g. https://forgejo.example/api/v1/repos/owner/name), TOKEN, TAG, TITLE,
#      NOTES_FILE (optional), PRERELEASE (optional, "true" for a pre-release).
set -euo pipefail

auth=(-H "Authorization: token $TOKEN")

if ! rel=$(curl -sf "${auth[@]}" "$API/releases/tags/$TAG"); then
  body=$(jq -n --arg tag "$TAG" --arg name "$TITLE" --rawfile notes "${NOTES_FILE:-/dev/null}" \
    --argjson pre "${PRERELEASE:-false}" '{tag_name: $tag, name: $name, body: $notes, prerelease: $pre}')
  rel=$(curl -sS --fail-with-body "${auth[@]}" -H "Content-Type: application/json" -d "$body" "$API/releases")
  echo "created Forgejo release $TAG"
fi
id=$(jq -r .id <<<"$rel")

for file in "$@"; do
  name=$(basename "$file")
  for old in $(jq -r --arg n "$name" '.assets[] | select(.name == $n) | .id' <<<"$rel"); do
    curl -sS --fail-with-body -X DELETE "${auth[@]}" "$API/releases/$id/assets/$old"
  done
  curl -sS --fail-with-body -o /dev/null "${auth[@]}" -F "attachment=@$file" \
    "$API/releases/$id/assets?name=$name"
  echo "attached $name"
done
