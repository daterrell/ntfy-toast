#!/bin/bash
# Delete all but the newest $KEEP development builds (pre-releases tagged v*-dev.*) and
# their tags. Formal releases, including formal pre-releases like v1.0.0-rc.1, are never
# touched.
#
# Env: API (e.g. https://forgejo.example/api/v1/repos/owner/name), TOKEN, KEEP (default 2).
set -euo pipefail

auth=(-H "Authorization: token $TOKEN")
keep=${KEEP:-2}

dev=$(curl -sSf "${auth[@]}" "$API/releases?limit=50&page=1" |
  jq -r '.[] | select(.prerelease and (.tag_name | test("^v[0-9]+\\.[0-9]+\\.[0-9]+.*-dev\\.[0-9]+$"))) | "\(.id) \(.tag_name)"' |
  sort -k2 -V -r)

echo "$dev" | tail -n +$((keep + 1)) | while read -r id tag; do
  [ -n "$id" ] || continue
  curl -sS --fail-with-body -X DELETE "${auth[@]}" "$API/releases/$id"
  curl -sS --fail-with-body -X DELETE "${auth[@]}" "$API/tags/$tag"
  echo "deleted development build $tag"
done
