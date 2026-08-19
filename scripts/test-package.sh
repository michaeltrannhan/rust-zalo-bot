#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=packaging/lib/package.sh
source "${ROOT}/packaging/lib/package.sh"

load_package_version

HOST_ARCH="${ARCH:-$(uname -m)}"
DEB_ARCH="$(normalize_deb_arch "${HOST_ARCH}")"

FIXTURE_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/zl-expense-fixture.XXXXXX")"
OUTPUT_DIR="${FIXTURE_ROOT}/dist"
cleanup() {
    rm -rf "${FIXTURE_ROOT}"
}
trap cleanup EXIT

log() {
    printf '[test-package] %s\n' "$*"
}

fail() {
    printf '[test-package] ERROR: %s\n' "$*" >&2
    exit 1
}

require_command() {
    local cmd="${1}"
    if ! command -v "${cmd}" >/dev/null 2>&1; then
        fail "required command not found: ${cmd}"
    fi
}

have_local_dpkg_deb() {
    command -v dpkg-deb >/dev/null 2>&1
}

have_docker() {
    command -v docker >/dev/null 2>&1
}

create_fixtures() {
    local binary_dir migrations_dir config_dir
    binary_dir="${FIXTURE_ROOT}/build"
    migrations_dir="${FIXTURE_ROOT}/migrations"
    config_dir="${FIXTURE_ROOT}/config"

    install -d "${binary_dir}" "${migrations_dir}" "${config_dir}"

    cat >"${binary_dir}/zl-expense" <<'EOF'
#!/bin/sh
printf 'zl-expense stub for packaging tests\n'
exit 0
EOF
    chmod 0755 "${binary_dir}/zl-expense"

    cat >"${migrations_dir}/0001_init.sql" <<'EOF'
-- packaging test migration placeholder
SELECT 1;
EOF

    cat >"${config_dir}/config.example.toml" <<'EOF'
# Example configuration for packaging tests
listen_address = "127.0.0.1:8080"
database_url = "postgres://zl_expense:zl_expense_dev@127.0.0.1:5432/zl_expense"
EOF
}

run_shellcheck() {
    log "running shellcheck"
    if ! command -v shellcheck >/dev/null 2>&1; then
        log "shellcheck not installed; skipping"
        return 0
    fi

    shellcheck -x \
        "${ROOT}/packaging/lib/package.sh" \
        "${ROOT}/scripts/package-dev.sh" \
        "${ROOT}/scripts/test-package.sh" \
        "${ROOT}/scripts/generate-sbom.sh" \
        "${ROOT}/scripts/security-audit.sh" \
        "${ROOT}/scripts/lib/slot-deploy.sh" \
        "${ROOT}/scripts/host-slot-deploy.sh" \
        "${ROOT}/scripts/remote-deploy.sh" \
        "${ROOT}/scripts/test-slot-deploy.sh"
}

build_packages() {
    log "building development packages from fixtures"

    if have_local_dpkg_deb; then
        ZL_EXPENSE_BINARY="${FIXTURE_ROOT}/build/zl-expense" \
            ZL_EXPENSE_MIGRATIONS="${FIXTURE_ROOT}/migrations" \
            ZL_EXPENSE_CONFIG_EXAMPLE="${FIXTURE_ROOT}/config/config.example.toml" \
            ZL_EXPENSE_OUTPUT="${OUTPUT_DIR}" \
            "${ROOT}/scripts/package-dev.sh"
        return 0
    fi

    if ! have_docker; then
        fail "dpkg-deb missing and docker unavailable; cannot build packages"
    fi

    log "dpkg-deb not found locally; building inside debian:bookworm"
    docker run --rm \
        -v "${ROOT}:/work:ro" \
        -v "${FIXTURE_ROOT}:/fixture" \
        debian:bookworm \
        bash -euo pipefail -c '
            apt-get update -qq
            apt-get install -y -qq dpkg-dev >/dev/null
            ZL_EXPENSE_BINARY=/fixture/build/zl-expense \
                ZL_EXPENSE_MIGRATIONS=/fixture/migrations \
                ZL_EXPENSE_CONFIG_EXAMPLE=/fixture/config/config.example.toml \
                ZL_EXPENSE_OUTPUT=/fixture/dist \
                /work/scripts/package-dev.sh
        '
}

