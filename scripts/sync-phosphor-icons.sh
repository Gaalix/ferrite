#!/usr/bin/env bash
set -euo pipefail

# Sync verbatim MIT-licensed SVG icons from the official @phosphor-icons/core
# npm package into assets/icons/phosphor/{regular,fill}/.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
OUT_DIR="${ROOT_DIR}/assets/icons/phosphor"
PHOSPHOR_VERSION="${PHOSPHOR_VERSION:-2.1.1}"

ICONS=(
    arrow-clockwise
    arrow-counter-clockwise
    arrow-down
    arrow-left
    arrow-line-up
    arrow-right
    arrow-square-out
    arrow-up
    arrows-clockwise
    bookmark-simple
    calendar-blank
    caret-down
    caret-left
    caret-right
    caret-up
    check
    clipboard
    clock
    clock-clockwise
    clock-counter-clockwise
    closed-captioning
    compass
    copy
    corners-in
    corners-out
    cpu
    dots-three
    eye
    eye-slash
    file-text
    film-slate
    film-strip
    fire
    folder-open
    funnel
    gear-six
    grid-four
    hard-drive
    house
    info
    lightning
    list
    magnifying-glass
    minus
    monitor
    moon
    music-note
    palette
    pause
    play
    plus
    push-pin
    rows
    shield-check
    shuffle
    sidebar-simple
    skip-back
    skip-forward
    sliders-horizontal
    sparkle
    speaker-high
    speaker-slash
    square
    stack
    star
    subtitles
    sun
    television
    trash
    user
    users
    x
)

TMP_DIR="$(mktemp -d)"
cleanup() {
    rm -rf "${TMP_DIR}"
}
trap cleanup EXIT

TARBALL=""
if command -v npm >/dev/null 2>&1; then
    TARBALL_NAME="$(
        cd "${TMP_DIR}" && npm pack "@phosphor-icons/core@${PHOSPHOR_VERSION}" --silent
    )"
    TARBALL="${TMP_DIR}/${TARBALL_NAME}"
elif command -v curl >/dev/null 2>&1; then
    TARBALL="${TMP_DIR}/phosphor-icons-core-${PHOSPHOR_VERSION}.tgz"
    curl -fsSL \
        "https://registry.npmjs.org/@phosphor-icons/core/-/core-${PHOSPHOR_VERSION}.tgz" \
        -o "${TARBALL}"
else
    echo "Error: neither 'npm' nor 'curl' is available to fetch @phosphor-icons/core." >&2
    exit 1
fi

if [[ ! -f "${TARBALL}" ]]; then
    echo "Error: failed to download @phosphor-icons/core@${PHOSPHOR_VERSION} tarball." >&2
    exit 1
fi

tar -xzf "${TARBALL}" -C "${TMP_DIR}" \
    package/LICENSE \
    package/assets/regular \
    package/assets/fill

SRC_REGULAR="${TMP_DIR}/package/assets/regular"
SRC_FILL="${TMP_DIR}/package/assets/fill"

for name in "${ICONS[@]}"; do
    if [[ ! -f "${SRC_REGULAR}/${name}.svg" ]]; then
        echo "Error: missing regular SVG for '${name}' in @phosphor-icons/core@${PHOSPHOR_VERSION}." >&2
        exit 1
    fi
    if [[ ! -f "${SRC_FILL}/${name}-fill.svg" ]]; then
        echo "Error: missing fill SVG for '${name}' in @phosphor-icons/core@${PHOSPHOR_VERSION}." >&2
        exit 1
    fi
done

STAGE_DIR="${TMP_DIR}/staged"
mkdir -p "${STAGE_DIR}/regular" "${STAGE_DIR}/fill"
cp -f "${TMP_DIR}/package/LICENSE" "${STAGE_DIR}/LICENSE"

for name in "${ICONS[@]}"; do
    cp -f "${SRC_REGULAR}/${name}.svg" "${STAGE_DIR}/regular/${name}.svg"
    cp -f "${SRC_FILL}/${name}-fill.svg" "${STAGE_DIR}/fill/${name}-fill.svg"
done

rm -rf "${OUT_DIR}/regular" "${OUT_DIR}/fill"
mkdir -p "${OUT_DIR}/regular" "${OUT_DIR}/fill"
cp -f "${STAGE_DIR}/LICENSE" "${OUT_DIR}/LICENSE"
cp -f "${STAGE_DIR}/regular/"*.svg "${OUT_DIR}/regular/"
cp -f "${STAGE_DIR}/fill/"*.svg "${OUT_DIR}/fill/"

echo "Synced ${#ICONS[@]} Phosphor icons (${#ICONS[@]} regular + ${#ICONS[@]} fill) from @phosphor-icons/core@${PHOSPHOR_VERSION} into ${OUT_DIR}."
