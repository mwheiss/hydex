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

for command in file jq rpmbuild rpm sed sha256sum; do
  if ! command -v "${command}" >/dev/null 2>&1; then
    echo "required build command is missing: ${command}" >&2
    exit 2
  fi
done

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
rg_binary="${extension_bin}/codex-path/rg"
bwrap_binary="${extension_bin}/codex-resources/bwrap"
package_metadata="${extension_bin}/codex-package.json"
license_file="${repo_root}/LICENSE"

for path in \
  "${codex_binary}" \
  "${code_mode_host_binary}" \
  "${rg_binary}" \
  "${bwrap_binary}" \
  "${package_metadata}" \
  "${license_file}"; do
  if [[ ! -f "${path}" ]]; then
    echo "required package input is missing: ${path}" >&2
    exit 2
  fi
done

package_version="$(jq -er '.version' "${package_metadata}")"
package_target="$(jq -er '.target' "${package_metadata}")"
package_variant="$(jq -er '.variant' "${package_metadata}")"
package_entrypoint="$(jq -er '.entrypoint' "${package_metadata}")"
reported_version="$(${codex_binary} --version 2>&1 | sed -n 's/^codex-cli //p' | tail -n 1)"

if [[ "${reported_version}" != "${package_version}" ]]; then
  echo "bundled Hydex version ${reported_version} does not match ${package_version}" >&2
  exit 2
fi
if [[ "${package_target}" != "x86_64-unknown-linux-musl" ]]; then
  echo "expected a Linux x86_64 musl package, got ${package_target}" >&2
  exit 2
fi
if [[ "${package_variant}" != "codex" || "${package_entrypoint}" != "bin/codex" ]]; then
  echo "unexpected canonical package metadata in ${package_metadata}" >&2
  exit 2
fi

help_output="$(${codex_binary} --help 2>&1)"
grep -q -- '--offload' <<< "${help_output}"
grep -q -- '--no-offload' <<< "${help_output}"
for binary in \
  "${codex_binary}" \
  "${code_mode_host_binary}" \
  "${rg_binary}" \
  "${bwrap_binary}"; do
  if ! file "${binary}" | grep -q 'static-pie linked'; then
    echo "RHEL package input is not static PIE: ${binary}" >&2
    exit 2
  fi
done

rpm_version="${package_version//-/_}"
topdir="${script_dir}/.rpmbuild"
rm -rf "${topdir}"
install -d \
  "${topdir}/BUILD" \
  "${topdir}/BUILDROOT" \
  "${topdir}/RPMS" \
  "${topdir}/SOURCES" \
  "${topdir}/SPECS" \
  "${topdir}/SRPMS" \
  "${topdir}/tmp"

install -m 0755 "${codex_binary}" "${topdir}/SOURCES/codex"
install -m 0755 "${code_mode_host_binary}" "${topdir}/SOURCES/codex-code-mode-host"
install -m 0755 "${rg_binary}" "${topdir}/SOURCES/rg"
install -m 0755 "${bwrap_binary}" "${topdir}/SOURCES/bwrap"
install -m 0644 "${package_metadata}" "${topdir}/SOURCES/codex-package.json"
install -m 0644 "${license_file}" "${topdir}/SOURCES/LICENSE"

codex_sha256="$(sha256sum "${codex_binary}" | cut -d' ' -f1)"
code_mode_host_sha256="$(sha256sum "${code_mode_host_binary}" | cut -d' ' -f1)"

rpmbuild \
  --define "_topdir ${topdir}" \
  --define "_tmppath ${topdir}/tmp" \
  --define "dist .el10" \
  --define "hydex_version ${rpm_version}" \
  --define "hydex_plugin_baseline ${baseline}" \
  --define "hydex_codex_sha256 ${codex_sha256}" \
  --define "hydex_code_mode_host_sha256 ${code_mode_host_sha256}" \
  -bb "${script_dir}/hydex.spec"

mapfile -t built_packages < <(find "${topdir}/RPMS" -type f -name '*.rpm' -print)
if [[ ${#built_packages[@]} -ne 1 ]]; then
  echo "expected exactly one built RPM, found ${#built_packages[@]}" >&2
  exit 2
fi

package_path="${script_dir}/$(basename "${built_packages[0]}")"
install -m 0644 "${built_packages[0]}" "${package_path}"
package_sha256="$(sha256sum "${package_path}" | cut -d' ' -f1)"

rpm -qip "${package_path}"
rpm -qlp "${package_path}"
unexpected_requires="$(rpm -qpR "${package_path}" | grep -v '^rpmlib(' || true)"
if [[ -n "${unexpected_requires}" ]]; then
  echo "RHEL package unexpectedly has runtime dependencies:" >&2
  echo "${unexpected_requires}" >&2
  exit 2
fi

cat <<EOF
HYDEX_RHEL10_PACKAGE_SUMMARY
plugin_baseline=${baseline}
codex_version=${package_version}
codex_sha256=${codex_sha256}
code_mode_host_sha256=${code_mode_host_sha256}
package=${package_path}
package_sha256=${package_sha256}
install_command=sudo dnf install ${package_path}
update_command=sudo dnf upgrade ${package_path}
EOF
