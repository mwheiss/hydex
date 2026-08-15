---
name: hydex-upstream-sync
description: Sync the fork main branch with current openai/codex main or a Codex release tag bundled by the Hydex VS Code plugin, then replay hydex/main onto that upstream base while preserving Hydex local-offload behavior. Use when asked to rebase, replay, refresh, or bring Hydex in line with OpenAI/Codex main or a plugin-bundled Codex version.
---

# Hydex Upstream Sync

## Purpose

Replay the Hydex patch line onto current OpenAI Codex `main`, or onto the exact OpenAI Codex
release tag bundled by the current Hydex VS Code plugin.

The fork's `main` is the authoritative upstream-base pointer for the current `hydex/main` patch
line. It may point to an OpenAI release tag commit rather than current OpenAI `main`.
`openai/main` is the separately fetched current OpenAI-main reference.

Use an explicit `git rebase --onto <new-base> <old-base>` in an isolated worktree. This preserves
Hydex commit boundaries without assuming that two plugin release tags share a mainline ancestry.
The aggregate binary-patch transplant remains available as a fallback, but it is not the normal
workflow. Advance `main` only after the replay passes validation.

For plugin releases, prefer the tag-pinned workflow: update the upstream preview VSIX first, read
its bundled `codex-package.json` version, resolve the matching OpenAI tag `rust-v<version>`, and
replay Hydex only when that tag advances the underlying checkout. Reuse the existing matching
Hydex binary for a plugin-only refresh. This keeps the Rust code and extension bundle on the same
upstream Codex version without rebuilding unrelated packages.

## Plugin-Pinned Workflow

Use this when updating the Hydex VS Code plugin.

1. Refresh the plugin repo with the newest upstream preview VSIX:

   ```bash
   HYDEX_COMMIT_BEFORE=$(git rev-parse hydex/main)
   cd hydex-plugin
   python3 scripts/update_upstream_vsix.py
   git status --short --branch
   cd ..
   ```

   This updates `vendor/openai-chatgpt-<extension-version>-linux-x64.vsix`, unpacks it under
   `hydex-plugin/unpacked/`, and reports the bundled Codex package version.

2. Resolve the matching OpenAI Codex release tag:

   ```bash
   python3 .codex/skills/hydex-upstream-sync/scripts/resolve_plugin_codex_base.py \
     --plugin-dir hydex-plugin \
     --fetch-tag
   ```

   The script reads:

   ```text
   hydex-plugin/unpacked/<baseline>/extension/bin/linux-x86_64/codex-package.json
   ```

   and resolves `version = "X.Y.Z"` to `rust-vX.Y.Z`. Do not guess the tag from
   OpenAI `main`.

   Compare the peeled resolved tag commit with the current Hydex base pointer. If it is already
   the current base and no Hydex replay is required, set `hydex_checkout_updated=false`, skip steps
   3 through 5, and use the plugin-only path in step 6. A new extension version or VSIX digest is
   not an underlying Hydex checkout update.

3. Verify the current base-pointer contract and replay Hydex onto the new tag:

   ```bash
   git fetch origin
   BASE_ANCHOR=origin/main
   test "$(git merge-base hydex/main "$BASE_ANCHOR")" = "$(git rev-parse "$BASE_ANCHOR")"
   UPSTREAM_TAG=$(python3 .codex/skills/hydex-upstream-sync/scripts/resolve_plugin_codex_base.py \
     --plugin-dir hydex-plugin | awk -F= '/^upstream_tag=/{print $2}')
   UPSTREAM_SHA=$(git rev-parse "${UPSTREAM_TAG}^{commit}")
   SCRATCH=hydex/rebase-plugin-${UPSTREAM_TAG}
   REPLAY_WORKTREE=/tmp/${SCRATCH//\//-}
   python3 .codex/skills/hydex-upstream-sync/scripts/prepare_hydex_upstream_sync.py \
     --base-anchor "$BASE_ANCHOR" \
     --hydex-branch hydex/main \
     --upstream "$UPSTREAM_TAG" \
     --scratch-branch "$SCRATCH" \
     --worktree "$REPLAY_WORKTREE"
   ```

   `origin/main` remains at the old upstream base while the scratch replay is prepared and tested.
   This preserves the exact Hydex patch boundary. Do not replace `BASE_ANCHOR` with
   `git merge-base hydex/main openai/main`; a release-tag history can contain upstream commits that
   are not ancestors of the currently fetched OpenAI-main tip.

