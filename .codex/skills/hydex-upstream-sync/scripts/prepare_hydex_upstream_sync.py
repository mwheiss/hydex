#!/usr/bin/env python3
"""Prepare a Hydex patch-stack transplant onto current upstream main."""

import argparse
import datetime as dt
import pathlib
import subprocess
import sys
import tempfile


def run(
    args: list[str],
    *,
    cwd: pathlib.Path,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    print("+", " ".join(args), flush=True)
    return subprocess.run(args, cwd=cwd, text=True, check=check)


def capture(args: list[str], *, cwd: pathlib.Path) -> str:
    print("+", " ".join(args), flush=True)
    return subprocess.check_output(args, cwd=cwd, text=True).strip()


def ensure_clean_enough(repo: pathlib.Path, allow_untracked: bool) -> None:
    status = capture(["git", "status", "--porcelain"], cwd=repo)
    if not status:
        return

    tracked_dirty = [line for line in status.splitlines() if not line.startswith("?? ")]
    if tracked_dirty:
        print("Tracked worktree changes are present; commit/stash them first:", file=sys.stderr)
        print("\n".join(tracked_dirty), file=sys.stderr)
        raise SystemExit(2)

    if not allow_untracked:
        print("Untracked files are present; rerun with --allow-untracked or clean them up:", file=sys.stderr)
        print(status, file=sys.stderr)
        raise SystemExit(2)

    print("Leaving untracked files untouched:")
    print(status)


def ref_exists(repo: pathlib.Path, ref: str) -> bool:
    return subprocess.run(
        ["git", "rev-parse", "--verify", "--quiet", ref],
        cwd=repo,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    ).returncode == 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Create a scratch branch from upstream and apply the Hydex delta with git apply --3way.",
    )
    parser.add_argument("--base-anchor", required=True, help="Previous upstream anchor SHA/ref.")
    parser.add_argument("--hydex-branch", default="hydex/main", help="Hydex branch/ref to replay.")
    parser.add_argument("--upstream", default="origin/main", help="Current upstream base ref.")
    parser.add_argument("--scratch-branch", help="Scratch branch to create from upstream.")
    parser.add_argument("--remote", default="origin", help="Remote to fetch when --fetch is used.")
    parser.add_argument("--fetch", action="store_true", help="Fetch the remote before preparing.")
    parser.add_argument("--allow-untracked", action="store_true", help="Allow unrelated untracked files.")
    parser.add_argument("--patch-out", help="Where to write the generated binary patch.")
    args = parser.parse_args()

    repo = pathlib.Path(capture(["git", "rev-parse", "--show-toplevel"], cwd=pathlib.Path.cwd()))
    ensure_clean_enough(repo, args.allow_untracked)

    run(["git", "config", "rerere.enabled", "true"], cwd=repo)
    run(["git", "config", "rerere.autoupdate", "true"], cwd=repo)

    if args.fetch:
        run(["git", "fetch", args.remote], cwd=repo)

    for ref in (args.base_anchor, args.hydex_branch, args.upstream):
        if not ref_exists(repo, ref):
            print(f"Required ref does not exist: {ref}", file=sys.stderr)
            return 2

    scratch_branch = args.scratch_branch
    if not scratch_branch:
        stamp = dt.datetime.now(dt.UTC).strftime("%Y%m%d-%H%M%S")
        scratch_branch = f"hydex/rebase-apply-{stamp}"

    if ref_exists(repo, f"refs/heads/{scratch_branch}"):
        print(f"Scratch branch already exists: {scratch_branch}", file=sys.stderr)
        return 2

    patch_path = (
        pathlib.Path(args.patch_out)
        if args.patch_out
        else pathlib.Path(tempfile.gettempdir()) / "hydex-main-delta.patch"
    )
    patch_path.parent.mkdir(parents=True, exist_ok=True)
    print(f"Writing patch: {patch_path}")
    with patch_path.open("wb") as patch_file:
        subprocess.run(
            ["git", "diff", "--binary", f"{args.base_anchor}..{args.hydex_branch}"],
            cwd=repo,
            stdout=patch_file,
            check=True,
        )

    run(["git", "switch", "-c", scratch_branch, args.upstream], cwd=repo)
    apply_result = run(["git", "apply", "--3way", str(patch_path)], cwd=repo, check=False)

    if apply_result.returncode == 0:
        print("Patch applied cleanly. Run formatting/tests, then commit the scratch branch.")
        return 0

    print(
        "Patch applied with conflicts or failed. Resolve conflicts, stage files, run validation, then commit.",
        file=sys.stderr,
    )
    return apply_result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
