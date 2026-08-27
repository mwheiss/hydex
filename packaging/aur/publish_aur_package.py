#!/usr/bin/env python3

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import tarfile
import tempfile
from pathlib import Path

AUR_URL = "ssh://aur@aur.archlinux.org/hydex-bin.git"
PLACEHOLDERS = {
    "@PKGVER@",
    "@PKGREL@",
    "@RUNTIME_VERSION@",
    "@RELEASE_TAG@",
    "@ARCHIVE_SHA256@",
}


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def run(
    command: list[str],
    cwd: Path,
    *,
    check: bool = True,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        command,
        cwd=cwd,
        check=False,
        capture_output=True,
        env=env,
        text=True,
    )
    if check and result.returncode != 0:
        rendered = " ".join(command)
        output = result.stdout + result.stderr
        raise SystemExit(f"command failed ({result.returncode}): {rendered}\n{output}")
    return result


def read_manifest(archive: Path) -> dict[str, object]:
    with tarfile.open(archive, mode="r:gz") as bundle:
        members = [
            member
            for member in bundle.getmembers()
            if member.isfile() and member.name.endswith("/manifest.json")
        ]
        if len(members) != 1:
            raise SystemExit(
                f"expected one runtime manifest in {archive}, found {len(members)}"
            )
        handle = bundle.extractfile(members[0])
        if handle is None:
            raise SystemExit(f"could not read runtime manifest from {archive}")
        manifest = json.load(handle)
    if not isinstance(manifest, dict) or manifest.get("schema_version") != 1:
        raise SystemExit("runtime manifest is missing schema_version=1")
    return manifest


def nested(mapping: dict[str, object], *keys: str) -> object:
    value: object = mapping
    for key in keys:
        if not isinstance(value, dict) or key not in value:
            raise SystemExit(f"runtime manifest is missing {'.'.join(keys)}")
        value = value[key]
    return value


def verify_archive(archive: Path, checksum: Path) -> tuple[dict[str, object], str]:
    if not archive.is_file() or not checksum.is_file():
        raise SystemExit("runtime archive or adjacent checksum is missing")
    parts = checksum.read_text().strip().split(maxsplit=1)
    actual = sha256(archive)
    if len(parts) != 2 or parts[1].lstrip("*") != archive.name or parts[0] != actual:
        raise SystemExit("runtime archive does not match its adjacent checksum")
    manifest = read_manifest(archive)
    expected_name = (
        f"hydex-runtime-{nested(manifest, 'artifact', 'version')}-"
        f"r{nested(manifest, 'artifact', 'release')}-"
        f"{nested(manifest, 'artifact', 'target')}.tar.gz"
    )
    if archive.name != expected_name:
        raise SystemExit(
            f"runtime archive name is {archive.name}, expected {expected_name}"
        )
    return manifest, actual


def render_pkgbuild(
    template: str, manifest: dict[str, object], archive_hash: str
) -> str:
    version = nested(manifest, "artifact", "version")
    release = nested(manifest, "artifact", "release")
    tag = nested(manifest, "provenance", "release_tag")
    if (
        not isinstance(version, str)
        or not isinstance(release, int)
        or not isinstance(tag, str)
    ):
        raise SystemExit("runtime manifest has invalid version, release, or tag")
    values = {
        "@PKGVER@": version.replace("-", "."),
        "@PKGREL@": str(release),
        "@RUNTIME_VERSION@": version,
        "@RELEASE_TAG@": tag,
        "@ARCHIVE_SHA256@": archive_hash,
    }
    rendered = template
    for placeholder, value in values.items():
        rendered = rendered.replace(placeholder, value)
    remaining = sorted(
        placeholder for placeholder in PLACEHOLDERS if placeholder in rendered
    )
    if remaining:
        raise SystemExit(f"unresolved PKGBUILD placeholders: {remaining}")
    return rendered


def render_package(args: argparse.Namespace, output: Path) -> dict[str, str]:
    archive = args.archive.resolve()
    checksum = args.checksum.resolve()
    manifest, archive_hash = verify_archive(archive, checksum)
    template = args.template.resolve().read_text()
    pkgbuild = render_pkgbuild(template, manifest, archive_hash)
    output.mkdir(parents=True, exist_ok=True)
    (output / "PKGBUILD").write_text(pkgbuild)
    shutil.copyfile(args.license.resolve(), output / "LICENSE")
    srcinfo = run(["makepkg", "--printsrcinfo"], output).stdout
    if not srcinfo.strip():
        raise SystemExit("makepkg generated an empty .SRCINFO")
    (output / ".SRCINFO").write_text(srcinfo)
    version = nested(manifest, "artifact", "version")
    release = nested(manifest, "artifact", "release")
    tag = nested(manifest, "provenance", "release_tag")
    return {
        "pkgver": str(version).replace("-", "."),
        "pkgrel": str(release),
        "release_tag": str(tag),
        "archive_sha256": archive_hash,
        "source_url": (
            f"https://github.com/mwheiss/hydex/releases/download/{tag}/{archive.name}"
        ),
    }


