#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

if [[ -x "${ROOT_DIR}/target/release/ferrite" ]]; then
    BIN_SRC="${ROOT_DIR}/target/release/ferrite"
elif [[ -x "${ROOT_DIR}/ferrite" ]]; then
    BIN_SRC="${ROOT_DIR}/ferrite"
else
    echo "Error: ferrite binary not found. Run 'cargo build --release -p ferrite' first." >&2
    exit 1
fi

BIN_DIR="${HOME}/.local/bin"
APPS_DIR="${HOME}/.local/share/applications"
ICONS_DIR="${HOME}/.local/share/icons/hicolor"

install -Dm755 "${BIN_SRC}" "${BIN_DIR}/ferrite"

for sz in 32 48 64 128 256 512; do
    ICON_FILE="${ROOT_DIR}/assets/icons/ferrite-${sz}.png"
    if [[ -f "${ICON_FILE}" ]]; then
        install -Dm644 "${ICON_FILE}" "${ICONS_DIR}/${sz}x${sz}/apps/ferrite.png"
    fi
done

if [[ -f "${ROOT_DIR}/assets/icons/ferrite.svg" ]]; then
    install -Dm644 "${ROOT_DIR}/assets/icons/ferrite.svg" "${ICONS_DIR}/scalable/apps/ferrite.svg"
fi

mkdir -p "${APPS_DIR}"
sed \
    -e "s|^Exec=ferrite %u$|Exec=${BIN_DIR}/ferrite %u|" \
    -e "s|^TryExec=ferrite$|TryExec=${BIN_DIR}/ferrite|" \
    "${ROOT_DIR}/packaging/linux/ferrite.desktop" > "${APPS_DIR}/ferrite.desktop"
chmod 644 "${APPS_DIR}/ferrite.desktop"

if command -v xdg-mime >/dev/null 2>&1; then
    xdg-mime default ferrite.desktop x-scheme-handler/stremio || true
fi

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "${APPS_DIR}" >/dev/null 2>&1 || true
fi

if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t "${ICONS_DIR}" >/dev/null 2>&1 || true
fi

echo "Installed Ferrite to ${BIN_DIR}/ferrite"
