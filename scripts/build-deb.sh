#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

RAW_VERSION="${FERRITE_VERSION:-$(awk -F'"' '/^version[[:space:]]*=/ { print $2; exit }' "${ROOT_DIR}/crates/app/Cargo.toml")}"
VERSION="${RAW_VERSION#v}"

if [[ -z "${VERSION}" ]]; then
    echo "Error: unable to determine package version." >&2
    exit 1
fi

BIN_PATH="${ROOT_DIR}/target/release/ferrite"
if [[ ! -x "${BIN_PATH}" ]]; then
    cargo build --release -p ferrite --manifest-path "${ROOT_DIR}/Cargo.toml"
fi

STAGE_DIR="$(mktemp -d)"
chmod 0755 "${STAGE_DIR}"
cleanup() {
    rm -rf "${STAGE_DIR}"
}
trap cleanup EXIT

install -Dm755 "${BIN_PATH}" "${STAGE_DIR}/usr/bin/ferrite"
install -Dm644 "${ROOT_DIR}/packaging/linux/ferrite.desktop" "${STAGE_DIR}/usr/share/applications/ferrite.desktop"

for sz in 32 48 64 128 256 512; do
    install -Dm644 \
        "${ROOT_DIR}/assets/icons/ferrite-${sz}.png" \
        "${STAGE_DIR}/usr/share/icons/hicolor/${sz}x${sz}/apps/ferrite.png"
done

install -Dm644 \
    "${ROOT_DIR}/assets/icons/ferrite.svg" \
    "${STAGE_DIR}/usr/share/icons/hicolor/scalable/apps/ferrite.svg"

if [[ -f "${ROOT_DIR}/LICENSE" ]]; then
    install -Dm644 "${ROOT_DIR}/LICENSE" "${STAGE_DIR}/usr/share/doc/ferrite/copyright"
fi
if [[ -f "${ROOT_DIR}/THIRD_PARTY_NOTICES.md" ]]; then
    install -Dm644 "${ROOT_DIR}/THIRD_PARTY_NOTICES.md" "${STAGE_DIR}/usr/share/doc/ferrite/THIRD_PARTY_NOTICES.md"
fi

INSTALLED_KB="$(du -ks "${STAGE_DIR}/usr" | awk '{print $1}')"
mkdir -p "${STAGE_DIR}/DEBIAN"
find "${STAGE_DIR}" -type d -exec chmod 0755 {} +

cat > "${STAGE_DIR}/DEBIAN/control" <<EOF
Package: ferrite
Version: ${VERSION}
Section: video
Priority: optional
Architecture: amd64
Installed-Size: ${INSTALLED_KB}
Depends: libc6, libmpv2 | libmpv1, libwayland-client0, libvulkan1, libegl1, libxcb1, libfreetype6, libfontconfig1, libxkbcommon0, libxkbcommon-x11-0, ca-certificates
Maintainer: Gaalix <https://github.com/Gaalix/ferrite>
Homepage: https://github.com/Gaalix/ferrite
Description: High-framerate GPU-accelerated desktop cinema client
 Ferrite is a native desktop media player built on GPUI, librqbit, and libmpv
 with Stremio addon compatibility, hardware-decoded playback, and low memory
 overhead.
EOF
chmod 644 "${STAGE_DIR}/DEBIAN/control"

DIST_DIR="${ROOT_DIR}/dist"
mkdir -p "${DIST_DIR}"
OUT_DEB="${DIST_DIR}/ferrite_${VERSION}_amd64.deb"

dpkg-deb --root-owner-group --build "${STAGE_DIR}" "${OUT_DEB}"
echo "Built ${OUT_DEB}"
