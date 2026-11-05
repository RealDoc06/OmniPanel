#!/usr/bin/env bash
# Builds release installers from a Linux x86_64 machine using Docker toolchains.
#
#   scripts/build-release.sh <linux-x64|linux-arm64|windows-x64|all>
#
# Output: release-artifacts/<version>/ plus SHA256SUMS. Needs Docker, Node and pnpm.
# Reruns are incremental: images, the cargo registry and target/docker-<name> are cached.
# macOS cannot be built here (no SDK); the release workflow builds it.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
version="$(node -p "require('./apps/desktop/src-tauri/tauri.conf.json').version")"
out="release-artifacts/$version"
home="target/docker-home" # cargo registry and Tauri's downloaded bundler tools

# Frontends are platform independent, so they are built once on the host. pnpm
# inside a container would relink the node_modules the host is using.
build_frontend() {
  pnpm install --frozen-lockfile
  pnpm build:deck
  pnpm build:ui
}

# build_target <name> <rust triple> <tauri bundles> <file glob...>
build_target() {
  local name="$1" triple="$2" bundles="$3"
  shift 3
  local target_dir="target/docker-$name" release_dir="target/docker-$name/$triple/release"

  docker build -q -f packaging/Dockerfile --target "$name" -t "omnipanel-builder:$name" packaging
  mkdir -p "$home" "$out"

  # rust-embed does not notice added or removed files, so force the crates that
  # embed the frontends to recompile. Old bundles would otherwise be collected too.
  rm -rf "$release_dir"/.fingerprint/omni-host-* "$release_dir"/.fingerprint/omnipanel-* "$release_dir/bundle"

  # APPIMAGE_EXTRACT_AND_RUN: containers have no FUSE for Tauri's AppImage tools.
  # Same absolute path as the host, because pnpm's bin shims and cargo metadata embed it.
  docker run --rm --name "omnipanel-build-$name" \
    -u "$(id -u):$(id -g)" -v "$root:$root" -w "$root/apps/desktop" \
    -e HOME="$root/$home" -e CARGO_HOME="$root/$home/.cargo" -e CARGO_TARGET_DIR="$root/$target_dir" \
    -e CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" -e APPIMAGE_EXTRACT_AND_RUN=1 \
    "omnipanel-builder:$name" \
    node node_modules/@tauri-apps/cli/tauri.js build --ci --target "$triple" --bundles "$bundles" \
      --config '{"build":{"beforeBuildCommand":null}}'

  local pattern
  for pattern in "$@"; do
    find "$release_dir/bundle" -type f -name "$pattern" -exec cp -f {} "$out/" \;
  done
}

build_one() {
  case "$1" in
    linux-x64) build_target linux-x64 x86_64-unknown-linux-gnu deb,rpm,appimage '*.deb' '*.rpm' '*.AppImage' ;;
    # AppImage is left out: linuxdeploy-aarch64 cannot run on x86_64 without a binfmt handler.
    linux-arm64) build_target linux-arm64 aarch64-unknown-linux-gnu deb,rpm '*.deb' '*.rpm' ;;
    windows-x64) build_target windows-x64 x86_64-pc-windows-gnu nsis '*-setup.exe' ;;
    *) echo "usage: $0 <linux-x64|linux-arm64|windows-x64|all>" >&2; exit 2 ;;
  esac
}

target="${1:-}"
[ -n "$target" ] || build_one help
build_frontend
if [ "$target" = all ]; then
  for t in linux-x64 linux-arm64 windows-x64; do build_one "$t"; done
else
  build_one "$target"
fi

# Checksums cover everything built so far for this version, not only this run.
(cd "$out" && find . -maxdepth 1 -type f ! -name SHA256SUMS -printf '%f\n' | sort | xargs sha256sum > SHA256SUMS)
ls -lh "$out"
