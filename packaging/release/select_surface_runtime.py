#!/usr/bin/env python3
"""Select the newer validated Hydex runtime from plugin and desktop surfaces."""

import argparse
import json
import re
import subprocess
from dataclasses import dataclass
from pathlib import Path

SEMVER_RE = re.compile(
    r"^(?P<major>0|[1-9][0-9]*)\."
    r"(?P<minor>0|[1-9][0-9]*)\."
    r"(?P<patch>0|[1-9][0-9]*)"
    r"(?:-(?P<prerelease>[0-9A-Za-z.-]+))?"
    r"(?:\+[0-9A-Za-z.-]+)?$"
)
CODEX_VERSION_RE = re.compile(r"^codex-cli\s+(\S+)$", re.MULTILINE)


@dataclass(frozen=True)
class SemVer:
    core: tuple[int, int, int]
    prerelease: tuple[int | str, ...] | None


@dataclass(frozen=True)
class SurfaceRuntime:
    surface: str
    root: Path
    version: str


def parse_semver(value: str) -> SemVer:
    match = SEMVER_RE.fullmatch(value)
    if match is None:
        raise ValueError(f"invalid Codex semantic version: {value}")
    prerelease_text = match.group("prerelease")
    prerelease = None
    if prerelease_text is not None:
        prerelease = tuple(
            int(part) if part.isdigit() else part for part in prerelease_text.split(".")
        )
    return SemVer(
        core=(
            int(match.group("major")),
            int(match.group("minor")),
            int(match.group("patch")),
        ),
        prerelease=prerelease,
    )


def compare_prerelease(
    left: tuple[int | str, ...] | None,
    right: tuple[int | str, ...] | None,
) -> int:
    if left is None or right is None:
        return (left is None) - (right is None)
    for left_part, right_part in zip(left, right):
        if left_part == right_part:
            continue
        if isinstance(left_part, int) and isinstance(right_part, str):
            return -1
        if isinstance(left_part, str) and isinstance(right_part, int):
            return 1
        return (left_part > right_part) - (left_part < right_part)
    return (len(left) > len(right)) - (len(left) < len(right))


def compare_versions(left: str, right: str) -> int:
    left_version = parse_semver(left)
    right_version = parse_semver(right)
    if left_version.core != right_version.core:
        return (left_version.core > right_version.core) - (
            left_version.core < right_version.core
        )
    return compare_prerelease(left_version.prerelease, right_version.prerelease)


def read_runtime(surface: str, root: Path) -> SurfaceRuntime:
    resolved = root.resolve()
    metadata_path = resolved / "codex-package.json"
    binary = resolved / "bin" / "codex"
    if not metadata_path.is_file():
        raise SystemExit(f"{surface} runtime metadata is missing: {metadata_path}")
    if not binary.is_file():
        raise SystemExit(f"{surface} Hydex CLI is missing: {binary}")
    metadata = json.loads(metadata_path.read_text())
    version = metadata.get("version")
    if not isinstance(version, str):
        raise SystemExit(f"{surface} runtime has no string version: {metadata_path}")
    parse_semver(version)
    output = subprocess.run(
        [str(binary), "--version"],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=True,
    ).stdout
    match = CODEX_VERSION_RE.search(output)
    if match is None or match.group(1) != version:
        raise SystemExit(
            f"{surface} Hydex CLI version does not match metadata: "
            f"{match.group(1) if match else output.strip()} != {version}"
        )
    help_output = subprocess.run(
        [str(binary), "--help"],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=True,
    ).stdout
    if "--offload" not in help_output or "--no-offload" not in help_output:
        raise SystemExit(f"{surface} Hydex CLI is missing offload flags: {binary}")
    return SurfaceRuntime(surface=surface, root=resolved, version=version)


def select_runtime(plugin: SurfaceRuntime, desktop: SurfaceRuntime) -> SurfaceRuntime:
    comparison = compare_versions(plugin.version, desktop.version)
    return desktop if comparison < 0 else plugin


def matrix(plugin_root: Path, desktop_root: Path) -> dict[str, object]:
    plugin = read_runtime("plugin", plugin_root)
    desktop = read_runtime("desktop", desktop_root)
    selected = select_runtime(plugin, desktop)
    return {
        "plugin": {"version": plugin.version, "runtimeRoot": str(plugin.root)},
        "desktop": {"version": desktop.version, "runtimeRoot": str(desktop.root)},
        "selected": {
            "surface": selected.surface,
            "version": selected.version,
            "runtimeRoot": str(selected.root),
        },
        "versionsDiverge": plugin.version != desktop.version,
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--plugin-runtime-root", type=Path, required=True)
    parser.add_argument("--desktop-runtime-root", type=Path, required=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    print(
        json.dumps(
            matrix(args.plugin_runtime_root, args.desktop_runtime_root),
            indent=2,
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
