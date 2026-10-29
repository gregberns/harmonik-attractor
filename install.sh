#!/usr/bin/env bash
set -euo pipefail

INSTALL_DIR="${HOME}/.local/bin"
BIN_NAMES=("harmonik-attractor" "hka")

echo "Building harmonik-attractor (release)..."
cargo build -p attractor-cli --release --features monitor

# Create install directory if needed
mkdir -p "${INSTALL_DIR}"

# Copy binaries
for BIN_NAME in "${BIN_NAMES[@]}"; do
    cp "target/release/${BIN_NAME}" "${INSTALL_DIR}/${BIN_NAME}"
done

# Sign with Apple Developer identity (prevents macOS SIGKILL on unsigned binaries)
if [[ -z "${CODESIGN_IDENTITY:-}" ]]; then
    echo "ERROR: CODESIGN_IDENTITY is not set."
    echo "Set it in your .envrc (see .envrc.example) or export it before running install.sh"
    exit 1
fi
if security find-identity -v -p codesigning 2>/dev/null | grep -q "${CODESIGN_IDENTITY}"; then
    for BIN_NAME in "${BIN_NAMES[@]}"; do
        codesign --force --sign "${CODESIGN_IDENTITY}" "${INSTALL_DIR}/${BIN_NAME}"
        echo "Signed ${BIN_NAME} with ${CODESIGN_IDENTITY}"
    done
else
    echo "WARNING: Signing identity not found, skipping codesign"
fi

for BIN_NAME in "${BIN_NAMES[@]}"; do
    echo "Installed ${BIN_NAME} to ${INSTALL_DIR}/${BIN_NAME}"
done

# Check if install dir is in PATH
if ! echo "${PATH}" | tr ':' '\n' | grep -qx "${INSTALL_DIR}"; then
    echo ""
    echo "WARNING: ${INSTALL_DIR} is not in your PATH."
    echo "Add it to your shell config:"
    echo ""
    echo "  # bash (~/.bashrc)"
    echo "  export PATH=\"${INSTALL_DIR}:\${PATH}\""
    echo ""
    echo "  # zsh (~/.zshrc)"
    echo "  export PATH=\"${INSTALL_DIR}:\${PATH}\""
    echo ""
    echo "  # fish (~/.config/fish/config.fish)"
    echo "  fish_add_path ${INSTALL_DIR}"
fi

echo ""
"${INSTALL_DIR}/hka" --version
