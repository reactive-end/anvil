#!/usr/bin/env bash
# Installs the `anvil` binary, either by building this checkout (default) or by
# downloading a published GitHub release (--from-release).
#
# Usage:
#   scripts/install.sh [--install-dir DIR] [--skip-build]
#   scripts/install.sh --from-release [--version vX.Y.Z] [--install-dir DIR]
#
# Environment:
#   ANVIL_INSTALL_DIR  default install directory
#   ANVIL_REPO         GitHub repository for --from-release (default: reactive-end/anvil)
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

ANVIL_REPO="${ANVIL_REPO:-reactive-end/anvil}"
TMP_DIR=""
SKIP_BUILD=0
FROM_RELEASE=0
VERSION=""

usage_error() {
  echo "error: $1" >&2
  exit 1
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --install-dir)
      [[ $# -ge 2 ]] || usage_error "--install-dir requires a value"
      INSTALL_DIR="$2"
      shift 2
      ;;
    --skip-build)
      SKIP_BUILD=1
      shift
      ;;
    --from-release)
      FROM_RELEASE=1
      shift
      ;;
    --version)
      [[ $# -ge 2 ]] || usage_error "--version requires a value such as v0.1.0"
      VERSION="$2"
      shift 2
      ;;
    *)
      echo "Unknown option: $1" >&2
      exit 1
      ;;
  esac
done

if [[ -n "${VERSION}" && "${FROM_RELEASE}" -eq 0 ]]; then
  usage_error "--version only applies together with --from-release"
fi

detect_target() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"
  case "${os}/${arch}" in
    Linux/x86_64) echo "x86_64-unknown-linux-musl" ;;
    Linux/aarch64 | Linux/arm64) echo "aarch64-unknown-linux-gnu" ;;
    Darwin/arm64) echo "aarch64-apple-darwin" ;;
    Darwin/x86_64) echo "x86_64-apple-darwin" ;;
    *) usage_error "unsupported platform ${os}/${arch}; build from source instead (run this script without --from-release)" ;;
  esac
}

latest_tag() {
  local tag location
  if command -v gh >/dev/null 2>&1; then
    tag="$(gh release view --repo "${ANVIL_REPO}" --json tagName -q .tagName)" ||
      usage_error "could not read the latest release of ${ANVIL_REPO} with gh (run 'gh auth login' for private repositories)"
  else
    location="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/${ANVIL_REPO}/releases/latest")" ||
      usage_error "could not read the latest release of ${ANVIL_REPO} (curl only works for public repositories; install gh and run 'gh auth login' for private ones)"
    [[ "${location}" == */releases/tag/* ]] || usage_error "no release found for ${ANVIL_REPO}"
    tag="${location##*/}"
  fi
  [[ -n "${tag}" ]] || usage_error "no release found for ${ANVIL_REPO}"
  echo "${tag}"
}

download_asset() {
  local tag="$1" asset="$2" dest_dir="$3"
  if command -v gh >/dev/null 2>&1; then
    gh release download "${tag}" --repo "${ANVIL_REPO}" --pattern "${asset}" --dir "${dest_dir}" --clobber ||
      usage_error "gh could not download ${asset} from ${ANVIL_REPO}@${tag} (run 'gh auth login' for private repositories)"
  else
    curl -fsSL -o "${dest_dir}/${asset}" "https://github.com/${ANVIL_REPO}/releases/download/${tag}/${asset}" ||
      usage_error "could not download ${asset} from ${ANVIL_REPO}@${tag} (curl only works for public repositories; install gh and run 'gh auth login' for private ones)"
  fi
}

verify_checksum() {
  local dir="$1" asset="$2"
  if command -v sha256sum >/dev/null 2>&1; then
    (cd "${dir}" && sha256sum -c "${asset}.sha256") || usage_error "checksum mismatch for ${asset}; aborting"
  elif command -v shasum >/dev/null 2>&1; then
    (cd "${dir}" && shasum -a 256 -c "${asset}.sha256") || usage_error "checksum mismatch for ${asset}; aborting"
  else
    usage_error "neither sha256sum nor shasum is available to verify the download"
  fi
}

install_from_release() {
  local target tag asset
  target="$(detect_target)"
  tag="${VERSION}"
  if [[ -z "${tag}" ]]; then
    tag="$(latest_tag)"
  fi
  asset="anvil-${tag}-${target}.tar.gz"

  TMP_DIR="$(mktemp -d)"
  trap 'rm -rf "${TMP_DIR}"' EXIT

  echo "Downloading ${asset} from ${ANVIL_REPO}@${tag}..."
  download_asset "${tag}" "${asset}" "${TMP_DIR}"
  download_asset "${tag}" "${asset}.sha256" "${TMP_DIR}"
  verify_checksum "${TMP_DIR}" "${asset}"

  mkdir -p "${TMP_DIR}/extract"
  tar -xzf "${TMP_DIR}/${asset}" -C "${TMP_DIR}/extract"
  [[ -f "${TMP_DIR}/extract/anvil" ]] || usage_error "archive ${asset} does not contain an anvil binary"

  mkdir -p "${INSTALL_DIR}"
  install -m 0755 "${TMP_DIR}/extract/anvil" "${INSTALL_DIR}/anvil"
}

install_from_source() {
  local src_bin="${REPO_ROOT}/target/release/anvil"

  if [[ "${SKIP_BUILD}" -eq 0 ]]; then
    if ! command -v cargo >/dev/null 2>&1; then
      echo "error: cargo not found on PATH. Install Rust via https://rustup.rs/" >&2
      exit 1
    fi
    echo "Building anvil release binary..."
    (cd "${REPO_ROOT}" && cargo build --release -p anvil-cli)
  fi

  if [[ ! -f "${src_bin}" ]]; then
    echo "error: release binary not found at ${src_bin}" >&2
    exit 1
  fi

  mkdir -p "${INSTALL_DIR}"
  install -m 0755 "${src_bin}" "${INSTALL_DIR}/anvil"
}

if [[ "${FROM_RELEASE}" -eq 1 ]]; then
  install_from_release
else
  install_from_source
fi
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
