#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# shellcheck shell=bash
# Shared helpers for blue/green VM slot deployment.

SLOT_BLUE="blue"
SLOT_GREEN="green"
SLOT_BLUE_PORT="8081"
SLOT_GREEN_PORT="8082"

other_slot() {
    case "${1}" in
    "${SLOT_BLUE}")
        printf '%s' "${SLOT_GREEN}"
        ;;
    "${SLOT_GREEN}")
        printf '%s' "${SLOT_BLUE}"
        ;;
    *)
        printf 'unknown slot: %s\n' "${1}" >&2
        return 1
        ;;
    esac
}

slot_port() {
    case "${1}" in
    "${SLOT_BLUE}")
        printf '%s' "${SLOT_BLUE_PORT}"
        ;;
    "${SLOT_GREEN}")
        printf '%s' "${SLOT_GREEN_PORT}"
        ;;
    *)
        printf 'unknown slot: %s\n' "${1}" >&2
        return 1
        ;;
    esac
}

slot_health_url() {
    printf 'http://127.0.0.1:%s/health/ready' "$(slot_port "${1}")"
}

slot_unit() {
    printf 'zl-expense@%s.service' "${1}"
}

upstream_caddy_body() {
    printf 'reverse_proxy 127.0.0.1:%s\n' "$(slot_port "${1}")"
}

caddyfile_has_origin_import() {
    local caddyfile="${1}"
    local import_path="${2}"
    grep -Fq "import ${import_path}" "${caddyfile}"
}

ensure_caddyfile_origin_import() {
    local caddyfile="${1}"
    local import_path="${2}"
    if caddyfile_has_origin_import "${caddyfile}" "${import_path}"; then
        return 0
    fi
    printf '\nimport %s\n' "${import_path}" >>"${caddyfile}"
}
