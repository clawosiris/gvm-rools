#!/usr/bin/env bash
set -euo pipefail

VERSION="${1:-${VERSION:-}}"
TARGET="${TARGET:-x86_64-unknown-linux-musl}"
PACKAGE_NAME="${PACKAGE_NAME:-gvm-rools}"
BIN_NAME="${BIN_NAME:-gvm-cli}"
RELEASE_NUMBER="${RELEASE_NUMBER:-1}"
NFPM_ARCH="${NFPM_ARCH:-amd64}"
DIST_DIR="${DIST_DIR:-dist}"

if [[ -z "${VERSION}" ]]; then
  echo "usage: $0 <version>" >&2
  exit 1
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

TARGET_DIR="target/${TARGET}/release"
BINARY_PATH="${TARGET_DIR}/${BIN_NAME}"
TARBALL_BASENAME="${PACKAGE_NAME}-v${VERSION}-${TARGET}"
TARBALL_PATH="${DIST_DIR}/${TARBALL_BASENAME}.tar.gz"
PKGROOT_DIR="${DIST_DIR}/pkgroot"

rm -rf "${DIST_DIR}"
mkdir -p "${DIST_DIR}" "${PKGROOT_DIR}/usr/bin"

cargo build --locked --release --package gvm-rools-cli --bin "${BIN_NAME}" --target "${TARGET}"

install -Dm755 "${BINARY_PATH}" "${PKGROOT_DIR}/usr/bin/${BIN_NAME}"

TARBALL_STAGE="$(mktemp -d)"
trap 'rm -rf "${TARBALL_STAGE}"' EXIT
install -Dm755 "${BINARY_PATH}" "${TARBALL_STAGE}/${BIN_NAME}"
install -Dm644 README.md "${TARBALL_STAGE}/README.md"
install -Dm644 LICENSE "${TARBALL_STAGE}/LICENSE"
tar -C "${TARBALL_STAGE}" -czf "${TARBALL_PATH}" .

export VERSION RELEASE_NUMBER NFPM_ARCH

nfpm package \
  --config packaging/nfpm.yaml \
  --packager deb \
  --target "${DIST_DIR}/${PACKAGE_NAME}_${VERSION}_amd64.deb"

nfpm package \
  --config packaging/nfpm.yaml \
  --packager rpm \
  --target "${DIST_DIR}/${PACKAGE_NAME}-${VERSION}-1.x86_64.rpm"

nfpm package \
  --config packaging/nfpm.yaml \
  --packager archlinux \
  --target "${DIST_DIR}/${PACKAGE_NAME}-${VERSION}-1-x86_64.pkg.tar.zst"

(
  cd "${DIST_DIR}"
  sha256sum \
    "$(basename "${TARBALL_PATH}")" \
    "${PACKAGE_NAME}_${VERSION}_amd64.deb" \
    "${PACKAGE_NAME}-${VERSION}-1.x86_64.rpm" \
    "${PACKAGE_NAME}-${VERSION}-1-x86_64.pkg.tar.zst" \
    > SHA256SUMS
)
