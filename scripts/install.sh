#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

if [[ -n "${ANVIL_INSTALL_DIR:-}" ]]; then
  INSTALL_DIR="${ANVIL_INSTALL_DIR}"
elif [[ -d "${HOME}/.cargo/bin" ]]; then
  INSTALL_DIR="${HOME}/.cargo/bin"
else
  INSTALL_DIR="${HOME}/.local/bin"
fi

SKIP_BUILD=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --install-dir)
      INSTALL_DIR="$2"
      shift 2
      ;;
    --skip-build)
      SKIP_BUILD=1
      shift
      ;;
    *)
      echo "Unknown option: $1" >&2
      exit 1
      ;;
  esac
done

if [[ "${SKIP_BUILD}" -eq 0 ]]; then
  if ! command -v cargo >/dev/null 2>&1; then
    echo "error: cargo not found on PATH. Install Rust via https://rustup.rs/" >&2
    exit 1
  fi
  echo "Building anvil release binary..."
  (cd "${REPO_ROOT}" && cargo build --release -p anvil-cli)
fi

SRC_BIN="${REPO_ROOT}/target/release/anvil"
if [[ ! -f "${SRC_BIN}" ]]; then
  echo "error: release binary not found at ${SRC_BIN}" >&2
  exit 1
fi

mkdir -p "${INSTALL_DIR}"
install -m 0755 "${SRC_BIN}" "${INSTALL_DIR}/anvil"
echo "Installed anvil to ${INSTALL_DIR}/anvil"

case ":${PATH}:" in
  *":${INSTALL_DIR}:"*)
    echo "Verified ${INSTALL_DIR} is on PATH."
    ;;
  *)
    echo "Warning: ${INSTALL_DIR} is not on PATH. Add this to your shell profile:" >&2
    echo "  export PATH=\"${INSTALL_DIR}:\$PATH\"" >&2
    ;;
esac

"${INSTALL_DIR}/anvil" --version
