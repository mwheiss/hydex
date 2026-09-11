#!/usr/bin/env python3
"""Create one canonical Hydex runtime root from a host application surface."""

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import tempfile
from pathlib import Path

TARGET = "x86_64-unknown-linux-musl"
VERSION_RE = re.compile(r"^codex-cli\s+(\S+)$", re.MULTILINE)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def run_text(command: list[str]) -> str:
    result = subprocess.run(
        command,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    if result.returncode != 0:
        raise SystemExit(
            f"command failed ({result.returncode}): {' '.join(command)}\n{result.stdout}"
        )
    return result.stdout


def binary_version(path: Path) -> str:
    match = VERSION_RE.search(run_text([str(path), "--version"]))
    if match is None:
        raise SystemExit(f"could not parse {path} --version")
    return match.group(1)


def require_static_pie(path: Path, label: str) -> None:
    if not path.is_file():
        raise SystemExit(f"{label} is missing: {path}")
    description = run_text(["file", str(path)])
    if "static-pie linked" not in description:
        raise SystemExit(f"{label} is not static PIE: {description.strip()}")


def copy(path: Path, destination: Path, mode: int) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(path, destination)
    destination.chmod(mode)


def source_inputs(args: argparse.Namespace) -> tuple[str, dict[str, Path]]:
    source_root = args.source_root.resolve()
    if args.surface == "plugin":
        metadata_path = source_root / "codex-package.json"
        metadata = json.loads(metadata_path.read_text())
        version = metadata.get("version")
        if not isinstance(version, str) or not version:
            raise SystemExit(f"plugin runtime version is missing: {metadata_path}")
        inputs = {
            "bin/codex-code-mode-host": source_root / "codex-code-mode-host",
            "codex-path/rg": source_root / "codex-path" / "rg",
            "codex-resources/bwrap": source_root / "codex-resources" / "bwrap",
        }
    else:
        version = binary_version(source_root / "codex")
        if args.bwrap is None:
            raise SystemExit("desktop runtime preparation requires --bwrap")
        inputs = {
            "bin/codex-code-mode-host": source_root / "codex-code-mode-host",
            "codex-path/rg": source_root / "rg",
            "codex-resources/bwrap": args.bwrap.resolve(),
        }
    return version, inputs


def prepare(args: argparse.Namespace) -> dict[str, object]:
    hydex = args.hydex_bin.resolve()
    version, inputs = source_inputs(args)
    require_static_pie(hydex, "Hydex CLI")
    actual_version = binary_version(hydex)
    if actual_version != version:
        raise SystemExit(
            f"Hydex CLI version {actual_version} does not match {args.surface} {version}"
        )
    help_output = run_text([str(hydex), "--help"])
    if "--offload" not in help_output or "--no-offload" not in help_output:
        raise SystemExit("Hydex CLI is missing --offload or --no-offload")
    for relative, path in inputs.items():
        require_static_pie(path, relative)

    output = args.output_root.resolve()
    if output.exists():
        raise SystemExit(f"surface runtime output already exists: {output}")
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(
        prefix=".surface-runtime-", dir=output.parent
    ) as temp:
        staged = Path(temp) / output.name
        all_inputs = {"bin/codex": hydex, **inputs}
        for relative, path in all_inputs.items():
            copy(path, staged / relative, 0o755)
        metadata = {
            "version": version,
            "target": TARGET,
            "variant": "codex",
            "entrypoint": "bin/codex",
        }
        (staged / "codex-package.json").write_text(
            json.dumps(metadata, indent=2, sort_keys=True) + "\n"
        )
        surface = {
            "schemaVersion": 1,
            "surface": args.surface,
            "sourceLabel": args.source_label,
            "version": version,
            "hydexCommit": args.hydex_commit,
            "files": {
                relative: sha256(staged / relative) for relative in sorted(all_inputs)
            },
        }
        (staged / "surface.json").write_text(
            json.dumps(surface, indent=2, sort_keys=True) + "\n"
        )
        os.replace(staged, output)
    return {**surface, "runtimeRoot": str(output)}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--surface", choices=("plugin", "desktop"), required=True)
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--hydex-bin", type=Path, required=True)
    parser.add_argument("--bwrap", type=Path)
    parser.add_argument("--output-root", type=Path, required=True)
    parser.add_argument("--source-label", required=True)
    parser.add_argument("--hydex-commit", required=True)
    return parser.parse_args()


def main() -> int:
    summary = prepare(parse_args())
    print("HYDEX_SURFACE_RUNTIME_SUMMARY")
    for key, value in summary.items():
        rendered = (
            json.dumps(value, sort_keys=True) if isinstance(value, dict) else value
        )
        print(f"{key}={rendered}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
