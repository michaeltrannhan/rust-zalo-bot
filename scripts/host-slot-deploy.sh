#!/usr/bin/env bash
# Zero-downtime blue/green cutover for a single Oracle VM.
# Intended to run on the host as a sudo-capable operator (ubuntu).
set -euo pipefail

ROOT=""
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

source_slot_lib() {
    local candidate
    for candidate in \
        "${SCRIPT_DIR}/lib/slot-deploy.sh" \
        "${SCRIPT_DIR}/slot-deploy.sh" \
        "${SCRIPT_DIR}/../scripts/lib/slot-deploy.sh"; do
        if [ -f "${candidate}" ]; then
            # shellcheck source=scripts/lib/slot-deploy.sh
            source "${candidate}"
            return 0
        fi
    done
    printf 'missing slot-deploy.sh helpers\n' >&2
    return 1
}

resolve_file_root() {
    local candidate
    for candidate in \
        "${SCRIPT_DIR}/.." \
        "${SCRIPT_DIR}" \
        /usr/share/zl-expense; do
        if [ -f "${candidate}/deploy/systemd/zl-expense@.service" ] \
            || [ -f "${candidate}/deploy/caddy/origin.caddy" ]; then
            ROOT="$(cd "${candidate}" && pwd)"
            return 0
        fi
    done
    ROOT=""
}

source_slot_lib
resolve_file_root

usage() {
    cat <<'EOF'
Usage: host-slot-deploy.sh [--deb PATH] [--skip-migrate] [--skip-backup]

Installs the Debian package without stopping the live process, migrates the
database, starts the inactive slot, waits for /health/ready, reloads the
loopback Caddy origin, then SIGTERM-drains the previous slot.

First run bootstraps Caddy on 127.0.0.1:8080 so Cloudflare Tunnel can keep
using that address. Traffic is moved to the new slot before the old listener
is released.
EOF
}

DEB_PATH=""
SKIP_MIGRATE=0
SKIP_BACKUP=0
CONFIG="/etc/zl-expense/config.toml"
ACTIVE_SLOT_FILE="/var/lib/zl-expense/deploy/active-slot"
UPSTREAM_FILE="/etc/zl-expense/caddy/upstream.caddy"
ORIGIN_FILE="/etc/zl-expense/caddy/origin.caddy"
CADDYFILE="/etc/caddy/Caddyfile"
CLOUDFLARED_CONFIG="${CLOUDFLARED_CONFIG:-/home/ubuntu/.cloudflared/config.yml}"
CLOUDFLARED_UNIT="${CLOUDFLARED_UNIT:-cloudflared-zl-expense.service}"
BACKUP_DIR="/var/backups/zl-expense"
HEALTH_TIMEOUT_SEC="${HEALTH_TIMEOUT_SEC:-60}"
ORIGIN_PORT="8080"

while [ "$#" -gt 0 ]; do
    case "${1}" in
    --deb)
        DEB_PATH="${2}"
        shift 2
        ;;
    --skip-migrate)
        SKIP_MIGRATE=1
        shift
        ;;
    --skip-backup)
        SKIP_BACKUP=1
        shift
        ;;
    -h | --help)
        usage
        exit 0
        ;;
    *)
        printf 'unknown argument: %s\n' "${1}" >&2
        usage >&2
        exit 2
        ;;
    esac
done

log() {
    printf '[slot-deploy] %s\n' "$*"
}

die() {
    printf '[slot-deploy] ERROR: %s\n' "$*" >&2
    exit 1
}

require_cmd() {
    command -v "${1}" >/dev/null 2>&1 || die "missing command: ${1}"
}

wait_ready() {
    local url="${1}"
    local deadline=$((SECONDS + HEALTH_TIMEOUT_SEC))
    while [ "${SECONDS}" -lt "${deadline}" ]; do
        if curl -fsS "${url}" >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
    done
    die "timed out waiting for ${url}"
}

read_active_slot() {
    if [ -f "${ACTIVE_SLOT_FILE}" ]; then
        tr -d '[:space:]' <"${ACTIVE_SLOT_FILE}"
        return 0
    fi
    printf ''
}

write_active_slot() {
    install -d -m 0750 -o zl-expense -g zl-expense "$(dirname "${ACTIVE_SLOT_FILE}")"
    printf '%s\n' "${1}" >"${ACTIVE_SLOT_FILE}"
    chmod 0644 "${ACTIVE_SLOT_FILE}"
}

