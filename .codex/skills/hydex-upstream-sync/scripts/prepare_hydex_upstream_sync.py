#!/usr/bin/env python3
"""Prepare an isolated Hydex replay onto a selected upstream base."""

import argparse
import datetime as dt
import pathlib
import subprocess
import sys
import tempfile


GENERATED_REPLAY_PATHS = (
    "codex-rs/app-server-protocol/schema/",
    "codex-rs/app-server-protocol/src/schema_fixtures.rs",
    "codex-rs/core/config.schema.json",
)
HYDEX_README_PATH = "README.md"
UPSTREAM_README_MIRROR_PATH = "README-codex.md"


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


def capture_lines(args: list[str], *, cwd: pathlib.Path) -> list[str]:
    output = capture(args, cwd=cwd)
    return output.splitlines() if output else []


def ref_exists(repo: pathlib.Path, ref: str) -> bool:
    return subprocess.run(
        ["git", "rev-parse", "--verify", "--quiet", ref],
        cwd=repo,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    ).returncode == 0


def resolve_commit(repo: pathlib.Path, ref: str) -> str:
    return capture(["git", "rev-parse", f"{ref}^{{commit}}"], cwd=repo)


def changed_paths(repo: pathlib.Path, old: str, new: str) -> set[str]:
    return set(capture_lines(["git", "diff", "--name-only", f"{old}..{new}"], cwd=repo))


