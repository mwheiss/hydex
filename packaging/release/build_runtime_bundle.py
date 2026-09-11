#!/usr/bin/env python3

import argparse
import gzip
import hashlib
import json
import os
import re
import shutil
import subprocess
import tarfile
import tempfile
from pathlib import Path

TARGET = "x86_64-unknown-linux-musl"
BASELINE_RE = re.compile(r"^openai-chatgpt-(?P<version>[0-9.]+)-linux-x64$")
CODEX_VERSION_RE = re.compile(r"^[0-9A-Za-z][0-9A-Za-z.+~-]*$")
RELEASE_RE = re.compile(r"^[1-9][0-9]*$")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def run_text(command: list[str], cwd: Path) -> str:
    result = subprocess.run(
        command,
        cwd=cwd,
        check=False,
        capture_output=True,
        text=True,
    )
    output = result.stdout + result.stderr
    if result.returncode != 0:
        rendered = " ".join(command)
        raise SystemExit(f"command failed ({result.returncode}): {rendered}\n{output}")
    return output


def require_file(path: Path) -> None:
    if not path.is_file():
        raise SystemExit(f"required runtime input is missing: {path}")


def require_static_pie(path: Path, repo: Path) -> None:
    description = run_text(["file", str(path)], repo)
    if "static-pie linked" not in description:
        raise SystemExit(f"runtime input is not static PIE: {path}\n{description}")


def git_output(repo: Path, *args: str) -> str:
    return run_text(["git", *args], repo).strip()


def validate_git_state(
    repo: Path, hydex_commit: str, allow_dirty: bool
) -> tuple[str, int]:
    packaging_commit = git_output(repo, "rev-parse", "HEAD")
    resolved_hydex_commit = git_output(repo, "rev-parse", f"{hydex_commit}^{{commit}}")
    if not allow_dirty:
        status = git_output(repo, "status", "--porcelain", "--untracked-files=normal")
        if status:
            raise SystemExit(
                "Hydex worktree changes would make a public runtime bundle ambiguous"
            )
    epoch_text = git_output(repo, "show", "-s", "--format=%ct", packaging_commit)
    return resolved_hydex_commit, int(epoch_text)


def add_directory(archive: tarfile.TarFile, path: str, epoch: int) -> None:
    info = tarfile.TarInfo(path.rstrip("/") + "/")
    info.type = tarfile.DIRTYPE
    info.mode = 0o755
    info.uid = 0
    info.gid = 0
    info.uname = "root"
    info.gname = "root"
    info.mtime = epoch
    archive.addfile(info)


def add_file(
    archive: tarfile.TarFile,
    source: Path,
    archive_path: str,
    mode: int,
    epoch: int,
) -> None:
    info = tarfile.TarInfo(archive_path)
    info.size = source.stat().st_size
    info.mode = mode
    info.uid = 0
    info.gid = 0
    info.uname = "root"
    info.gname = "root"
    info.mtime = epoch
    with source.open("rb") as handle:
        archive.addfile(info, handle)


def write_deterministic_archive(
    source_root: Path,
    archive_path: Path,
    root_name: str,
    epoch: int,
) -> None:
    temporary_path = archive_path.with_suffix(archive_path.suffix + ".tmp")
    with (
        temporary_path.open("wb") as raw_handle,
        gzip.GzipFile(
            filename="",
            mode="wb",
            compresslevel=9,
            fileobj=raw_handle,
            mtime=epoch,
        ) as gzip_handle,
        tarfile.open(
            fileobj=gzip_handle,
            mode="w",
            format=tarfile.PAX_FORMAT,
        ) as archive,
    ):
        add_directory(archive, root_name, epoch)
        directories = sorted(path for path in source_root.rglob("*") if path.is_dir())
        for directory in directories:
            relative = directory.relative_to(source_root).as_posix()
            add_directory(archive, f"{root_name}/{relative}", epoch)
        files = sorted(path for path in source_root.rglob("*") if path.is_file())
        for path in files:
            relative = path.relative_to(source_root).as_posix()
            mode = 0o755 if os.access(path, os.X_OK) else 0o644
            add_file(archive, path, f"{root_name}/{relative}", mode, epoch)
    os.replace(temporary_path, archive_path)


def copy_file(source: Path, destination: Path, mode: int) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, destination)
    destination.chmod(mode)