4. Resolve conflicts in `$REPLAY_WORKTREE` and validate as in the validation section below.
   The helper automatically takes generated schema outputs from the new upstream base and keeps
   the Hydex root README; regenerate schemas after source conflicts are resolved. To resume after
   manual conflict resolution:

   ```bash
   python3 .codex/skills/hydex-upstream-sync/scripts/prepare_hydex_upstream_sync.py \
     --base-anchor "$BASE_ANCHOR" \
     --hydex-branch hydex/main \
     --upstream "$UPSTREAM_TAG" \
     --continue-worktree "$REPLAY_WORKTREE"
   ```

5. Commit and push the scratch branch. Then atomically advance `main` to the new upstream tag
   commit and `hydex/main` to the validated replay:

   ```bash
   OLD_MAIN=$(git rev-parse origin/main)
   OLD_HYDEX=$(git rev-parse origin/hydex/main)
   NEW_HYDEX=$(git -C "$REPLAY_WORKTREE" rev-parse HEAD)
   git push -u origin "$SCRATCH"
   git push --atomic \
     --force-with-lease=refs/heads/main:"$OLD_MAIN" \
     --force-with-lease=refs/heads/hydex/main:"$OLD_HYDEX" \
     origin \
     "$UPSTREAM_SHA":refs/heads/main \
     "$NEW_HYDEX":refs/heads/hydex/main
   git fetch origin
   test "$(git merge-base origin/hydex/main origin/main)" = "$(git rev-parse origin/main)"
   ```

   Use the peeled tag commit from `git rev-parse "${UPSTREAM_TAG}^{commit}"`, not the annotated
   tag object. The explicit leases prevent overwriting concurrent remote updates.

   Set `hydex_checkout_updated=true` only after the validated replay advances `hydex/main` from
   `HYDEX_COMMIT_BEFORE` to a different commit. Record that resulting commit for the report.

6. Refresh the plugin according to whether the underlying Hydex checkout changed.

   When `hydex_checkout_updated=true`, rebuild and patch from the resulting Hydex commit, then
   build both matching system packages:

   ```bash
   cd hydex-plugin
   .codex/skills/hydex-plugin-refresh/scripts/refresh_hydex_plugin.py --repo ..
   git status --short --branch
   git add vendor metadata .codex/skills scripts README.md analysis
   git commit -m "Refresh Hydex plugin for Codex <version>"
   git push
   cd ..
   ./packaging/arch/build-local-package.sh
   ./packaging/rpm/build-rhel10-package.sh
   ```

   The plugin refresh script stamps the Hydex workspace version to the bundled
   `codex-package.json` version before building, then verifies that the bundled Hydex
   `codex --version` matches. The package helpers then build the matching pacman and RHEL 10
   packages from that refreshed Hydex source.

   When `hydex_checkout_updated=false`, patch and validate the new VSIX with the existing matching
   release binary, and do not build either package:

   ```bash
   cd hydex-plugin
   .codex/skills/hydex-plugin-refresh/scripts/refresh_hydex_plugin.py --repo .. --skip-build
   ```

   Report `hydex_checkout_updated=false` and that both packages were skipped. Omit all install and
   update instructions in this case, including pacman, dnf, local VS Code, and Remote-SSH
   instructions. Do not report stale package artifacts from an earlier refresh.

   Only when `hydex_checkout_updated=true`, or when the user explicitly requests package rebuilding,
   include the generated package paths and SHA-256 values followed by the exact update commands:

   ```bash
   sudo pacman -U /absolute/path/to/hydex-bin-<version>-1-x86_64.pkg.tar.zst
   sudo dnf install /absolute/path/to/hydex-<version>-1.el10.x86_64.rpm
   sudo dnf upgrade /absolute/path/to/hydex-<version>-1.el10.x86_64.rpm
   ```

   Do not run the sudo command automatically unless the user explicitly asks for installation.
   Pacman replaces the conflicting `openai-codex-bin` package; no separate removal is needed.

   In that same updated-checkout case, include the concrete VSIX path and these extension update
   reminders too:

   - Local Linux x64 VS Code:

     ```bash
     code --install-extension /absolute/path/to/hydex-chatgpt-<extension-version>-linux-x64.vsix --force
     code --list-extensions --show-versions | rg '^mwheiss\.hydex@'
     ```

   - Remote-SSH: connect to each host, then run `Extensions: Install from VSIX...` inside that
     connected remote window, select the generated Linux x64 VSIX, verify it under
     `SSH: <host> - Installed`, and run `Developer: Reload Window`.

   State explicitly that local and Remote-SSH extension hosts are separate and every SSH host must
   be updated independently. Do not present the ordinary local `code --install-extension` command
   as updating a remote extension host.

