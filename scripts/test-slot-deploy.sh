#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/slot-deploy.sh
source "${ROOT}/scripts/lib/slot-deploy.sh"

fail() {
    printf '[test-slot-deploy] ERROR: %s\n' "$*" >&2
    exit 1
}

[ "$(other_slot blue)" = green ] || fail 'other_slot blue'
[ "$(other_slot green)" = blue ] || fail 'other_slot green'
[ "$(slot_port blue)" = 8081 ] || fail 'slot_port blue'
[ "$(slot_port green)" = 8082 ] || fail 'slot_port green'
[ "$(slot_unit blue)" = 'zl-expense@blue.service' ] || fail 'slot_unit'
[ "$(slot_health_url green)" = 'http://127.0.0.1:8082/health/ready' ] || fail 'health url'
[ "$(upstream_caddy_body blue)" = 'reverse_proxy 127.0.0.1:8081' ] || fail 'upstream body'

tmp="$(mktemp)"
cat >"${tmp}" <<'EOF'
:80 {
	root * /usr/share/caddy
	file_server
}
EOF
ensure_caddyfile_origin_import "${tmp}" /etc/zl-expense/caddy/origin.caddy
caddyfile_has_origin_import "${tmp}" /etc/zl-expense/caddy/origin.caddy \
    || fail 'import not added'
ensure_caddyfile_origin_import "${tmp}" /etc/zl-expense/caddy/origin.caddy
count="$(grep -c 'import /etc/zl-expense/caddy/origin.caddy' "${tmp}")"
[ "${count}" -eq 1 ] || fail "import duplicated (${count})"
rm -f "${tmp}"

grep -q 'upgrade)' "${ROOT}/packaging/debian/DEBIAN/prerm" || fail 'prerm missing upgrade'
if grep -A3 '^upgrade)' "${ROOT}/packaging/debian/DEBIAN/prerm" | grep -q 'stop_units'; then
    fail 'prerm still stops units on upgrade'
fi

printf '[test-slot-deploy] ok\n'
