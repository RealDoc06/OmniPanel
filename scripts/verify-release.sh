#!/usr/bin/env bash
# Checks the installers from scripts/build-release.sh in clean containers.
#
#   scripts/verify-release.sh [deb-debian|deb-ubuntu|rpm|appimage|windows|arm64 ...]
#
# With no arguments runs every check whose artefact exists and reports PASS or FAIL.
set -uo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
version="$(node -p "require('./apps/desktop/src-tauri/tauri.conf.json').version")"
dir="$root/release-artifacts/$version"
smoke_tools_apt="xvfb xauth dbus curl"

# clean_run <check name> <image> <shell script>: artefacts at /a, smoke script at /smoke.sh
clean_run() {
  docker run --rm --name "omnipanel-verify-$1" -v "$dir:/a:ro" -v "$root/packaging/smoke-linux.sh:/smoke.sh:ro" \
    -e version="$version" -e DEBIAN_FRONTEND=noninteractive "$2" bash -c "$3"
}

verify_deb() { # <check name> <image>
  clean_run "$1" "$2" "set -e
    apt-get update -qq
    apt-get install -y -qq --no-install-recommends /a/*_amd64.deb $smoke_tools_apt >/dev/null
    command -v pactl
    ldd /usr/bin/omnipanel | (! grep 'not found')
    bash /smoke.sh \$version omnipanel"
}

verify_rpm() {
  clean_run rpm fedora:latest "set -e
    dnf install -y -q /a/*.x86_64.rpm xorg-x11-server-Xvfb xorg-x11-xauth dbus-daemon curl-minimal >/dev/null
    command -v pactl
    ldd /usr/bin/omnipanel | (! grep 'not found')
    bash /smoke.sh \$version omnipanel"
}

# The AppImage leaves out libraries every desktop has (AppImage's exclude list: X11, GL, GLES,
# fonts, Wayland, ALSA), so the container gets exactly those; the AppImage must carry the rest.
desktop_base_apt="libx11-6 libx11-xcb1 libxcb1 libgl1 libegl1 libgbm1 libgles2 libdrm2 libexpat1 libfontconfig1 libfreetype6 libfribidi0 libharfbuzz0b libwayland-client0 libasound2t64"
verify_appimage() {
  clean_run appimage ubuntu:24.04 "set -e
    apt-get update -qq
    apt-get install -y -qq --no-install-recommends $smoke_tools_apt $desktop_base_apt >/dev/null
    bash /smoke.sh \$version /a/*_amd64.AppImage --appimage-extract-and-run"
}

verify_windows() {
  docker run --rm --name omnipanel-verify-windows -v "$dir:/a:ro" -v "$root/apps:/apps:ro" \
    -e DEBIAN_FRONTEND=noninteractive omnipanel-builder:windows-x64 bash -c '
    set -e
    apt-get install -y -qq --no-install-recommends 7zip >/dev/null
    setup=$(ls /a/*-setup.exe); file "$setup"
    mkdir /x; 7z x -o/x "$setup" >/dev/null; (cd /x; find . -type f | sort)
    exe=/x/omnipanel.exe; file "$exe"
    x86_64-w64-mingw32-objdump -p "$exe" | grep "DLL Name" | sort -u
    ! x86_64-w64-mingw32-objdump -p "$exe" | grep -iE "DLL Name: (libgcc|libwinpthread|libstdc)"
    grep -qaF "$(head -c 60 /apps/deck/dist/app.js)" "$exe"; echo deck assets embedded
    for f in /apps/desktop/dist/assets/*.js; do grep -qaF "assets/$(basename "$f")" "$exe"; done; echo frontend assets embedded'
}

verify_arm64() {
  file "$dir"/*arm64.deb "$dir"/*.aarch64.rpm
  docker run --rm --name omnipanel-verify-arm64 -v "$dir:/a:ro" omnipanel-builder:linux-arm64 bash -c '
    set -e
    mkdir /x; dpkg-deb -x /a/*_arm64.deb /x; file /x/usr/bin/omnipanel
    dpkg-deb -f /a/*_arm64.deb Depends'
}

failed=0
check() {
  echo "=== $1"
  case "$1" in
    deb-debian) verify_deb deb-debian debian:13 ;;
    deb-ubuntu) verify_deb deb-ubuntu ubuntu:24.04 ;;
    rpm) verify_rpm ;;
    appimage) verify_appimage ;;
    windows) verify_windows ;;
    arm64) verify_arm64 ;;
    *) echo "unknown check $1"; false ;;
  esac
  if [ $? = 0 ]; then echo "PASS $1"; else echo "FAIL $1"; failed=1; fi
}

checks=("$@")
[ ${#checks[@]} -gt 0 ] || checks=(deb-debian deb-ubuntu rpm appimage windows arm64)
for c in "${checks[@]}"; do check "$c"; done
exit "$failed"
