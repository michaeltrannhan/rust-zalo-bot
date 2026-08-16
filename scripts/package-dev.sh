#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=packaging/lib/package.sh
source "${ROOT}/packaging/lib/package.sh"

load_package_version

HOST_ARCH="${ARCH:-$(uname -m)}"
DEB_ARCH="$(normalize_deb_arch "${HOST_ARCH}")"

BINARY="${ZL_EXPENSE_BINARY:-${ROOT}/target/release/zl-expense}"
MIGRATIONS_SRC="${ZL_EXPENSE_MIGRATIONS:-${ROOT}/migrations}"
CONFIG_EXAMPLE="${ZL_EXPENSE_CONFIG_EXAMPLE:-${ROOT}/config/config.example.toml}"
OUTPUT_DIR="${ZL_EXPENSE_OUTPUT:-${ROOT}/dist}"

STAGING_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/zl-expense-pkg.XXXXXX")"
cleanup() {
    rm -rf "${STAGING_ROOT}"
}
trap cleanup EXIT

require_path() {
    local label path
    label="${1}"
    path="${2}"
    if [ ! -e "${path}" ]; then
        printf 'missing %s: %s\n' "${label}" "${path}" >&2
        exit 1
    fi
}

stage_operator_docs() {
    local doc_dir="${1}"
    local deploy_dir="${2}"

    install -d "${doc_dir}" "${deploy_dir}/caddy" "${deploy_dir}/minio"

    if [ -f "${ROOT}/docs/operator-install.md" ]; then
        stage_file 0644 "${ROOT}/docs/operator-install.md" \
            "${doc_dir}/operator-install.md"
    fi
    if [ -f "${ROOT}/docs/operator-runbook.md" ]; then
        stage_file 0644 "${ROOT}/docs/operator-runbook.md" \
            "${doc_dir}/operator-runbook.md"
    fi
    if [ -f "${ROOT}/docs/privacy-policy-template.md" ]; then
        stage_file 0644 "${ROOT}/docs/privacy-policy-template.md" \
            "${doc_dir}/privacy-policy-template.md"
    fi
    if [ -f "${ROOT}/deploy/caddy/Caddyfile" ]; then
        stage_file 0644 "${ROOT}/deploy/caddy/Caddyfile" \
            "${deploy_dir}/caddy/Caddyfile"
    fi
    if [ -f "${ROOT}/deploy/minio/compose.minio.yaml" ]; then
        stage_file 0644 "${ROOT}/deploy/minio/compose.minio.yaml" \
            "${deploy_dir}/minio/compose.minio.yaml"
    fi
    if [ -f "${ROOT}/dist/sbom.cdx.json" ]; then
        stage_file 0644 "${ROOT}/dist/sbom.cdx.json" "${doc_dir}/sbom.cdx.json"
    fi
}

require_path "binary" "${BINARY}"
require_path "migrations directory" "${MIGRATIONS_SRC}"
require_path "config example" "${CONFIG_EXAMPLE}"

DEB_STAGING="${STAGING_ROOT}/deb"
DEBIAN_DIR="${DEB_STAGING}/DEBIAN"

install -d "${DEB_STAGING}/usr/bin" \
    "${DEB_STAGING}/usr/share/zl-expense/migrations" \
    "${DEB_STAGING}/usr/share/zl-expense" \
    "${DEB_STAGING}/usr/share/doc/zl-expense" \
    "${DEB_STAGING}/lib/systemd/system" \
    "${DEBIAN_DIR}"

stage_file 0755 "${BINARY}" "${DEB_STAGING}/usr/bin/zl-expense"
stage_tree "${MIGRATIONS_SRC}" "${DEB_STAGING}/usr/share/zl-expense/migrations" 0644 0755
stage_file 0644 "${CONFIG_EXAMPLE}" "${DEB_STAGING}/usr/share/zl-expense/config.example.toml"
stage_file 0644 "${ROOT}/deploy/systemd/zl-expense.service" \
    "${DEB_STAGING}/lib/systemd/system/zl-expense.service"

stage_operator_docs "${DEB_STAGING}/usr/share/doc/zl-expense" \
    "${DEB_STAGING}/usr/share/zl-expense/deploy"

substitute_control \
    "${ROOT}/packaging/debian/DEBIAN/control" \
    "${DEBIAN_DIR}/control" \
    "${PACKAGE_VERSION}" \
    "${DEB_ARCH}"

cp "${ROOT}/packaging/debian/DEBIAN/preinst" "${DEBIAN_DIR}/preinst"
cp "${ROOT}/packaging/debian/DEBIAN/postinst" "${DEBIAN_DIR}/postinst"
cp "${ROOT}/packaging/debian/DEBIAN/prerm" "${DEBIAN_DIR}/prerm"
cp "${ROOT}/packaging/debian/DEBIAN/postrm" "${DEBIAN_DIR}/postrm"
chmod 0755 "${DEBIAN_DIR}/preinst" "${DEBIAN_DIR}/postinst" \
    "${DEBIAN_DIR}/prerm" "${DEBIAN_DIR}/postrm"

install -d "${OUTPUT_DIR}"

DEB_FILE="${OUTPUT_DIR}/${PACKAGE_NAME}_${PACKAGE_VERSION}_${DEB_ARCH}.deb"
if ! command -v dpkg-deb >/dev/null 2>&1; then
    printf 'dpkg-deb is required to build the Debian package\n' >&2
    exit 1
fi

dpkg-deb --root-owner-group --build "${DEB_STAGING}" "${DEB_FILE}"

TARBALL_ROOT="${STAGING_ROOT}/tar/${PACKAGE_NAME}-${PACKAGE_VERSION}-${DEB_ARCH}"
install -d "${TARBALL_ROOT}/bin" \
    "${TARBALL_ROOT}/share/migrations" \
    "${TARBALL_ROOT}/share" \
    "${TARBALL_ROOT}/systemd" \
    "${TARBALL_ROOT}/doc"

stage_file 0755 "${BINARY}" "${TARBALL_ROOT}/bin/zl-expense"
stage_tree "${MIGRATIONS_SRC}" "${TARBALL_ROOT}/share/migrations" 0644 0755
stage_file 0644 "${CONFIG_EXAMPLE}" "${TARBALL_ROOT}/share/config.example.toml"
stage_file 0644 "${ROOT}/deploy/systemd/zl-expense.service" \
    "${TARBALL_ROOT}/systemd/zl-expense.service"

stage_operator_docs "${TARBALL_ROOT}/doc" "${TARBALL_ROOT}/deploy"

cat >"${TARBALL_ROOT}/INSTALL.txt" <<'EOF'
Portable zl-expense bundle (unsigned development build).

1. Copy bin/zl-expense to /usr/local/bin/ (or another root-owned path on PATH).
2. Copy share/migrations to /usr/share/zl-expense/migrations.
3. Copy share/config.example.toml to /etc/zl-expense/config.toml if missing.
4. Install systemd/zl-expense.service under /lib/systemd/system/ and follow
   doc/operator-install.md for user, directories, and service enablement.
EOF

TAR_FILE="${OUTPUT_DIR}/${PACKAGE_NAME}-${PACKAGE_VERSION}-${DEB_ARCH}.tar.gz"
tar -C "${STAGING_ROOT}/tar" -czf "${TAR_FILE}" \
    "${PACKAGE_NAME}-${PACKAGE_VERSION}-${DEB_ARCH}"

printf 'Debian package: %s\n' "${DEB_FILE}"
printf 'Portable bundle: %s\n' "${TAR_FILE}"
