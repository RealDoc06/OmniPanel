# Cross-build toolchains for scripts/build-release.sh. One stage per target:
#   docker build -f packaging/Dockerfile --target <linux-x64|linux-arm64|windows-x64> packaging
# Sources are bind-mounted at run time, so the images only hold tools.

# Linux builds use Ubuntu 22.04: same glibc baseline as the release CI, so
# packages run on every distro at least that new.
FROM ubuntu:22.04 AS linux-base
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:/opt/node/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl xz-utils file pkg-config build-essential patchelf desktop-file-utils xdg-utils \
 && curl -fsSL https://nodejs.org/dist/v24.22.1/node-v24.22.1-linux-x64.tar.xz | (mkdir -p /opt/node && tar -xJ --strip-components=1 -C /opt/node) \
 && curl -fsSL https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal --default-toolchain 1.98.1 \
 && chmod -R a+rX /opt

FROM linux-base AS linux-x64
RUN apt-get install -y --no-install-recommends libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libasound2-dev libxdo-dev libxkbcommon-dev libssl-dev

# Multiarch: amd64 tools plus arm64 dev libraries from ports.ubuntu.com, which
# is where Ubuntu publishes arm64. qemu-user only runs test binaries directly;
# no binfmt registration, which would change the host kernel.
FROM linux-base AS linux-arm64
ENV PKG_CONFIG_ALLOW_CROSS=1 PKG_CONFIG_PATH=/usr/lib/aarch64-linux-gnu/pkgconfig:/usr/share/pkgconfig PKG_CONFIG_SYSROOT_DIR=/ \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++ \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
RUN dpkg --add-architecture arm64 \
 && sed -i 's/^deb /deb [arch=amd64] /' /etc/apt/sources.list \
 && printf 'deb [arch=arm64] http://ports.ubuntu.com/ubuntu-ports jammy main universe\ndeb [arch=arm64] http://ports.ubuntu.com/ubuntu-ports jammy-updates main universe\ndeb [arch=arm64] http://ports.ubuntu.com/ubuntu-ports jammy-security main universe\n' > /etc/apt/sources.list.d/arm64.list \
 && apt-get update \
 && apt-get install -y --no-install-recommends gcc-aarch64-linux-gnu g++-aarch64-linux-gnu qemu-user \
      libwebkit2gtk-4.1-dev:arm64 libayatana-appindicator3-dev:arm64 librsvg2-dev:arm64 libasound2-dev:arm64 libxdo-dev:arm64 libxkbcommon-dev:arm64 libssl-dev:arm64 \
 && rustup target add aarch64-unknown-linux-gnu \
 && chmod -R a+rX /opt

# Windows only needs a recent mingw, NSIS and Wine, so it uses a current Debian.
# The appindicator dev package is only there because the Tauri CLI, running on
# Linux, panics when it cannot detect the library, whatever the target.
FROM debian:13 AS windows-x64
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:/opt/node/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl xz-utils file pkg-config build-essential gcc-mingw-w64-x86-64-posix g++-mingw-w64-x86-64-posix binutils-mingw-w64-x86-64 nsis wine wine64 libayatana-appindicator3-dev \
 && curl -fsSL https://nodejs.org/dist/v24.22.1/node-v24.22.1-linux-x64.tar.xz | (mkdir -p /opt/node && tar -xJ --strip-components=1 -C /opt/node) \
 && curl -fsSL https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal --default-toolchain 1.98.1 --target x86_64-pc-windows-gnu \
 && chmod -R a+rX /opt
