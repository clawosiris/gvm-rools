#!/usr/bin/env bash
# Test this checkout against one resolved commit from canonical rust-gvm main.
# This intentionally rewrites manifests and Cargo.lock, so run it only in a
# disposable checkout (as the CI job does).

set -euo pipefail

readonly RUST_GVM_REPOSITORY="https://github.com/greenbone-hive/rust-gvm.git"
readonly RUST_GVM_MAIN_REF="refs/heads/main"
readonly PACKAGES=(gvm-connection gvm-protocol gvm-gmp gvm-mock-server)

usage() {
  echo "Usage: $0 [rust-gvm-main-sha]" >&2
}

if (( $# > 1 )); then
  usage
  exit 2
fi

repository_root="$(git rev-parse --show-toplevel)"
cd "${repository_root}"

rust_gvm_sha="${1:-}"
if [[ -z "${rust_gvm_sha}" ]]; then
  mapfile -t main_refs < <(git ls-remote "${RUST_GVM_REPOSITORY}" "${RUST_GVM_MAIN_REF}")
  if (( ${#main_refs[@]} != 1 )); then
    echo "Expected exactly one ${RUST_GVM_MAIN_REF} from ${RUST_GVM_REPOSITORY}" >&2
    exit 1
  fi
  read -r rust_gvm_sha resolved_ref extra <<< "${main_refs[0]}"
  if [[ "${resolved_ref}" != "${RUST_GVM_MAIN_REF}" || -n "${extra:-}" ]]; then
    echo "Unexpected ls-remote response for rust-gvm main" >&2
    exit 1
  fi
fi

if [[ ! "${rust_gvm_sha}" =~ ^[0-9a-f]{40}$ ]]; then
  echo "rust-gvm revision must be a lowercase 40-character SHA: ${rust_gvm_sha}" >&2
  exit 2
fi

for package in "${PACKAGES[@]}"; do
  manifest="Cargo.toml"
  if [[ "${package}" == "gvm-mock-server" ]]; then
    manifest="crates/gvm-rools-cli/Cargo.toml"
  fi

  declaration_prefix="${package} = { git = \"${RUST_GVM_REPOSITORY}\", rev = \""
  if [[ "$(grep -Fc "${declaration_prefix}" "${manifest}")" != "1" ]]; then
    echo "Expected exactly one canonical ${package} declaration in ${manifest}" >&2
    exit 1
  fi

  sed -E -i \
    "s|^(${package} = \{ git = \"${RUST_GVM_REPOSITORY}\", rev = )\"[0-9a-f]{40}\"|\1\"${rust_gvm_sha}\"|" \
    "${manifest}"
done

echo "Testing all rust-gvm crates at ${rust_gvm_sha}"
cargo generate-lockfile

expected_source="source = \"git+${RUST_GVM_REPOSITORY}?rev=${rust_gvm_sha}#${rust_gvm_sha}\""
for package in "${PACKAGES[@]}"; do
  source_line="$(awk -v package="${package}" '
    $0 == "[[package]]" { in_package = 0 }
    $0 == "name = \"" package "\"" { in_package = 1 }
    in_package && /^source = / { print; exit }
  ' Cargo.lock)"
  if [[ "${source_line}" != "${expected_source}" ]]; then
    echo "${package} did not resolve to ${rust_gvm_sha}: ${source_line}" >&2
    exit 1
  fi
done

cargo test --workspace --all-features --locked
