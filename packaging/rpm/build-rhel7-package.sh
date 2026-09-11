#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
HYDEX_RHEL_MAJOR=7 exec "${script_dir}/build-rpm-package.sh" "$@"