## Mainline Workflow

Use this when intentionally syncing Hydex to current OpenAI `main`, independent of the plugin.

1. Inspect repository state:

   ```bash
   git status --short --branch
   git remote -v
   git log --oneline --decorate -5
   ```

   Leave unrelated untracked files alone. The nested private plugin checkout `hydex-plugin/` is separate and must not be added to this repo.

2. Fetch the fork and OpenAI main:

   ```bash
   git fetch origin
   git remote get-url openai >/dev/null 2>&1 || \
     git remote add openai https://github.com/openai/codex.git
   git fetch openai main
   ```

3. Verify that the fork's `main` still identifies the current Hydex upstream base.

   Do this before moving either remote branch:

   ```bash
   BASE_ANCHOR=origin/main
   test "$(git merge-base hydex/main "$BASE_ANCHOR")" = "$(git rev-parse "$BASE_ANCHOR")"
   git log --oneline "$BASE_ANCHOR"..hydex/main
   ```

   `openai/main` is the new replay target. `origin/main` must remain at the old base until the
   Hydex delta has been generated and the replay has passed validation.

4. Enable rerere:

   ```bash
   git config rerere.enabled true
   git config rerere.autoupdate true
   ```

5. Use `origin/main` directly as the previous upstream anchor.

   Do not infer this anchor with a merge-base against the new OpenAI-main tip. The branch contract
   makes `origin/main` the exact old base even when the previous replay targeted a release branch.

6. Create a scratch replay branch and apply the Hydex delta:

   ```bash
   SCRATCH=hydex/rebase-apply-$(date -u +%Y%m%d-openai)
   REPLAY_WORKTREE=/tmp/${SCRATCH//\//-}
   python3 .codex/skills/hydex-upstream-sync/scripts/prepare_hydex_upstream_sync.py \
     --base-anchor "$BASE_ANCHOR" \
     --hydex-branch hydex/main \
     --upstream openai/main \
     --scratch-branch "$SCRATCH" \
     --worktree "$REPLAY_WORKTREE"
   ```

   If the scratch branch name already exists, use a unique suffix.

7. Resolve conflicts in `$REPLAY_WORKTREE`, if any, preserving Hydex invariants:

   - Primary/OpenAI/Codex routes keep upstream auth, account, attestation, Agent Identity, proxy, and control-plane behavior.
   - Local/offload routes never receive OpenAI/ChatGPT auth tokens, account headers, attestation, or Agent Identity headers.
   - Local transforms are wire-only; canonical history keeps namespace/name pairs.
   - Remote compaction v1/v2 stays primary unless local routing explicitly recovers/projects it first.
   - `web.run` stays executable through the primary Codex search endpoint even when model inference is local.
   - Memory routing changes only when Hydex offload config says so; vanilla/no-offload behavior stays upstream.

   After resolving conflicts:

   ```bash
   rg -n "<<<<<<<|=======|>>>>>>>" .
   git add <resolved-files>
   git diff --check
   ```

   Resume the commit-preserving replay after staging manual resolutions:

   ```bash
   python3 .codex/skills/hydex-upstream-sync/scripts/prepare_hydex_upstream_sync.py \
     --base-anchor "$BASE_ANCHOR" \
     --hydex-branch hydex/main \
     --upstream openai/main \
     --continue-worktree "$REPLAY_WORKTREE"
   ```

