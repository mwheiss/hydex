# Exact Hydex Replay

## 1. Inspect and fetch

```bash
cd /home/mheiss/hydex
git status --short --branch
git remote -v
git fetch origin
git remote get-url openai >/dev/null 2>&1 || \
  git remote add openai https://github.com/openai/codex.git
```

Leave unrelated untracked files and the nested plugin repository alone. Stop for overlapping
tracked edits.

For a surface release, resolve the exact Codex version before choosing any ref:

```bash
python3 .codex/skills/hydex-upstream-sync/scripts/resolve_plugin_codex_base.py \
  --version <version> --fetch-tag
```

For an intentional mainline sync, fetch `openai main` and use the exact fetched `openai/main` SHA.

## 2. Verify the old base pointer

```bash
BASE_ANCHOR=origin/main
test "$(git merge-base hydex/main "$BASE_ANCHOR")" = "$(git rev-parse "$BASE_ANCHOR")"
git log --oneline "$BASE_ANCHOR"..hydex/main
```

Keep `origin/main` unchanged until the replay is validated. A release tag may not share the ancestry
shape of current OpenAI main, so never infer the old base from a merge-base with the new target.

## 3. Prepare once in an isolated worktree

Choose a stable versioned scratch branch, for example
`hydex/rebase-plugin-rust-v<version>` or `hydex/rebase-desktop-rust-v<version>`. If both surfaces use
the same Codex version, reuse one replay rather than creating duplicate branches.

```bash
python3 .codex/skills/hydex-upstream-sync/scripts/prepare_hydex_upstream_sync.py \
  --base-anchor "$BASE_ANCHOR" \
  --hydex-branch hydex/main \
  --upstream <target-ref> \
  --scratch-branch "$SCRATCH" \
  --worktree "$REPLAY_WORKTREE"
```

Use `--preflight-only` when the overlap report is needed without creating a branch/worktree. Use
`--mode aggregate` only when commit-preserving replay is genuinely blocked and the fallback is
explicitly justified.

## 4. Continue conflicts safely

The helper automatically resolves only managed paths: Hydex `README.md`, upstream
`README-codex.md`, and generated schema outputs. Resolve remaining source conflicts manually in the
replay worktree, stage exact paths, then continue:

```bash
rg -n '<<<<<<<|=======|>>>>>>>' .
git add <exact-resolved-paths>
git diff --check
python3 /home/mheiss/hydex/.codex/skills/hydex-upstream-sync/scripts/prepare_hydex_upstream_sync.py \
  --continue-worktree "$REPLAY_WORKTREE"
```

Do not add new compatibility behavior merely to make a replay pass. Compare the new upstream
implementation and preserve the routing/security invariants from the skill entrypoint.

Proceed to [validation.md](validation.md). Do not advance canonical refs from an unvalidated replay.
