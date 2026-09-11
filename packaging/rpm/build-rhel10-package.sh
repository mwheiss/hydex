#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
HYDEX_RHEL_MAJOR=10 exec "${script_dir}/build-rpm-package.sh" "$@"
