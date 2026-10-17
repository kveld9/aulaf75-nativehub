#!/usr/bin/env bash
# Installer script for AULA F75 native Linux utilities (aula-f75-rs and aula-f75-gui).
# Builds release binaries, installs user-space components, and configures the udev rule.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Verify Rust toolchain is available
if ! command -v cargo >/dev/null 2>&1; then
    echo "Error: cargo is not installed or not found in PATH." >&2
    echo "Please install the Rust toolchain (https://rustup.rs) before running install.sh." >&2
    exit 1
fi

# Build release binaries
echo "Building release binaries (aula-f75-rs, aula-f75-gui)..."
cargo build --release --manifest-path "${SCRIPT_DIR}/rust/Cargo.toml"

# Install binaries to user local bin (~/.local/bin)
BIN_DIR="${HOME}/.local/bin"
echo "Installing binaries to ${BIN_DIR}..."
install -Dm755 "${SCRIPT_DIR}/rust/target/release/aula-f75-rs" "${BIN_DIR}/aula-f75-rs"
install -Dm755 "${SCRIPT_DIR}/rust/target/release/aula-f75-gui" "${BIN_DIR}/aula-f75-gui"

# Install udev rule to /etc/udev/rules.d (requires sudo)
UDEV_RULES_DIR="/etc/udev/rules.d"
echo "Installing udev rule to ${UDEV_RULES_DIR}/99-aula-f75.rules (requires sudo privileges)..."
sudo install -Dm644 "${SCRIPT_DIR}/99-aula-f75.rules" "${UDEV_RULES_DIR}/99-aula-f75.rules"

echo "Reloading udev rules..."
if command -v udevadm >/dev/null 2>&1; then
    sudo udevadm control --reload-rules
    sudo udevadm trigger
else
    echo "Warning: udevadm not found in PATH, skipping udev reload."
fi

# Install desktop entry to ~/.local/share/applications
APP_DIR="${XDG_DATA_HOME:-${HOME}/.local/share}/applications"
echo "Installing desktop entry to ${APP_DIR}..."
install -Dm644 "${SCRIPT_DIR}/aula-f75.desktop" "${APP_DIR}/aula-f75.desktop"
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "${APP_DIR}" >/dev/null 2>&1 || true
fi

# Install systemd user service to ~/.config/systemd/user
SYSTEMD_DIR="${XDG_CONFIG_HOME:-${HOME}/.config}/systemd/user"
echo "Installing systemd user service to ${SYSTEMD_DIR}..."
install -Dm644 "${SCRIPT_DIR}/aula-f75.service" "${SYSTEMD_DIR}/aula-f75.service"
if command -v systemctl >/dev/null 2>&1; then
    systemctl --user daemon-reload 2>/dev/null || true
fi

echo ""
echo "Installation completed successfully."
echo ""
echo "Installed binaries:"
echo "  - ${BIN_DIR}/aula-f75-rs   (CLI)"
echo "  - ${BIN_DIR}/aula-f75-gui  (Desktop GUI)"
echo ""
if [[ ":${PATH}:" != *":${BIN_DIR}:"* ]]; then
    echo "Notice: ${BIN_DIR} is not in your current PATH."
    echo "Add it by adding 'export PATH=\"\$HOME/.local/bin:\$PATH\"' to your ~/.bashrc or ~/.profile."
    echo ""
fi
echo "To enable and start the RGB lighting service on user login:"
echo "  systemctl --user enable --now aula-f75.service"
