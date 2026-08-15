# SPDX-License-Identifier: MIT
# shellcheck shell=bash
# Shared helpers for unsigned development packaging.

packaging_root() {
    local lib_dir
    lib_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    cd "${lib_dir}/../.." && pwd
}

load_package_version() {
    local root version_file
    root="$(packaging_root)"
    version_file="${root}/packaging/version.env"
    if [ ! -f "${version_file}" ]; then
        printf 'missing packaging/version.env\n' >&2
        return 1
    fi
    # shellcheck source=packaging/version.env
    source "${version_file}"
}

normalize_deb_arch() {
    local arch="${1:-}"
    case "${arch}" in
    x86_64 | amd64)
        printf 'amd64'
        ;;
    aarch64 | arm64)
        printf 'arm64'
        ;;
    *)
        printf 'unsupported architecture: %s\n' "${arch}" >&2
        return 1
        ;;
    esac
}

stage_file() {
    local mode src dest
    mode="${1}"
    src="${2}"
    dest="${3}"

    install -d "$(dirname "${dest}")"
    install -m "${mode}" "${src}" "${dest}"

    if [ "$(id -u)" -eq 0 ]; then
        chown root:root "${dest}"
    fi
}

stage_tree() {
    local src_dir dest_dir file_mode dir_mode rel_path
    src_dir="${1}"
    dest_dir="${2}"
    file_mode="${3:-0644}"
    dir_mode="${4:-0755}"

    while IFS= read -r -d '' file_path; do
        rel_path="${file_path#"${src_dir}/"}"
        stage_file "${file_mode}" "${file_path}" "${dest_dir}/${rel_path}"
    done < <(find "${src_dir}" -type f -print0)

    while IFS= read -r -d '' dir_path; do
        rel_path="${dir_path#"${src_dir}/"}"
        install -d -m "${dir_mode}" "${dest_dir}/${rel_path}"
        if [ "$(id -u)" -eq 0 ]; then
            chown root:root "${dest_dir}/${rel_path}"
        fi
    done < <(find "${src_dir}" -type d -print0)
}

substitute_control() {
    local template dest version arch
    template="${1}"
    dest="${2}"
    version="${3}"
    arch="${4}"

  sed \
        -e "s/@VERSION@/${version}/g" \
        -e "s/@ARCH@/${arch}/g" \
        "${template}" >"${dest}"
}