def build_bundle(args: argparse.Namespace) -> tuple[Path, Path, dict[str, object]]:
    repo = args.repo.resolve()
    plugin_dir = args.plugin_dir.resolve()
    baseline_match = BASELINE_RE.fullmatch(args.baseline or "")
    if args.runtime_root is None and baseline_match is None:
        raise SystemExit(
            "--baseline must be openai-chatgpt-<extension-version>-linux-x64 "
            "when --runtime-root is omitted"
        )
    if RELEASE_RE.fullmatch(args.release) is None:
        raise SystemExit("release must be a positive integer")

    hydex_commit, source_date_epoch = validate_git_state(
        repo, args.hydex_commit, args.allow_dirty
    )
    packaging_commit = git_output(repo, "rev-parse", "HEAD")
    runtime_root = (
        args.runtime_root.resolve() if args.runtime_root is not None else None
    )
    extension_bin = runtime_root or (
        plugin_dir / "unpacked" / args.baseline / "extension" / "bin" / "linux-x86_64"
    )
    surface_metadata = {}
    if runtime_root is not None and (runtime_root / "surface.json").is_file():
        surface_metadata = json.loads((runtime_root / "surface.json").read_text())
    inputs = {
        "bin/codex": extension_bin / ("bin/codex" if runtime_root else "codex"),
        "bin/codex-code-mode-host": extension_bin
        / ("bin/codex-code-mode-host" if runtime_root else "codex-code-mode-host"),
        "codex-path/rg": extension_bin / "codex-path" / "rg",
        "codex-resources/bwrap": extension_bin / "codex-resources" / "bwrap",
        "codex-package.json": extension_bin / "codex-package.json",
    }
    license_inputs = {
        "LICENSES/Hydex-Apache-2.0.txt": repo / "LICENSE",
        "LICENSES/bubblewrap-LGPL-2.0-or-later.txt": (
            repo / "codex-rs" / "vendor" / "bubblewrap" / "COPYING"
        ),
        "LICENSES/ripgrep-MIT.txt": (
            repo / "packaging" / "release" / "licenses" / "ripgrep" / "LICENSE-MIT"
        ),
        "LICENSES/ripgrep-UNLICENSE.txt": (
            repo / "packaging" / "release" / "licenses" / "ripgrep" / "UNLICENSE"
        ),
    }
    for path in [*inputs.values(), *license_inputs.values()]:
        require_file(path)
    for key in [
        "bin/codex",
        "bin/codex-code-mode-host",
        "codex-path/rg",
        "codex-resources/bwrap",
    ]:
        require_static_pie(inputs[key], repo)

    metadata = json.loads(inputs["codex-package.json"].read_text())
    version = metadata.get("version")
    if not isinstance(version, str) or CODEX_VERSION_RE.fullmatch(version) is None:
        raise SystemExit(f"invalid codex-package.json version: {version!r}")
    if metadata.get("target") != TARGET:
        raise SystemExit(f"runtime target is not {TARGET}: {metadata.get('target')!r}")
    if metadata.get("variant") != "codex" or metadata.get("entrypoint") != "bin/codex":
        raise SystemExit("runtime package metadata is not the canonical Codex layout")

    codex_output = run_text([str(inputs["bin/codex"]), "--version"], repo)
    if f"codex-cli {version}" not in codex_output.splitlines():
        raise SystemExit(
            f"bundled Hydex version does not match {version}:\n{codex_output}"
        )
    help_output = run_text([str(inputs["bin/codex"]), "--help"], repo)
    if "--offload" not in help_output or "--no-offload" not in help_output:
        raise SystemExit("bundled Hydex CLI help is missing offload flags")
    rg_output = run_text([str(inputs["codex-path/rg"]), "--version"], repo)
    rg_match = re.search(r"^ripgrep ([^ ]+)", rg_output, re.MULTILINE)
    if rg_match is None:
        raise SystemExit("could not determine the bundled ripgrep version")
    run_text([str(inputs["codex-resources/bwrap"]), "--version"], repo)

    output_dir = args.output_dir.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    root_name = f"hydex-runtime-{version}-r{args.release}-{TARGET}"
    archive_path = output_dir / f"{root_name}.tar.gz"
    checksum_path = output_dir / f"{archive_path.name}.sha256"
    release_tag = f"hydex-runtime-v{version}-r{args.release}"

    with tempfile.TemporaryDirectory(prefix=".runtime-bundle-", dir=output_dir) as temp:
        stage_root = Path(temp) / root_name
        staged_files: dict[str, Path] = {}
        for relative, source in inputs.items():
            destination = stage_root / relative
            mode = 0o755 if relative != "codex-package.json" else 0o644
            copy_file(source, destination, mode)
            staged_files[relative] = destination
        for relative, source in license_inputs.items():
            destination = stage_root / relative
            copy_file(source, destination, 0o644)
            staged_files[relative] = destination

        manifest = {
            "schema_version": 1,
            "artifact": {
                "name": "hydex-runtime",
                "version": version,
                "release": int(args.release),
                "target": TARGET,
                "variant": "codex",
                "entrypoint": "bin/codex",
            },
            "provenance": {
                "hydex_commit": hydex_commit,
                "packaging_commit": packaging_commit,
                "source_surface": surface_metadata.get("surface", "plugin"),
                "surface_label": surface_metadata.get("sourceLabel", args.baseline),
                "plugin_baseline": args.baseline,
                "extension_version": (
                    baseline_match.group("version")
                    if baseline_match is not None
                    else None
                ),
                "source_date_epoch": source_date_epoch,
                "release_tag": release_tag,
            },
            "sources": {
                "hydex": f"https://github.com/mwheiss/hydex/tree/{hydex_commit}",
                "bubblewrap": (
                    "https://github.com/mwheiss/hydex/tree/"
                    f"{hydex_commit}/codex-rs/vendor/bubblewrap"
                ),
                "ripgrep": f"https://github.com/BurntSushi/ripgrep/tree/{rg_match.group(1)}",
            },
            "licenses": {
                "hydex": "Apache-2.0",
                "bubblewrap": "LGPL-2.0-or-later",
                "ripgrep": "MIT OR Unlicense",
            },
            "files": [
                {
                    "path": relative,
                    "sha256": sha256(path),
                    "mode": "0755" if os.access(path, os.X_OK) else "0644",
                }
                for relative, path in sorted(staged_files.items())
            ],
        }
        manifest_path = stage_root / "manifest.json"
        manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
        manifest_path.chmod(0o644)
        staged_files["manifest.json"] = manifest_path

        sums_path = stage_root / "SHA256SUMS"
        sums_path.write_text(
            "".join(
                f"{sha256(path)}  {relative}\n"
                for relative, path in sorted(staged_files.items())
            )
        )
        sums_path.chmod(0o644)
        write_deterministic_archive(
            stage_root, archive_path, root_name, source_date_epoch
        )

    archive_hash = sha256(archive_path)
    checksum_path.write_text(f"{archive_hash}  {archive_path.name}\n")
    summary = {
        "release_tag": release_tag,
        "version": version,
        "release": args.release,
        "target": TARGET,
        "hydex_commit": hydex_commit,
        "packaging_commit": packaging_commit,
        "source_surface": surface_metadata.get("surface", "plugin"),
        "surface_label": surface_metadata.get("sourceLabel", args.baseline),
        "plugin_baseline": args.baseline,
        "archive": str(archive_path),
        "archive_sha256": archive_hash,
        "checksum": str(checksum_path),
        "codex_sha256": sha256(inputs["bin/codex"]),
        "code_mode_host_sha256": sha256(inputs["bin/codex-code-mode-host"]),
        "ripgrep_sha256": sha256(inputs["codex-path/rg"]),
        "bwrap_sha256": sha256(inputs["codex-resources/bwrap"]),
    }
    return archive_path, checksum_path, summary


def parse_args() -> argparse.Namespace:
    script_dir = Path(__file__).resolve().parent
    repo = script_dir.parent.parent
    parser = argparse.ArgumentParser(
        description="Build a deterministic public Hydex Linux x64 runtime bundle"
    )
    parser.add_argument("--repo", type=Path, default=repo)
    parser.add_argument("--plugin-dir", type=Path, default=repo / "hydex-plugin")
    parser.add_argument("--baseline")
    parser.add_argument("--runtime-root", type=Path)
    parser.add_argument("--release", required=True)
    parser.add_argument("--hydex-commit", default="HEAD")
    parser.add_argument("--output-dir", type=Path, default=script_dir / "dist")
    parser.add_argument("--allow-dirty", action="store_true")
    return parser.parse_args()


def main() -> int:
    _, _, summary = build_bundle(parse_args())
    print("HYDEX_RUNTIME_BUNDLE_SUMMARY")
    for key, value in summary.items():
        print(f"{key}={value}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
