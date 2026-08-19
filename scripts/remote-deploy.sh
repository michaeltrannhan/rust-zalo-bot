#!/usr/bin/env bash
# Copy a Debian package to a Linux host over SSH and run host-slot-deploy.sh.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

DEPLOY_HOST="${DEPLOY_HOST:-}"
DEPLOY_USER="${DEPLOY_USER:-ubuntu}"
DEPLOY_PORT="${DEPLOY_PORT:-22}"
REMOTE_DIR="${REMOTE_DIR:-/tmp/zl-expense-deploy}"
DEB_PATH="${1:-}"

usage() {
    cat <<'EOF'
Usage: remote-deploy.sh <deb-file>

Environment:
  DEPLOY_HOST     SSH hostname or IP (required)
  DEPLOY_USER     SSH user (default: ubuntu)
  DEPLOY_PORT     SSH port (default: 22)
  SSH_IDENTITY    optional IdentityFile
  REMOTE_DIR      remote staging directory
EOF
}

[ -n "${DEB_PATH}" ] || {
    usage >&2
    exit 2
}
[ -f "${DEB_PATH}" ] || {
    printf 'deb not found: %s\n' "${DEB_PATH}" >&2
    exit 1
}
[ -n "${DEPLOY_HOST}" ] || {
    printf 'DEPLOY_HOST is required\n' >&2
    exit 1
}

ssh_base=(ssh -o BatchMode=yes -o IdentitiesOnly=yes -p "${DEPLOY_PORT}")
scp_base=(scp -o BatchMode=yes -o IdentitiesOnly=yes -P "${DEPLOY_PORT}")
if [ -n "${SSH_IDENTITY:-}" ]; then
    ssh_base+=(-i "${SSH_IDENTITY}")
    scp_base+=(-i "${SSH_IDENTITY}")
fi

remote="${DEPLOY_USER}@${DEPLOY_HOST}"
deb_name="$(basename "${DEB_PATH}")"

"${ssh_base[@]}" "${remote}" "mkdir -p '${REMOTE_DIR}'"
"${scp_base[@]}" \
    "${DEB_PATH}" \
    "${ROOT}/scripts/host-slot-deploy.sh" \
    "${ROOT}/scripts/lib/slot-deploy.sh" \
    "${ROOT}/deploy/caddy/origin.caddy" \
    "${ROOT}/deploy/systemd/zl-expense@.service" \
    "${remote}:${REMOTE_DIR}/"

"${ssh_base[@]}" "${remote}" "bash -s" <<EOF
set -euo pipefail
cd '${REMOTE_DIR}'
install -d /tmp/zl-expense-deploy-src/scripts/lib /tmp/zl-expense-deploy-src/deploy/caddy /tmp/zl-expense-deploy-src/deploy/systemd
cp host-slot-deploy.sh /tmp/zl-expense-deploy-src/scripts/
cp slot-deploy.sh /tmp/zl-expense-deploy-src/scripts/lib/
cp origin.caddy /tmp/zl-expense-deploy-src/deploy/caddy/
cp 'zl-expense@.service' /tmp/zl-expense-deploy-src/deploy/systemd/
chmod +x /tmp/zl-expense-deploy-src/scripts/host-slot-deploy.sh
sudo /tmp/zl-expense-deploy-src/scripts/host-slot-deploy.sh --deb '${REMOTE_DIR}/${deb_name}'
EOF