validate_archive_contents() {
    local deb_file tar_file
    deb_file="${OUTPUT_DIR}/${PACKAGE_NAME}_${PACKAGE_VERSION}_${DEB_ARCH}.deb"
    tar_file="${OUTPUT_DIR}/${PACKAGE_NAME}-${PACKAGE_VERSION}-${DEB_ARCH}.tar.gz"

    [ -f "${deb_file}" ] || fail "missing deb: ${deb_file}"
    [ -f "${tar_file}" ] || fail "missing tarball: ${tar_file}"

    if have_local_dpkg_deb; then
        log "validating deb contents"
        dpkg-deb -c "${deb_file}" | grep -q './usr/bin/zl-expense$' || fail 'deb missing binary'
        dpkg-deb -c "${deb_file}" | grep -q './usr/share/zl-expense/migrations/0001_init.sql' \
            || fail 'deb missing migration'
        dpkg-deb -c "${deb_file}" | grep -q './lib/systemd/system/zl-expense.service' \
            || fail 'deb missing systemd unit'
        dpkg-deb -c "${deb_file}" | grep -q './lib/systemd/system/zl-expense@.service' \
            || fail 'deb missing slot systemd unit'
        dpkg-deb -c "${deb_file}" | grep -q './usr/share/zl-expense/deploy/caddy/origin.caddy' \
            || fail 'deb missing Caddy origin profile'
        dpkg-deb -c "${deb_file}" | grep -q './usr/share/zl-expense/deploy/host-slot-deploy.sh' \
            || fail 'deb missing host slot deploy script'
        dpkg-deb -c "${deb_file}" | grep -q './usr/share/zl-expense/deploy/caddy/Caddyfile' \
            || fail 'deb missing Caddy profile'
        dpkg-deb -c "${deb_file}" | grep -q './usr/share/doc/zl-expense/operator-runbook.md' \
            || fail 'deb missing operator runbook'

        log "validating deb file modes"
        dpkg-deb -c "${deb_file}" | grep './usr/bin/zl-expense' | grep -q 'rwxr-xr-x' \
            || fail 'binary mode not 755 in deb'
        dpkg-deb -c "${deb_file}" | grep './usr/share/zl-expense/migrations/0001_init.sql' \
            | grep -q 'rw-r--r--' || fail 'migration mode not 644 in deb'
    elif have_docker; then
        log "validating deb contents inside debian:bookworm"
        docker run --rm \
            -v "${deb_file}:/pkg.deb:ro" \
            debian:bookworm \
            bash -euo pipefail -c '
                apt-get update -qq
                apt-get install -y -qq dpkg-dev >/dev/null
                dpkg-deb -c /pkg.deb > /tmp/deb.list
                grep -q "./usr/bin/zl-expense$" /tmp/deb.list
                grep -q "./usr/share/zl-expense/migrations/0001_init.sql" /tmp/deb.list
                grep -q "./lib/systemd/system/zl-expense.service" /tmp/deb.list
                grep -q "./lib/systemd/system/zl-expense@.service" /tmp/deb.list
                grep -q "./usr/share/zl-expense/deploy/caddy/origin.caddy" /tmp/deb.list
                grep "./usr/bin/zl-expense" /tmp/deb.list | grep -q "rwxr-xr-x"
                grep "./usr/share/zl-expense/migrations/0001_init.sql" /tmp/deb.list | grep -q "rw-r--r--"
            '
    else
        fail "cannot validate deb without dpkg-deb or docker"
    fi

    log "validating tarball contents"
    tar -tzf "${tar_file}" | grep -q "${PACKAGE_NAME}-${PACKAGE_VERSION}-${DEB_ARCH}/bin/zl-expense" \
        || fail 'tarball missing binary'
    tar -tzf "${tar_file}" | grep -q \
        "${PACKAGE_NAME}-${PACKAGE_VERSION}-${DEB_ARCH}/share/migrations/0001_init.sql" \
        || fail 'tarball missing migration'
    tar -tzf "${tar_file}" | grep -q \
        "${PACKAGE_NAME}-${PACKAGE_VERSION}-${DEB_ARCH}/systemd/zl-expense.service" \
        || fail 'tarball missing systemd unit'
    tar -tzf "${tar_file}" | grep -q \
        "${PACKAGE_NAME}-${PACKAGE_VERSION}-${DEB_ARCH}/systemd/zl-expense@.service" \
        || fail 'tarball missing slot systemd unit'
    tar -tzf "${tar_file}" | grep -q \
        "${PACKAGE_NAME}-${PACKAGE_VERSION}-${DEB_ARCH}/deploy/caddy/Caddyfile" \
        || fail 'tarball missing Caddy profile'
    tar -tzf "${tar_file}" | grep -q \
        "${PACKAGE_NAME}-${PACKAGE_VERSION}-${DEB_ARCH}/doc/operator-runbook.md" \
        || fail 'tarball missing operator runbook'
}

