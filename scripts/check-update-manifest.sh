#!/usr/bin/env bash
# Checks an update manifest the way installed apps will use it: every platform is
# there and announces the expected version, and its package downloads, is the right
# kind of package for that platform, and carries a signature made with OmniPanel's
# updater key for exactly that version.
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

# The package file an installed copy of each kind must be given. A signature names the
# file it was made for, inside the part that is signed.
package_for() {
  case "$1" in
    linux-x86_64 | linux-x86_64-appimage) echo '*_amd64.AppImage' ;;
    linux-x86_64-deb) echo '*_amd64.deb' ;;
    linux-x86_64-rpm) echo '*.x86_64.rpm' ;;
    linux-aarch64 | linux-aarch64-appimage) echo '*_aarch64.AppImage' ;;
    linux-aarch64-deb) echo '*_arm64.deb' ;;
    linux-aarch64-rpm) echo '*.aarch64.rpm' ;;
    windows-x86_64 | windows-x86_64-nsis) echo '*_x64-setup.exe' ;;
    darwin-*) echo '*.app.tar.gz' ;; # one universal bundle
    *) fail "no package is known for $1" ;;
  esac
}

[ "$(jq -er .version "$manifest")" = "$version" ] || fail "latest.json is not for $version"

auth=()
[ -z "${GH_TOKEN:-}" ] || auth=(-H "Authorization: Bearer $GH_TOKEN")

# What installed copies look up: the installer-specific entry first, then the short one.
for target in linux-{x86_64,aarch64}{,-appimage,-deb,-rpm} windows-x86_64{,-nsis} darwin-{aarch64,x86_64}{,-app}; do
  url="$(jq -er --arg t "$target" '.platforms[$t].url' "$manifest")" || fail "latest.json has no $target"
  signature="$(jq -er --arg t "$target" '.platforms[$t].signature' "$manifest")" || fail "latest.json has no signature for $target"
  # Several platforms share a package: download and verify each one once.
  checked="$work/$(printf '%s %s' "$url" "$signature" | cksum | tr -d ' ')"
  if [ ! -e "$checked" ]; then
    echo "$target: $url"
    curl -fsSL ${auth[@]+"${auth[@]}"} -H "Accept: application/octet-stream" -o "$work/package" "$url" || fail "$target does not download"
    base64 -d <<<"$signature" >"$work/package.minisig"
    # -Q prints the trusted comment, which the signature covers: tab-separated key:value fields.
    minisign -VQ -P "$pubkey" -m "$work/package" -x "$work/package.minisig" >"$checked" || fail "$target is not signed with OmniPanel's key"
  fi
  signed=$'\t'"$(cat "$checked")"$'\t'
  # Apps set requireSignedVersion: they refuse a signature made for another version.
  [[ "$signed" == *$'\t'"version:$version"$'\t'* ]] || fail "$target is signed for another version:$signed"
  [[ "$signed" == *$'\t'"file:"$(package_for "$target")$'\t'* ]] || fail "$target points at the wrong package:$signed"
done

echo "latest.json $version: every platform is present and verified"