8. Regenerate and validate:

   ```bash
   cd "$REPLAY_WORKTREE/codex-rs"
   PATH=/home/mheiss/.local/bin:/home/mheiss/.cargo/bin:$PATH just fmt
   PATH=/home/mheiss/.local/bin:/home/mheiss/.cargo/bin:$PATH just write-config-schema
   PATH=/home/mheiss/.local/bin:/home/mheiss/.cargo/bin:$PATH just write-app-server-schema
   PATH=/home/mheiss/.local/bin:/home/mheiss/.cargo/bin:$PATH just test -p codex-core offload
   PATH=/home/mheiss/.local/bin:/home/mheiss/.cargo/bin:$PATH just test -p codex-core compaction_recovery
   PATH=/home/mheiss/.local/bin:/home/mheiss/.cargo/bin:$PATH just test -p codex-app-server-protocol schema_fixtures
   PATH=/home/mheiss/.local/bin:/home/mheiss/.cargo/bin:$PATH just test -p codex-app-server model_offload
   cargo check -p codex-core
   cargo check -p codex-cli --bin codex
   cargo check -p codex-memories-write
   cargo check --workspace
   PATH=/home/mheiss/.local/bin:/home/mheiss/.cargo/bin:$PATH just fix -p codex-core
   ```

   Run app-server integration tests outside the sandbox when wiremock needs to bind local ports.

9. Commit and push the scratch branch:

   ```bash
   git -C "$REPLAY_WORKTREE" status --short
   git -C "$REPLAY_WORKTREE" add -u
   git -C "$REPLAY_WORKTREE" add <intended-new-hydex-files>
   git -C "$REPLAY_WORKTREE" diff --cached --check
   git -C "$REPLAY_WORKTREE" commit -m "Hydex: refresh generated outputs for OpenAI main"
   git -C "$REPLAY_WORKTREE" push -u origin "$SCRATCH"
   ```

   Replace `<intended-new-hydex-files>` with the explicit new files shown by
   `git status`; omit the command when there are none. Never use a broad
   `git add -A` here: the checkout may contain local `.codex/config.toml`, skill
   symlinks, plugin workspaces, or generated package artifacts that are not part
   of the Hydex patch line.

10. After validation passes, atomically advance `main` to the selected OpenAI-main commit and
    `hydex/main` to the replay:

   ```bash
   OLD_MAIN=$(git rev-parse origin/main)
   OLD_HYDEX=$(git rev-parse origin/hydex/main)
   NEW_BASE=$(git rev-parse openai/main)
   NEW_HYDEX=$(git -C "$REPLAY_WORKTREE" rev-parse HEAD)
   git push --atomic \
     --force-with-lease=refs/heads/main:"$OLD_MAIN" \
     --force-with-lease=refs/heads/hydex/main:"$OLD_HYDEX" \
     origin \
     "$NEW_BASE":refs/heads/main \
     "$NEW_HYDEX":refs/heads/hydex/main
   git fetch origin
   test "$(git merge-base origin/hydex/main origin/main)" = "$(git rev-parse origin/main)"
   ```

   Use explicit `--force-with-lease` values, never blind force push. Moving both refs atomically
   prevents the remote from exposing a mismatched Hydex tip and base pointer.

11. After publication, retain a tag or remote replay branch for the previous validated tip, then
    remove the temporary worktree and local scratch branch when they are no longer needed:

   ```bash
   git worktree remove "$REPLAY_WORKTREE"
   git branch -d "$SCRATCH"
   ```

## Branch Contract

After every successful replay:

```text
origin/main        exact OpenAI tag/main commit used as the Hydex replay base
origin/hydex/main  validated Hydex patch line based on origin/main
openai/main        latest separately fetched OpenAI-main reference
```

The required invariant is:

```bash
test "$(git merge-base origin/hydex/main origin/main)" = "$(git rev-parse origin/main)"
```

This check proves that `origin/main` is an ancestor of the Hydex branch. Replay still uses
`origin/main` directly as the old base; the merge-base is only a contract check, not anchor
inference. New Hydex commits remain distinct across refreshes instead of being repeatedly
collapsed into one aggregate sync commit.
