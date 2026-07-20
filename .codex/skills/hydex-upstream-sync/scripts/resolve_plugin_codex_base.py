#!/usr/bin/env python3
"""Resolve the OpenAI Codex tag matching a bundled plugin codex-package.json."""

import argparse
import json
import subprocess
import sys
from pathlib import Path


OPENAI_CODEX_REMOTE = "https://github.com/openai/codex.git"
TARGET_PLATFORM = "linux-x64"


def run(args: list[str], *, cwd: Path, check: bool = True) -> subprocess.CompletedProcess[str]:
    print("+", " ".join(args), flush=True)
    return subprocess.run(args, cwd=cwd, text=True, check=check)


def capture(args: list[str], *, cwd: Path) -> str:
    print("+", " ".join(args), flush=True)
    return subprocess.check_output(args, cwd=cwd, text=True).strip()


def find_unpacked_baseline(plugin_dir: Path, baseline: str | None) -> Path:
    if baseline is not None:
        path = plugin_dir / "unpacked" / baseline
        if not path.exists():
            raise SystemExit(f"requested plugin baseline does not exist: {path}")
        return path

    candidates = sorted(
        (plugin_dir / "unpacked").glob(f"openai-chatgpt-*-{TARGET_PLATFORM}"),
        key=lambda path: path.stat().st_mtime,
        reverse=True,
    )
    if not candidates:
        raise SystemExit(f"no unpacked {TARGET_PLATFORM} plugin baseline found in {plugin_dir}")
    return candidates[0]


def codex_package_version(unpacked_baseline: Path) -> str:
    package_path = (
        unpacked_baseline
        / "extension"
        / "bin"
        / "linux-x86_64"
        / "codex-package.json"
    )
    data = json.loads(package_path.read_text())
    version = data.get("version")
    if not isinstance(version, str) or not version:
        raise SystemExit(f"missing codex-package version in {package_path}")
    return version


def ref_exists(repo: Path, ref: str) -> bool:
    return subprocess.run(
        ["git", "rev-parse", "--verify", "--quiet", ref],
        cwd=repo,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    ).returncode == 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--plugin-dir", type=Path, default=Path("hydex-plugin"))
    parser.add_argument("--baseline", help="Unpacked plugin baseline directory name.")
    parser.add_argument("--fetch-tag", action="store_true")
    parser.add_argument("--openai-remote", default=OPENAI_CODEX_REMOTE)
    args = parser.parse_args()

    repo = Path(capture(["git", "rev-parse", "--show-toplevel"], cwd=Path.cwd()))
    plugin_dir = (repo / args.plugin_dir).resolve()
    unpacked_baseline = find_unpacked_baseline(plugin_dir, args.baseline)
    version = codex_package_version(unpacked_baseline)
    tag = f"rust-v{version}"

    if args.fetch_tag:
        run(
            [
                "git",
                "fetch",
                args.openai_remote,
                f"refs/tags/{tag}:refs/tags/{tag}",
            ],
            cwd=repo,
        )

    if not ref_exists(repo, f"refs/tags/{tag}"):
        raise SystemExit(
            f"matching OpenAI Codex tag is not available locally: {tag}\n"
            "Rerun with --fetch-tag or fetch tags from https://github.com/openai/codex.git."
        )

    sha = capture(["git", "rev-parse", tag], cwd=repo)
    print("HYDEX_PLUGIN_CODEX_BASE")
    print(f"plugin_baseline={unpacked_baseline.name}")
    print(f"codex_package_version={version}")
    print(f"upstream_tag={tag}")
    print(f"upstream_sha={sha}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
