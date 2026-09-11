#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "${script_dir}/.." && pwd)"
plugin_runtime_root=""
desktop_runtime_root=""
plan_only=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --plugin-runtime-root)
      [[ $# -ge 2 ]] || { echo "--plugin-runtime-root requires a value" >&2; exit 2; }
      plugin_runtime_root="$2"
      shift 2
      ;;
    --desktop-runtime-root)
      [[ $# -ge 2 ]] || { echo "--desktop-runtime-root requires a value" >&2; exit 2; }
      desktop_runtime_root="$2"
      shift 2
      ;;
    --plan-only)
      plan_only=1
      shift
      ;;
    *)
      echo "usage: $0 --plugin-runtime-root PATH --desktop-runtime-root PATH [--plan-only]" >&2
      exit 2
      ;;
  esac
done

[[ -n "${plugin_runtime_root}" ]] || { echo "missing --plugin-runtime-root" >&2; exit 2; }
[[ -n "${desktop_runtime_root}" ]] || { echo "missing --desktop-runtime-root" >&2; exit 2; }

matrix="$(${repo_root}/packaging/release/select_surface_runtime.py \
  --plugin-runtime-root "${plugin_runtime_root}" \
  --desktop-runtime-root "${desktop_runtime_root}")"
selected_surface="$(jq -er '.selected.surface' <<< "${matrix}")"
selected_version="$(jq -er '.selected.version' <<< "${matrix}")"
selected_root="$(jq -er '.selected.runtimeRoot' <<< "${matrix}")"

cat <<EOF
HYDEX_LOCAL_PACKAGE_PLAN
plugin_version=$(jq -er '.plugin.version' <<< "${matrix}")
desktop_version=$(jq -er '.desktop.version' <<< "${matrix}")
selected_surface=${selected_surface}
selected_version=${selected_version}
selected_runtime_root=${selected_root}
EOF

[[ "${plan_only}" -eq 0 ]] || exit 0

export HYDEX_RUNTIME_LABEL="${selected_surface}:${selected_version}"
export HYDEX_PLUGIN_BASELINE=""
"${repo_root}/packaging/arch/build-local-package.sh" --runtime-root "${selected_root}"
"${repo_root}/packaging/rpm/build-rhel7-package.sh" --runtime-root "${selected_root}"
"${repo_root}/packaging/rpm/build-rhel10-package.sh" --runtime-root "${selected_root}"