install_slot_files() {
    install -d -m 0755 /etc/zl-expense/slots
    install -d -m 0755 /etc/zl-expense/caddy
    if [ ! -f /etc/zl-expense/slots/blue.env ]; then
        printf 'ZL_EXPENSE_LISTEN_ADDRESS=127.0.0.1:%s\n' "${SLOT_BLUE_PORT}" \
            >/etc/zl-expense/slots/blue.env
    fi
    if [ ! -f /etc/zl-expense/slots/green.env ]; then
        printf 'ZL_EXPENSE_LISTEN_ADDRESS=127.0.0.1:%s\n' "${SLOT_GREEN_PORT}" \
            >/etc/zl-expense/slots/green.env
    fi
    chmod 0644 /etc/zl-expense/slots/blue.env /etc/zl-expense/slots/green.env
    local origin_src unit_src
    origin_src="${ROOT:+${ROOT}/deploy/caddy/origin.caddy}"
    if [ -z "${origin_src}" ] || [ ! -f "${origin_src}" ]; then
        origin_src="${SCRIPT_DIR}/../deploy/caddy/origin.caddy"
    fi
    if [ -f "${origin_src}" ]; then
        install -m 0644 "${origin_src}" "${ORIGIN_FILE}"
    elif [ ! -f "${ORIGIN_FILE}" ]; then
        die "missing ${ORIGIN_FILE} and repository origin.caddy"
    fi
    unit_src="${ROOT:+${ROOT}/deploy/systemd/zl-expense@.service}"
    if [ -z "${unit_src}" ] || [ ! -f "${unit_src}" ]; then
        unit_src="${SCRIPT_DIR}/../deploy/systemd/zl-expense@.service"
    fi
    if [ -f "${unit_src}" ]; then
        install -m 0644 "${unit_src}" /lib/systemd/system/zl-expense@.service
    elif [ ! -f /lib/systemd/system/zl-expense@.service ]; then
        die "missing zl-expense@.service"
    fi
    if [ -f /etc/systemd/system/zl-expense.service.d/profile.conf ] \
        && [ ! -f /etc/systemd/system/zl-expense@.service.d/profile.conf ]; then
        limits="$(grep -E '^(CPUQuota|MemoryMax|TasksMax)=' \
            /etc/systemd/system/zl-expense.service.d/profile.conf || true)"
        if [ -n "${limits}" ]; then
            install -d /etc/systemd/system/zl-expense@.service.d
            printf '[Service]\n%s\n' "${limits}" \
                >/etc/systemd/system/zl-expense@.service.d/profile.conf
        fi
    fi
    systemctl daemon-reload
}

install_deb() {
    [ -n "${DEB_PATH}" ] || return 0
    [ -f "${DEB_PATH}" ] || die "deb not found: ${DEB_PATH}"
    log "installing ${DEB_PATH} (running processes keep the previous inode)"
    DEBIAN_FRONTEND=noninteractive dpkg -i "${DEB_PATH}"
    install_slot_files
}

backup_db() {
    [ "${SKIP_BACKUP}" -eq 0 ] || return 0
    install -d -m 0750 -o root -g zl-expense "${BACKUP_DIR}"
    local stamp output
    stamp="$(date -u +%Y%m%dT%H%M%SZ)"
    output="${BACKUP_DIR}/pre-deploy-${stamp}.dump"
    log "writing database backup ${output}"
    zl-expense --config "${CONFIG}" backup --output "${output}"
}

migrate_db() {
    [ "${SKIP_MIGRATE}" -eq 0 ] || return 0
    log "applying migrations"
    zl-expense --config "${CONFIG}" db migrate
}

write_upstream() {
    local slot="${1}"
    local tmp
    tmp="$(mktemp "${UPSTREAM_FILE}.XXXXXX")"
    upstream_caddy_body "${slot}" >"${tmp}"
    chmod 0644 "${tmp}"
    mv "${tmp}" "${UPSTREAM_FILE}"
}

reload_caddy() {
    require_cmd caddy
    caddy validate --config "${CADDYFILE}" >/dev/null
    if systemctl is-active --quiet caddy.service; then
        systemctl reload caddy.service
    else
        systemctl enable --now caddy.service
    fi
}

ensure_caddy_origin() {
    [ -f "${CADDYFILE}" ] || die "missing ${CADDYFILE}"
    ensure_caddyfile_origin_import "${CADDYFILE}" "${ORIGIN_FILE}"
}

