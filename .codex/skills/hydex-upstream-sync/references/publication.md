# Source Replay Publication

Read this only when root publication is explicitly authorized. Plugin or desktop publication does
not imply authority to move Hydex source refs.

## Publish provenance first

From the validated replay worktree, stage explicit paths, run `git diff --cached --check`, and make
any final focused commit. Never use broad `git add -A`.

```bash
NEW_HYDEX=$(git -C "$REPLAY_WORKTREE" rev-parse HEAD)
git -C "$REPLAY_WORKTREE" push -u origin "$SCRATCH"
```

Keep the versioned replay remote. It is the durable source for an older surface when VS Code and
Desktop versions diverge.

## Advance canonical refs only for the selected replay

The cross-surface coordinator decides which validated replay is newer and therefore canonical.
For that replay only:

```bash
OLD_MAIN=$(git rev-parse origin/main)
OLD_HYDEX=$(git rev-parse origin/hydex/main)
NEW_BASE=$(git rev-parse '<target-ref>^{commit}')
git push --atomic \
  --force-with-lease=refs/heads/main:"$OLD_MAIN" \
  --force-with-lease=refs/heads/hydex/main:"$OLD_HYDEX" \
  origin \
  "$NEW_BASE":refs/heads/main \
  "$NEW_HYDEX":refs/heads/hydex/main
```

Use the peeled tag commit, not an annotated tag object. Never blind-force either ref.

## Read back and align

```bash
git fetch origin
test "$(git rev-parse origin/main)" = "$NEW_BASE"
test "$(git rev-parse origin/hydex/main)" = "$NEW_HYDEX"
test "$(git merge-base origin/hydex/main origin/main)" = "$(git rev-parse origin/main)"
test "$(git rev-parse "origin/$SCRATCH")" = "$NEW_HYDEX"
```

If the clean main worktree is checked out on `hydex/main`, align it with
`git reset --keep origin/hydex/main`. Align local `main` only when it is not checked out in another
worktree. Never use `reset --hard`. Finish with `0 0` ahead/behind for the current published branch.

## Cleanup

Retain the active replay worktree and versioned remote branch for current surface provenance.
Remove only superseded, clean local replay worktrees whose exact `HEAD` equals a retained remote ref
or annotated tag. Compare exact SHAs; rebased histories are not reliably classified by `--merged`.
Deleting any remote replay ref requires separate explicit authority.
