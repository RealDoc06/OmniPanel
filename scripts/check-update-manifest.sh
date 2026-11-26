#!/usr/bin/env bash
# Checks an update manifest the way installed apps will use it: every platform is
# there, announces the expected version, and its package downloads and carries a
# signature made with OmniPanel's updater key for exactly that version.
#
#   scripts/check-update-manifest.sh <latest.json> <version>
#
# With GH_TOKEN set the downloads are authenticated (the packages of a draft release
# are private). Without it they are fetched the way the app fetches them.
# Needs jq, curl and minisign. Used by .github/workflows/release.yml.
set -euo pipefail

manifest="$1"
version="$2"
conf="$(dirname "$0")/../apps/desktop/src-tauri/tauri.conf.json"
pubkey="$(jq -er .plugins.updater.pubkey "$conf" | base64 -d | tail -n 1)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

fail() {
  echo "::error::$*"
  exit 1
}

[ "$(jq -er .version "$manifest")" = "$version" ] || fail "latest.json is not for $version"

# What installed copies look up: the installer-specific entry first, then the short one.
for target in linux-{x86_64,aarch64}{,-appimage,-deb,-rpm} windows-x86_64{,-nsis} darwin-{aarch64,x86_64}{,-app}; do
  jq -e --arg t "$target" '.platforms | has($t)' "$manifest" >/dev/null || fail "latest.json has no $target"
done

auth=()
[ -z "${GH_TOKEN:-}" ] || auth=(-H "Authorization: Bearer $GH_TOKEN")

# One download per distinct package, however many platforms point at it.
packages="$(jq -r '.platforms | to_entries | unique_by([.value.url, .value.signature])[] | [.key, .value.url, .value.signature] | @tsv' "$manifest")"
while IFS=$'\t' read -r target url signature; do
  echo "$target: $url"
  curl -fsSL "${auth[@]}" -H "Accept: application/octet-stream" -o "$work/package" "$url" || fail "$target does not download"
  base64 -d <<<"$signature" >"$work/package.minisig"
  # -Q prints the trusted comment, which the signature covers.
  comment="$(minisign -VQ -P "$pubkey" -m "$work/package" -x "$work/package.minisig")" || fail "$target is not signed with OmniPanel's key"
  # Apps set requireSignedVersion: they refuse a signature made for another version.
  [[ "$comment" == *$'\t'"version:$version" ]] || fail "$target is signed for another version ($comment)"
done <<<"$packages"

echo "latest.json $version: every platform is present and verified"
