#!/usr/bin/env python3

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import tarfile
import tempfile
from pathlib import Path

CHROOTS = ("epel-7-x86_64", "rhel-9-x86_64", "rhel-10-x86_64")
PLACEHOLDERS = {
    "@RPM_VERSION@",
    "@RPM_RELEASE@",
    "@ARCHIVE_NAME@",
    "@RUNTIME_ROOT@",
    "@RELEASE_TAG@",
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


def render_spec(template: str, manifest: dict[str, object], archive: Path) -> str:
    version = nested(manifest, "artifact", "version")
    release = nested(manifest, "artifact", "release")
    target = nested(manifest, "artifact", "target")
    tag = nested(manifest, "provenance", "release_tag")
    if (
        not isinstance(version, str)
        or not isinstance(release, int)
        or not isinstance(target, str)
        or not isinstance(tag, str)
    ):
        raise SystemExit(
            "runtime manifest has invalid version, release, target, or tag"
        )
    values = {
        "@RPM_VERSION@": version.replace("-", "_"),
        "@RPM_RELEASE@": str(release),
        "@ARCHIVE_NAME@": archive.name,
        "@RUNTIME_ROOT@": archive.name.removesuffix(".tar.gz"),
        "@RELEASE_TAG@": tag,
    }
    rendered = template
    for placeholder, value in values.items():
        rendered = rendered.replace(placeholder, value)
    remaining = sorted(
        placeholder for placeholder in PLACEHOLDERS if placeholder in rendered
    )
    if remaining:
        raise SystemExit(f"unresolved spec placeholders: {remaining}")
    return rendered


def make_topdir(root: Path) -> None:
    for name in ["BUILD", "BUILDROOT", "RPMS", "SOURCES", "SPECS", "SRPMS", "tmp"]:
        (root / name).mkdir(parents=True, exist_ok=True)


def build_srpm(
    output: Path, archive: Path, rendered_spec: str, expected_version: str
) -> tuple[Path, Path]:
    output.mkdir(parents=True, exist_ok=True)
    rendered_path = output / "hydex.spec"
    rendered_path.write_text(rendered_spec)
    with tempfile.TemporaryDirectory(prefix="hydex-copr-srpm-", dir=output) as temp:
        topdir = Path(temp)
        make_topdir(topdir)
        source = topdir / "SOURCES" / archive.name
        try:
            os.link(archive, source)
        except OSError:
            shutil.copyfile(archive, source)
        spec = topdir / "SPECS" / "hydex.spec"
        spec.write_text(rendered_spec)
        run(
            [
                "rpmbuild",
                "--define",
                f"_topdir {topdir}",
                "--define",
                f"_tmppath {topdir / 'tmp'}",
                "-bs",
                str(spec),
            ],
            output,
        )
        packages = list((topdir / "SRPMS").glob("*.src.rpm"))
        if len(packages) != 1:
            raise SystemExit(f"expected one source RPM, found {packages}")
        destination = output / packages[0].name
        shutil.copyfile(packages[0], destination)
    if expected_version not in destination.name:
        raise SystemExit(f"source RPM has unexpected name: {destination.name}")
    return destination, rendered_path


def verify_rpm(path: Path, source: bool) -> str:
    verification_result = run(["rpm", "-Kv", str(path)], path.parent, check=False)
    verification = verification_result.stdout + verification_result.stderr
    if "Header SHA256 digest: OK" not in verification:
        raise SystemExit(f"RPM header digest did not verify:\n{verification}")
    if "Payload SHA256 digest: OK" not in verification:
        raise SystemExit(f"RPM payload digest did not verify:\n{verification}")
    query = run(
        [
            "rpm",
            "-qp",
            "--qf",
            "%{NAME} %{VERSION} %{RELEASE} %{ARCH} %{PAYLOADFORMAT} %{PAYLOADCOMPRESSOR}\\n",
            str(path),
        ],
        path.parent,
    ).stdout.strip()
    files = run(["rpm", "-qpl", str(path)], path.parent).stdout
    if source:
        if "hydex.spec" not in files or "hydex-runtime-" not in files:
            raise SystemExit("source RPM is missing its spec or runtime archive")
    elif "/usr/bin/codex" not in files or "/usr/libexec/hydex/" not in files:
        raise SystemExit("binary RPM is missing the canonical Hydex layout")
    if not source:
        requires = run(["rpm", "-qpR", str(path)], path.parent).stdout.splitlines()
        unexpected = [
            requirement
            for requirement in requires
            if not requirement.startswith("rpmlib(")
        ]
        if unexpected:
            raise SystemExit(
                f"binary RPM has unexpected runtime requirements: {unexpected}"
            )
        if ".el7" in query:
            if not query.endswith(" cpio gzip"):
                raise SystemExit(f"EL7 RPM does not use a cpio/gzip payload: {query}")
            if any(
                "LargeFiles" in item or "PayloadIsZstd" in item for item in requires
            ):
                raise SystemExit(
                    f"EL7 RPM uses unsupported RPM capabilities: {requires}"
                )
    return query


def build_local_rpm(output: Path, srpm: Path) -> tuple[Path, str]:
    with tempfile.TemporaryDirectory(prefix="hydex-copr-local-", dir=output) as temp:
        topdir = Path(temp)
        make_topdir(topdir)
        run(
            [
                "rpmbuild",
                "--define",
                f"_topdir {topdir}",
                "--define",
                f"_tmppath {topdir / 'tmp'}",
                "--define",
                "dist .el10",
                "--rebuild",
                str(srpm),
            ],
            output,
        )
        packages = list((topdir / "RPMS").rglob("*.rpm"))
        if len(packages) != 1:
            raise SystemExit(f"expected one local binary RPM, found {packages}")
        destination = output / packages[0].name
        shutil.copyfile(packages[0], destination)
    return destination, verify_rpm(destination, source=False)


def ensure_project(owner: str, project: str, repo: Path) -> str:
    full_name = f"{owner}/{project}"
    existing = run(["copr-cli", "get", full_name], repo, check=False)
    if existing.returncode == 0:
        for chroot in CHROOTS:
            run(
                [
                    "copr-cli",
                    "get-chroot",
                    f"{full_name}/{chroot}",
                    "--output-format",
                    "json",
                ],
                repo,
            )
        return "existing"
    command = ["copr-cli", "create", project]
    for chroot in CHROOTS:
        command.extend(["--chroot", chroot])
    command.extend(
        [
            "--description",
            "Hydex Codex CLI with optional local model offload",
            "--instructions",
            "Enable this COPR and install the hydex package.",
            "--enable-net",
            "off",
            "--appstream",
            "off",
            "--follow-fedora-branching",
            "off",
        ]
    )
    run(command, repo)
    run(["copr-cli", "get", full_name], repo)
    for chroot in CHROOTS:
        run(
            [
                "copr-cli",
                "get-chroot",
                f"{full_name}/{chroot}",
                "--output-format",
                "json",
            ],
            repo,
        )
    return "created"


def publish_srpm(owner: str, project: str, srpm: Path, repo: Path) -> str:
    full_name = f"{owner}/{project}"
    result = run(["copr-cli", "build", full_name, str(srpm)], repo)
    output = result.stdout + result.stderr
    print(output, end="")
    match = re.search(r"/coprs/build/(\d+)", output)
    if match is None:
        match = re.search(r"Created builds?:\s*([0-9]+)", output)
    return match.group(1) if match else "unknown"


def parse_args() -> argparse.Namespace:
    script_dir = Path(__file__).resolve().parent
    repo = script_dir.parent.parent
    parser = argparse.ArgumentParser(
        description="Build and optionally publish the Hydex COPR source RPM"
    )
    parser.add_argument("--repo", type=Path, default=repo)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--checksum", type=Path, required=True)
    parser.add_argument("--template", type=Path, default=script_dir / "hydex.spec.in")
    parser.add_argument("--output-dir", type=Path, default=script_dir / "dist")
    parser.add_argument("--owner")
    parser.add_argument("--project", default="hydex")
    parser.add_argument("--build-local", action="store_true")
    parser.add_argument("--publish", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    repo = args.repo.resolve()
    archive = args.archive.resolve()
    manifest, archive_hash = verify_archive(archive, args.checksum.resolve())
    version = nested(manifest, "artifact", "version")
    release = nested(manifest, "artifact", "release")
    if not isinstance(version, str) or not isinstance(release, int):
        raise SystemExit("runtime manifest has invalid version or release")
    rpm_version = version.replace("-", "_")
    rendered = render_spec(args.template.resolve().read_text(), manifest, archive)
    output = args.output_dir.resolve()
    srpm, spec = build_srpm(output, archive, rendered, rpm_version)
    srpm_query = verify_rpm(srpm, source=True)
    local_rpm = None
    local_query = ""
    if args.build_local:
        local_rpm, local_query = build_local_rpm(output, srpm)

    owner = args.owner
    project_status = "not_requested"
    build_id = "not_requested"
    if args.publish:
        if owner is None:
            owner = run(["copr-cli", "whoami"], repo).stdout.strip()
        if not owner:
            raise SystemExit("could not determine the COPR owner")
        project_status = ensure_project(owner, args.project, repo)
        build_id = publish_srpm(owner, args.project, srpm, repo)

    print("HYDEX_COPR_PACKAGE_SUMMARY")
    print(f"runtime_version={version}")
    print(f"rpm_version={rpm_version}")
    print(f"rpm_release={release}")
    print(f"archive_sha256={archive_hash}")
    print(f"spec={spec}")
    print(f"srpm={srpm}")
    print(f"srpm_sha256={sha256(srpm)}")
    print(f"srpm_query={srpm_query}")
    print(f"local_rpm={local_rpm or ''}")
    print(f"local_query={local_query}")
    print(f"copr_owner={owner or ''}")
    print(f"copr_project={args.project}")
    print(f"project_status={project_status}")
    print(f"build_id={build_id}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