validate_systemd_unit() {
    local unit_path
    unit_path="${ROOT}/deploy/systemd/zl-expense.service"

    log "validating systemd unit syntax"
    if command -v systemd-analyze >/dev/null 2>&1; then
        systemd-analyze verify "${unit_path}"
        systemd-analyze verify "${ROOT}/deploy/systemd/zl-expense@.service" || true
    fi
    grep -q '^Type=notify' "${unit_path}" || fail 'expected Type=notify'
    grep -q '^NotifyAccess=main' "${unit_path}" || fail 'missing NotifyAccess=main'
    grep -q '^WatchdogSec=30s' "${unit_path}" || fail 'missing WatchdogSec=30s'
    grep -q '^User=zl-expense' "${unit_path}" || fail 'missing User=zl-expense'
    grep -q '^TimeoutStopSec=30s' "${unit_path}" || fail 'missing TimeoutStopSec=30s'
    grep -q '^NoNewPrivileges=true' "${unit_path}" || fail 'missing NoNewPrivileges'
    grep -q '^MemoryMax=384M' "${unit_path}" || fail 'missing MemoryMax=384M'
    grep -q '^TasksMax=256' "${unit_path}" || fail 'missing TasksMax=256'
    grep -q 'RuntimeDirectory=zl-expense-%i' "${ROOT}/deploy/systemd/zl-expense@.service" \
        || fail 'slot unit missing per-instance RuntimeDirectory'
}

run_docker_install_test() {
    local deb_file docker_image
    deb_file="${OUTPUT_DIR}/${PACKAGE_NAME}_${PACKAGE_VERSION}_${DEB_ARCH}.deb"

    if ! command -v docker >/dev/null 2>&1; then
        log "docker not available; skipping install test"
        return 0
    fi

    docker_image="debian:bookworm"
    log "running Debian 12 install/uninstall test in Docker"

    docker run --rm \
        -v "${deb_file}:/pkg.deb:ro" \
        "${docker_image}" \
        bash -euo pipefail -c '
            apt-get update -qq
            apt-get install -y -qq dpkg >/dev/null
            dpkg -i /pkg.deb
            test -x /usr/bin/zl-expense
            test -f /etc/zl-expense/config.toml
            test -d /etc/zl-expense/credentials
            install -d -m 0750 /var/lib/zl-expense/markers
            touch /var/lib/zl-expense/markers/preserve-me
            touch /etc/zl-expense/credentials/test.secret
            printf "custom\n" > /etc/zl-expense/config.toml
            dpkg -r zl-expense
            test -f /etc/zl-expense/config.toml
            test -f /etc/zl-expense/credentials/test.secret
            test -f /var/lib/zl-expense/markers/preserve-me
            dpkg --purge zl-expense
            test ! -e /etc/zl-expense
            test ! -e /var/lib/zl-expense
        '
}

main() {
    create_fixtures
    run_shellcheck
    build_packages
    validate_archive_contents
    validate_systemd_unit
    run_docker_install_test
    "${ROOT}/scripts/test-slot-deploy.sh"
    log "all package tests passed"
}

main "$@"