def print_preflight(
    repo: pathlib.Path,
    *,
    base_sha: str,
    hydex_sha: str,
    upstream_sha: str,
) -> None:
    hydex_paths = changed_paths(repo, base_sha, hydex_sha)
    upstream_paths = changed_paths(repo, base_sha, upstream_sha)
    overlap = sorted(hydex_paths & upstream_paths)
    print("HYDEX_REPLAY_PREFLIGHT")
    print(f"hydex_changed_files={len(hydex_paths)}")
    print(f"upstream_changed_files={len(upstream_paths)}")
    print(f"overlapping_changed_files={len(overlap)}")
    for path in overlap:
        print(f"overlap={path}")
    merge_base = capture(["git", "merge-base", hydex_sha, upstream_sha], cwd=repo)
    if merge_base != base_sha:
        print("predicted_conflicts=unavailable_nonmatching_merge_base")
        return
    merge_result = subprocess.run(
        ["git", "merge-tree", "--write-tree", "--messages", upstream_sha, hydex_sha],
        cwd=repo,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    conflict_lines = [
        line for line in merge_result.stdout.splitlines() if line.startswith("CONFLICT ")
    ]
    print(f"predicted_conflicts={len(conflict_lines)}")
    for line in conflict_lines:
        print(f"predicted_conflict={line}")


def is_generated_replay_path(path: str) -> bool:
    return any(path == prefix or path.startswith(prefix) for prefix in GENERATED_REPLAY_PATHS)


def ref_has_path(repo: pathlib.Path, ref: str, path: str) -> bool:
    return subprocess.run(
        ["git", "cat-file", "-e", f"{ref}:{path}"],
        cwd=repo,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    ).returncode == 0


def restore_path_from_ref(
    repo: pathlib.Path,
    worktree: pathlib.Path,
    *,
    ref: str,
    path: str,
) -> None:
    destination = worktree / path
    if ref_has_path(repo, ref, path):
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(
            subprocess.check_output(["git", "show", f"{ref}:{path}"], cwd=repo)
        )
        run(["git", "add", "--", path], cwd=worktree)
    else:
        run(["git", "rm", "--ignore-unmatch", "--", path], cwd=worktree)


def write_path_from_ref(
    repo: pathlib.Path,
    worktree: pathlib.Path,
    *,
    ref: str,
    source_path: str,
    destination_path: str,
) -> None:
    destination = worktree / destination_path
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_bytes(
        subprocess.check_output(["git", "show", f"{ref}:{source_path}"], cwd=repo)
    )
    run(["git", "add", "--", destination_path], cwd=worktree)


def unmerged_paths(worktree: pathlib.Path) -> list[str]:
    return capture_lines(
        ["git", "diff", "--name-only", "--diff-filter=U"],
        cwd=worktree,
    )


def apply_managed_conflict_policy(
    repo: pathlib.Path,
    worktree: pathlib.Path,
    *,
    hydex_sha: str,
    upstream_sha: str,
) -> list[str]:
    for path in unmerged_paths(worktree):
        if path == HYDEX_README_PATH:
            print(f"Resolving Hydex-owned {path} from {hydex_sha}")
            restore_path_from_ref(repo, worktree, ref=hydex_sha, path=path)
        elif path == UPSTREAM_README_MIRROR_PATH:
            print(f"Resolving upstream mirror {path} from {upstream_sha}")
            write_path_from_ref(
                repo,
                worktree,
                ref=upstream_sha,
                source_path=HYDEX_README_PATH,
                destination_path=UPSTREAM_README_MIRROR_PATH,
            )
        elif is_generated_replay_path(path):
            print(f"Resolving generated output {path} from {upstream_sha}; regenerate after replay")
            restore_path_from_ref(repo, worktree, ref=upstream_sha, path=path)
    return unmerged_paths(worktree)


def refresh_upstream_readme_mirror(
    repo: pathlib.Path,
    worktree: pathlib.Path,
    *,
    upstream_sha: str,
) -> None:
    if not ref_has_path(repo, upstream_sha, HYDEX_README_PATH):
        raise SystemExit(f"upstream commit has no {HYDEX_README_PATH}: {upstream_sha}")
    write_path_from_ref(
        repo,
        worktree,
        ref=upstream_sha,
        source_path=HYDEX_README_PATH,
        destination_path=UPSTREAM_README_MIRROR_PATH,
    )
    if capture_lines(
        ["git", "status", "--porcelain", "--", UPSTREAM_README_MIRROR_PATH],
        cwd=worktree,
    ):
        print(f"Refreshed {UPSTREAM_README_MIRROR_PATH} from {upstream_sha}; commit after validation")


def rebase_in_progress(worktree: pathlib.Path) -> bool:
    for state_dir in ("rebase-merge", "rebase-apply"):
        path = pathlib.Path(capture(["git", "rev-parse", "--git-path", state_dir], cwd=worktree))
        if path.exists():
            return True
    return False


def continue_rebase(
    repo: pathlib.Path,
    worktree: pathlib.Path,
    *,
    hydex_sha: str,
    upstream_sha: str,
    run_continue_first: bool,
) -> int:
    if not rebase_in_progress(worktree):
        print(f"no rebase is in progress in {worktree}", file=sys.stderr)
        return 2
    should_continue = run_continue_first
    while True:
        unresolved = apply_managed_conflict_policy(
            repo,
            worktree,
            hydex_sha=hydex_sha,
            upstream_sha=upstream_sha,
        )
        if unresolved:
            print("Manual conflicts remain:", file=sys.stderr)
            for path in unresolved:
                print(f"  {path}", file=sys.stderr)
            print(f"Replay worktree: {worktree}", file=sys.stderr)
            return 1

        if not should_continue:
            return 0
        result = run(
            ["git", "-c", "core.editor=true", "rebase", "--continue"],
            cwd=worktree,
            check=False,
        )
        if result.returncode == 0:
            return 0
        if not rebase_in_progress(worktree):
            print("rebase continuation failed without leaving a resumable rebase", file=sys.stderr)
            return result.returncode
        should_continue = True


def create_worktree(
    repo: pathlib.Path,
    *,
    scratch_branch: str,
    start_ref: str,
    worktree: pathlib.Path,
) -> None:
    if ref_exists(repo, f"refs/heads/{scratch_branch}"):
        raise SystemExit(f"scratch branch already exists: {scratch_branch}")
    if worktree.exists():
        raise SystemExit(f"replay worktree already exists: {worktree}")
    run(["git", "branch", scratch_branch, start_ref], cwd=repo)
    run(["git", "worktree", "add", str(worktree), scratch_branch], cwd=repo)


def replay_commits(
    repo: pathlib.Path,
    worktree: pathlib.Path,
    *,
    scratch_branch: str,
    base_sha: str,
    hydex_sha: str,
    upstream_sha: str,
) -> int:
    result = run(
        ["git", "rebase", "--onto", upstream_sha, base_sha, scratch_branch],
        cwd=worktree,
        check=False,
    )
    if result.returncode != 0:
        if not rebase_in_progress(worktree):
            return result.returncode
        result_code = continue_rebase(
            repo,
            worktree,
            hydex_sha=hydex_sha,
            upstream_sha=upstream_sha,
            run_continue_first=True,
        )
        if result_code != 0:
            return result_code
    refresh_upstream_readme_mirror(repo, worktree, upstream_sha=upstream_sha)
    print("Commit-preserving replay completed. Regenerate outputs, validate, and commit any refreshes.")
    print(f"Replay worktree: {worktree}")
    return 0


def replay_aggregate_patch(
    repo: pathlib.Path,
    worktree: pathlib.Path,
    *,
    base_sha: str,
    hydex_sha: str,
    patch_path: pathlib.Path,
) -> int:
    patch_path.parent.mkdir(parents=True, exist_ok=True)
    print(f"Writing aggregate fallback patch: {patch_path}")
    with patch_path.open("wb") as patch_file:
        subprocess.run(
            ["git", "diff", "--binary", f"{base_sha}..{hydex_sha}"],
            cwd=repo,
            stdout=patch_file,
            check=True,
        )
    result = run(["git", "apply", "--3way", str(patch_path)], cwd=worktree, check=False)
    if result.returncode == 0:
        print("Aggregate fallback applied. Validate and commit the scratch branch.")
    else:
        print(f"Aggregate fallback needs manual resolution in {worktree}", file=sys.stderr)
    return result.returncode


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Replay Hydex onto an exact upstream base in an isolated worktree.",
    )
    parser.add_argument("--base-anchor", default="origin/main")
    parser.add_argument("--hydex-branch", default="hydex/main")
    parser.add_argument("--upstream", default="openai/main")
    parser.add_argument("--scratch-branch")
    parser.add_argument("--worktree", type=pathlib.Path)
    parser.add_argument("--mode", choices=("commits", "aggregate"), default="commits")
    parser.add_argument("--patch-out")
    parser.add_argument("--preflight-only", action="store_true")
    parser.add_argument("--continue-worktree", type=pathlib.Path)
    parser.add_argument(
        "--allow-untracked",
        action="store_true",
        help="Deprecated compatibility flag; isolated worktrees ignore the caller's untracked files.",
    )
    args = parser.parse_args()

    repo = pathlib.Path(capture(["git", "rev-parse", "--show-toplevel"], cwd=pathlib.Path.cwd()))
    run(["git", "config", "rerere.enabled", "true"], cwd=repo)
    run(["git", "config", "rerere.autoupdate", "true"], cwd=repo)

    for ref in (args.base_anchor, args.hydex_branch, args.upstream):
        if not ref_exists(repo, ref):
            print(f"required ref does not exist: {ref}", file=sys.stderr)
            return 2

    base_sha = resolve_commit(repo, args.base_anchor)
    hydex_sha = resolve_commit(repo, args.hydex_branch)
    upstream_sha = resolve_commit(repo, args.upstream)
    if subprocess.run(
        ["git", "merge-base", "--is-ancestor", base_sha, hydex_sha],
        cwd=repo,
        check=False,
    ).returncode != 0:
        print(
            f"Hydex base contract violated: {args.base_anchor} ({base_sha}) is not an ancestor "
            f"of {args.hydex_branch} ({hydex_sha}).",
            file=sys.stderr,
        )
        return 2

    print(f"Hydex base: {args.base_anchor} -> {base_sha}")
    print(f"Hydex tip: {args.hydex_branch} -> {hydex_sha}")
    print(f"Replay target: {args.upstream} -> {upstream_sha}")
    print_preflight(repo, base_sha=base_sha, hydex_sha=hydex_sha, upstream_sha=upstream_sha)
    if args.preflight_only:
        return 0

    stamp = dt.datetime.now(dt.UTC).strftime("%Y%m%d-%H%M%S")
    scratch_branch = args.scratch_branch or f"hydex/rebase-{stamp}"
    worktree = (
        args.continue_worktree
        or args.worktree
        or pathlib.Path(tempfile.gettempdir()) / scratch_branch.replace("/", "-")
    ).resolve()

    if args.continue_worktree is not None:
        result = continue_rebase(
            repo,
            worktree,
            hydex_sha=hydex_sha,
            upstream_sha=upstream_sha,
            run_continue_first=True,
        )
        if result == 0:
            refresh_upstream_readme_mirror(repo, worktree, upstream_sha=upstream_sha)
        return result

    start_ref = hydex_sha if args.mode == "commits" else upstream_sha
    create_worktree(
        repo,
        scratch_branch=scratch_branch,
        start_ref=start_ref,
        worktree=worktree,
    )
    if args.mode == "commits":
        return replay_commits(
            repo,
            worktree,
            scratch_branch=scratch_branch,
            base_sha=base_sha,
            hydex_sha=hydex_sha,
            upstream_sha=upstream_sha,
        )

    patch_path = (
        pathlib.Path(args.patch_out)
        if args.patch_out
        else pathlib.Path(tempfile.gettempdir()) / "hydex-main-delta.patch"
    )
    return replay_aggregate_patch(
        repo,
        worktree,
        base_sha=base_sha,
        hydex_sha=hydex_sha,
        patch_path=patch_path,
    )


if __name__ == "__main__":
    raise SystemExit(main())