set_tunnel_origin() {
    local url="${1}"
    [ -f "${CLOUDFLARED_CONFIG}" ] || {
        log "cloudflared config missing; skipping tunnel rewrite"
        return 0
    }
    python3 - "${CLOUDFLARED_CONFIG}" "${url}" <<'PY'
import pathlib, sys, tempfile, os, stat as statmod
path = pathlib.Path(sys.argv[1])
url = sys.argv[2]
info = path.stat()
text = path.read_text()
old = text
lines = []
replaced = False
for line in text.splitlines(True):
    stripped = line.lstrip()
    if stripped.startswith("service:") and "127.0.0.1:" in stripped and "http_status" not in stripped:
        indent = line[: len(line) - len(stripped)]
        line = f"{indent}service: {url}\n"
        replaced = True
    lines.append(line)
if not replaced:
    raise SystemExit("no loopback cloudflared service line to rewrite")
new = "".join(lines)
if new == old:
    raise SystemExit(0)
fd, tmp = tempfile.mkstemp(prefix="cloudflared.", suffix=".yml", dir=str(path.parent))
os.write(fd, new.encode())
os.close(fd)
os.replace(tmp, path)
os.chown(path, info.st_uid, info.st_gid)
os.chmod(path, statmod.S_IMODE(info.st_mode))
PY
    if systemctl is-active --quiet "${CLOUDFLARED_UNIT}"; then
        install -d "/etc/systemd/system/${CLOUDFLARED_UNIT}.d"
        cat >"/etc/systemd/system/${CLOUDFLARED_UNIT}.d/origin.conf" <<'EOF'
[Unit]
After=network-online.target caddy.service
Wants=caddy.service
EOF
        systemctl daemon-reload
        log "restarting ${CLOUDFLARED_UNIT} onto ${url}"
        systemctl restart "${CLOUDFLARED_UNIT}"
    fi
}

start_slot() {
    local slot="${1}"
    local unit
    unit="$(slot_unit "${slot}")"
    log "starting ${unit} on 127.0.0.1:$(slot_port "${slot}")"
    systemctl enable "${unit}"
    systemctl restart "${unit}"
    wait_ready "$(slot_health_url "${slot}")"
}

stop_slot() {
    local slot="${1}"
    local unit
    unit="$(slot_unit "${slot}")"
    if systemctl is-active --quiet "${unit}"; then
        log "draining ${unit}"
        systemctl stop "${unit}"
    fi
    systemctl disable "${unit}" >/dev/null 2>&1 || true
}

disable_legacy_unit() {
    if systemctl is-active --quiet zl-expense.service; then
        log "stopping legacy zl-expense.service after origin is serving"
        systemctl stop zl-expense.service
    fi
    systemctl disable zl-expense.service >/dev/null 2>&1 || true
}

slots_active() {
    systemctl is-active --quiet "$(slot_unit "${SLOT_BLUE}")" \
        || systemctl is-active --quiet "$(slot_unit "${SLOT_GREEN}")"
}

bootstrap_origin() {
    local first_slot="${1}"
    log "bootstrapping loopback origin onto slot ${first_slot}"
    start_slot "${first_slot}"
    set_tunnel_origin "http://127.0.0.1:$(slot_port "${first_slot}")"
    write_upstream "${first_slot}"
    ensure_caddy_origin
    disable_legacy_unit
    reload_caddy
    wait_ready "http://127.0.0.1:${ORIGIN_PORT}/health/ready"
    set_tunnel_origin "http://127.0.0.1:${ORIGIN_PORT}"
    write_active_slot "${first_slot}"
}

cutover() {
    local active inactive
    active="$(read_active_slot)"
    [ -n "${active}" ] || die "active slot file missing; run bootstrap first"
    inactive="$(other_slot "${active}")"
    log "cutover ${active} -> ${inactive}"
    start_slot "${inactive}"
    write_upstream "${inactive}"
    reload_caddy
    wait_ready "http://127.0.0.1:${ORIGIN_PORT}/health/ready"
    write_active_slot "${inactive}"
    stop_slot "${active}"
}

main() {
    [ "$(id -u)" -eq 0 ] || die "run as root (sudo)"
    require_cmd curl
    require_cmd python3
    require_cmd systemctl
    [ -f "${CONFIG}" ] || die "missing ${CONFIG}"

    install_slot_files
    install_deb
    backup_db
    migrate_db

    local active
    active="$(read_active_slot)"
    if [ -z "${active}" ] && ! slots_active; then
        bootstrap_origin "${SLOT_BLUE}"
        log "bootstrap complete; active slot=${SLOT_BLUE}"
        return 0
    fi
    if [ -z "${active}" ]; then
        if systemctl is-active --quiet "$(slot_unit "${SLOT_BLUE}")"; then
            active="${SLOT_BLUE}"
        else
            active="${SLOT_GREEN}"
        fi
        write_active_slot "${active}"
    fi
    cutover
    log "deploy complete; active slot=$(read_active_slot)"
}

main "$@"
