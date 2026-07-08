#!/usr/bin/env python3
"""Refresh the Hydex Linux x64 VS Code plugin bundle with the current CLI."""

from __future__ import annotations

import argparse
import hashlib
import subprocess
import zipfile
from pathlib import Path


DEFAULT_BASELINE = "openai-chatgpt-26.5623.141536-linux-x64"
DEFAULT_OUTPUT = "hydex-chatgpt-26.5623.141536-linux-x64.vsix"


def run(cmd: list[str], cwd: Path, *, echo_output: bool = True) -> str:
    print(f"+ ({cwd}) {' '.join(cmd)}")
    proc = subprocess.run(
        cmd,
        cwd=cwd,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    if echo_output and proc.stdout:
        print(proc.stdout, end="" if proc.stdout.endswith("\n") else "\n")
    if proc.returncode != 0:
        raise SystemExit(proc.returncode)
    return proc.stdout


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def validate_zip(path: Path) -> None:
    print(f"+ validate zip {path}")
    with zipfile.ZipFile(path) as archive:
        bad_file = archive.testzip()
    if bad_file is not None:
        raise SystemExit(f"zip validation failed for {bad_file}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    parser.add_argument("--plugin-dir", type=Path, default=None)
    parser.add_argument("--baseline", default=DEFAULT_BASELINE)
    parser.add_argument("--output", default=DEFAULT_OUTPUT)
    parser.add_argument("--hydex-bin", type=Path, default=None)
    parser.add_argument("--skip-build", action="store_true")
    args = parser.parse_args()

    repo = args.repo.resolve()
    plugin_dir = (args.plugin_dir or repo / "hydex-plugin").resolve()
    hydex_bin = (args.hydex_bin or repo / "codex-rs" / "target" / "release" / "codex").resolve()
    unpacked_root = plugin_dir / "unpacked" / args.baseline
    extension_dir = unpacked_root / "extension"
    output_vsix = plugin_dir / "dist" / args.output
    bundled_bin = extension_dir / "bin" / "linux-x86_64" / "codex"
    composer_js = extension_dir / "webview" / "assets" / "composer-ChmJYQgb.js"
    request_js = extension_dir / "webview" / "assets" / "thread-context-inputs-pO1QHnPh.js"

    if not args.skip_build:
        run(["cargo", "build", "-p", "codex-cli", "--release"], repo / "codex-rs")

    for path in [plugin_dir, extension_dir, hydex_bin]:
        if not path.exists():
            raise SystemExit(f"missing required path: {path}")

    hydex_sha = sha256(hydex_bin)
    run(
        [
            "python3",
            "scripts/apply_hydex_patch.py",
            str(extension_dir.relative_to(plugin_dir)),
            "--hydex-bin",
            str(hydex_bin),
        ],
        plugin_dir,
    )
    run(
        [
            "python3",
            "scripts/repack_vsix.py",
            str(unpacked_root.relative_to(plugin_dir)),
            str(output_vsix.relative_to(plugin_dir)),
        ],
        plugin_dir,
    )
    validate_zip(output_vsix)
    run(["node", "--check", str(composer_js.relative_to(plugin_dir))], plugin_dir)
    run(["node", "--check", str(request_js.relative_to(plugin_dir))], plugin_dir)
    help_text = run([str(bundled_bin), "--help"], plugin_dir, echo_output=False)
    if "--offload" not in help_text or "--no-offload" not in help_text:
        raise SystemExit("bundled Hydex CLI help did not include --offload and --no-offload")

    print("HYDEX_PLUGIN_REFRESH_SUMMARY")
    print(f"hydex_commit={run(['git', 'rev-parse', '--short=10', 'HEAD'], repo).strip()}")
    print(f"release_binary_sha256={hydex_sha}")
    print(f"bundled_binary_sha256={sha256(bundled_bin)}")
    print(f"vsix_sha256={sha256(output_vsix)}")
    print(f"vsix={output_vsix}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
