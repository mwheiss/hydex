#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "${script_dir}/../.." && pwd)"
plugin_dir="${HYDEX_PLUGIN_DIR:-${repo_root}/hydex-plugin}"
baseline="${HYDEX_PLUGIN_BASELINE:-}"

if [[ $# -gt 0 ]]; then
  if [[ $# -ne 2 || $1 != "--baseline" ]]; then
    echo "usage: $0 [--baseline openai-chatgpt-<version>-linux-x64]" >&2
    exit 2
  fi
  baseline="$2"
fi

if [[ -z "${baseline}" ]]; then
  mapfile -t baselines < <(
    find "${plugin_dir}/unpacked" \
      -mindepth 1 \
      -maxdepth 1 \
      -type d \
      -name 'openai-chatgpt-*-linux-x64' \
      -printf '%f\n' \
      | sort -V
  )
  if [[ ${#baselines[@]} -eq 0 ]]; then
    echo "no unpacked Linux x64 plugin baseline found under ${plugin_dir}" >&2
    exit 2
  fi
  baseline="${baselines[$((${#baselines[@]} - 1))]}"
fi

extension_bin="${plugin_dir}/unpacked/${baseline}/extension/bin/linux-x86_64"
codex_binary="${extension_bin}/codex"
code_mode_host_binary="${extension_bin}/codex-code-mode-host"
package_metadata="${extension_bin}/codex-package.json"
license_file="${repo_root}/LICENSE"

for path in \
  "${codex_binary}" \
  "${code_mode_host_binary}" \
  "${package_metadata}" \
  "${license_file}"; do
  if [[ ! -f "${path}" ]]; then
    echo "required package input is missing: ${path}" >&2
    exit 2
  fi
done

package_version="$(jq -er '.version' "${package_metadata}")"
reported_version="$(
  "${codex_binary}" --version 2>&1 \
    | sed -n 's/^codex-cli //p' \
    | tail -n 1
)"
if [[ "${reported_version}" != "${package_version}" ]]; then
  echo "bundled Hydex version ${reported_version} does not match ${package_version}" >&2
  exit 2
fi

help_output="$("${codex_binary}" --help 2>&1)"
grep -q -- '--offload' <<< "${help_output}"
grep -q -- '--no-offload' <<< "${help_output}"
file "${codex_binary}" | grep -q 'static-pie linked'
file "${code_mode_host_binary}" | grep -q 'static-pie linked'

export HYDEX_PACKAGE_VERSION="${package_version}"
export HYDEX_PLUGIN_BASELINE="${baseline}"
export HYDEX_CODEX_BINARY
HYDEX_CODEX_BINARY="$(realpath "${codex_binary}")"
export HYDEX_CODE_MODE_HOST_BINARY
HYDEX_CODE_MODE_HOST_BINARY="$(realpath "${code_mode_host_binary}")"
export HYDEX_LICENSE_FILE
HYDEX_LICENSE_FILE="$(realpath "${license_file}")"
export HYDEX_CODEX_SHA256
HYDEX_CODEX_SHA256="$(sha256sum "${codex_binary}" | cut -d' ' -f1)"
export HYDEX_CODE_MODE_HOST_SHA256
HYDEX_CODE_MODE_HOST_SHA256="$(
  sha256sum "${code_mode_host_binary}" | cut -d' ' -f1
)"
export HYDEX_LICENSE_SHA256
HYDEX_LICENSE_SHA256="$(sha256sum "${license_file}" | cut -d' ' -f1)"

(
  cd "${script_dir}"
  makepkg --clean --cleanbuild --force
)

package_path="$(
  cd "${script_dir}"
  makepkg --packagelist
)"
package_sha256="$(sha256sum "${package_path}" | cut -d' ' -f1)"

cat <<EOF
HYDEX_ARCH_PACKAGE_SUMMARY
plugin_baseline=${baseline}
codex_version=${package_version}
codex_sha256=${HYDEX_CODEX_SHA256}
code_mode_host_sha256=${HYDEX_CODE_MODE_HOST_SHA256}
package=${package_path}
package_sha256=${package_sha256}
install_command=sudo pacman -U ${package_path}
EOF
