#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

log() {
    printf '[security-audit] %s\n' "$*"
}

if [ ! -f "${ROOT}/Cargo.lock" ]; then
    printf 'missing Cargo.lock\n' >&2
    exit 1
fi

if grep -E '^\s+name = "(openssl|native-tls)"$' "${ROOT}/Cargo.lock" >/dev/null; then
    printf 'forbidden TLS stack in Cargo.lock\n' >&2
    exit 1
fi

if command -v cargo-deny >/dev/null 2>&1; then
    log "running cargo deny"
    (cd "${ROOT}" && cargo deny check)
elif command -v cargo >/dev/null 2>&1 && cargo deny --version >/dev/null 2>&1; then
    log "running cargo deny"
    (cd "${ROOT}" && cargo deny check)
else
    log "cargo-deny not installed; lockfile TLS check only"
fi

log "ok"