def validate_package(output: Path, build: bool) -> Path | None:
    run(["makepkg", "--verifysource", "--force"], output)
    if not build:
        return None
    run(["makepkg", "--clean", "--cleanbuild", "--force"], output)
    packages = [
        Path(path)
        for path in run(["makepkg", "--packagelist"], output).stdout.splitlines()
    ]
    if len(packages) != 1 or not packages[0].is_file():
        raise SystemExit(f"expected one built package, found {packages}")
    if shutil.which("namcap"):
        namcap_tmp = output / ".namcap-tmp"
        namcap_tmp.mkdir(exist_ok=True)
        namcap_env = os.environ.copy()
        namcap_env["TMPDIR"] = str(namcap_tmp)
        result = run(
            ["namcap", "PKGBUILD", str(packages[0])],
            output,
            check=False,
            env=namcap_env,
        )
        namcap_output = result.stdout + result.stderr
        print(namcap_output, end="")
        if result.returncode != 0 or any(
            " E:" in line or line.startswith("Error:")
            for line in namcap_output.splitlines()
        ):
            raise SystemExit("namcap reported a package error")
    return packages[0]


def publish_package(
    args: argparse.Namespace, rendered: Path, summary: dict[str, str]
) -> str:
    temp_parent = Path(os.environ.get("TMPDIR", tempfile.gettempdir())).resolve()
    temp_parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="hydex-aur-", dir=temp_parent) as temp:
        checkout = Path(temp) / "hydex-bin"
        run(["git", "clone", args.aur_url, str(checkout)], args.repo.resolve())
        for name in ["PKGBUILD", ".SRCINFO", "LICENSE"]:
            shutil.copyfile(rendered / name, checkout / name)
        run(["git", "config", "user.name", "Michael W. Heiss"], checkout)
        run(
            ["git", "config", "user.email", "mheiss@users.noreply.github.com"], checkout
        )
        run(["git", "add", "PKGBUILD", ".SRCINFO", "LICENSE"], checkout)
        run(["git", "diff", "--cached", "--check"], checkout)
        staged = run(["git", "diff", "--cached", "--quiet"], checkout, check=False)
        if staged.returncode == 0:
            return "unchanged"
        run(
            [
                "git",
                "commit",
                "-m",
                f"Update hydex-bin to {summary['pkgver']}-{summary['pkgrel']}",
            ],
            checkout,
        )
        run(["git", "push", "origin", "master"], checkout)
        run(["git", "fetch", "origin"], checkout)
        local = run(["git", "rev-parse", "HEAD"], checkout).stdout.strip()
        remote = run(["git", "rev-parse", "origin/master"], checkout).stdout.strip()
        if local != remote:
            raise SystemExit(f"AUR push readback mismatch: {local} != {remote}")
        return local


def parse_args() -> argparse.Namespace:
    script_dir = Path(__file__).resolve().parent
    repo = script_dir.parent.parent
    parser = argparse.ArgumentParser(
        description="Render, validate, and optionally publish the hydex-bin AUR package"
    )
    parser.add_argument("--repo", type=Path, default=repo)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--checksum", type=Path, required=True)
    parser.add_argument("--template", type=Path, default=script_dir / "PKGBUILD.in")
    parser.add_argument("--license", type=Path, default=script_dir / "LICENSE")
    parser.add_argument(
        "--output-dir", type=Path, default=script_dir / "dist" / "hydex-bin"
    )
    parser.add_argument("--aur-url", default=AUR_URL)
    parser.add_argument("--build", action="store_true")
    parser.add_argument("--publish", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    output = args.output_dir.resolve()
    summary = render_package(args, output)
    package = validate_package(output, args.build)
    publication = (
        publish_package(args, output, summary) if args.publish else "not_requested"
    )
    print("HYDEX_AUR_PACKAGE_SUMMARY")
    for key, value in summary.items():
        print(f"{key}={value}")
    print(f"rendered_dir={output}")
    print(f"package={package or ''}")
    print(f"publication={publication}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
