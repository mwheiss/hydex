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

Hydex history contains synthetic upstream sync commits, so avoid a literal
`git rebase origin/main` from `hydex/main`. Use a patch-stack transplant from `origin/main`, then
advance `main` to the new upstream base only after the replay passes validation.

For plugin releases, prefer the tag-pinned workflow: update the upstream preview VSIX first, read its bundled `codex-package.json` version, resolve the matching OpenAI tag `rust-v<version>`, replay Hydex onto that tag, then rebuild/inject the Hydex binary into the plugin. This keeps the Rust code and extension bundle on the same upstream Codex version.

## Plugin-Pinned Workflow

Use this when updating the Hydex VS Code plugin.

1. Refresh the plugin repo with the newest upstream preview VSIX:

   ```bash
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

3. Verify the current base-pointer contract and replay Hydex onto the new tag:

   ```bash
   git fetch origin
   BASE_ANCHOR=origin/main
   test "$(git merge-base hydex/main "$BASE_ANCHOR")" = "$(git rev-parse "$BASE_ANCHOR")"
   UPSTREAM_TAG=$(python3 .codex/skills/hydex-upstream-sync/scripts/resolve_plugin_codex_base.py \
     --plugin-dir hydex-plugin | awk -F= '/^upstream_tag=/{print $2}')
   UPSTREAM_SHA=$(git rev-parse "${UPSTREAM_TAG}^{commit}")
   SCRATCH=hydex/rebase-plugin-${UPSTREAM_TAG}
   python3 .codex/skills/hydex-upstream-sync/scripts/prepare_hydex_upstream_sync.py \
     --base-anchor "$BASE_ANCHOR" \
     --hydex-branch hydex/main \
     --upstream "$UPSTREAM_TAG" \
     --scratch-branch "$SCRATCH" \
     --allow-untracked \
     --patch-out "/tmp/hydex-main-delta-${UPSTREAM_TAG}.patch"
   ```

   `origin/main` remains at the old upstream base while the scratch replay is prepared and tested.
   This preserves the exact Hydex patch boundary. Do not replace `BASE_ANCHOR` with
   `git merge-base hydex/main openai/main`; a release-tag history can contain upstream commits that
   are not ancestors of the currently fetched OpenAI-main tip.

4. Resolve conflicts and validate as in the validation section below.

5. Commit and push the scratch branch. Then atomically advance `main` to the new upstream tag
   commit and `hydex/main` to the validated replay:

   ```bash
   OLD_MAIN=$(git rev-parse origin/main)
   OLD_HYDEX=$(git rev-parse origin/hydex/main)
   NEW_HYDEX=$(git rev-parse HEAD)
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

6. Rebuild and patch the plugin from the resulting Hydex commit:

   ```bash
   cd hydex-plugin
   .codex/skills/hydex-plugin-refresh/scripts/refresh_hydex_plugin.py --repo ..
   git status --short --branch
   git add vendor metadata .codex/skills scripts README.md analysis
   git commit -m "Refresh Hydex plugin for Codex <version>"
   git push
   cd ..
   ./packaging/arch/build-local-package.sh
   ```

   The plugin refresh script stamps the Hydex workspace version to the bundled
   `codex-package.json` version before building, then verifies that the bundled Hydex
   `codex --version` matches. The local package helper then builds the matching pacman-managed
   `hydex-bin` package from that refreshed baseline.

   Always include the generated package path and SHA-256 in the final report, followed by the exact
   update command:

   ```bash
   sudo pacman -U /absolute/path/to/hydex-bin-<version>-1-x86_64.pkg.tar.zst
   ```

   Do not run the sudo command automatically unless the user explicitly asks for installation.
   Pacman replaces the conflicting `openai-codex-bin` package; no separate removal is needed.

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
   git fetch https://github.com/openai/codex.git main:refs/remotes/openai/main
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
   python3 .codex/skills/hydex-upstream-sync/scripts/prepare_hydex_upstream_sync.py \
     --base-anchor "$BASE_ANCHOR" \
     --hydex-branch hydex/main \
     --upstream openai/main \
     --scratch-branch "$SCRATCH" \
     --allow-untracked \
     --patch-out "/tmp/hydex-main-delta-$(date -u +%Y%m%d).patch"
   ```

   If the scratch branch name already exists, use a unique suffix.

7. Resolve conflicts, if any, preserving Hydex invariants:

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

8. Regenerate and validate:

   ```bash
   cd codex-rs
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
   git diff --cached --check
   git add -A -- . ':!hydex-plugin'
   git commit -m "Hydex: sync offload patch with OpenAI main"
   git push -u origin "$SCRATCH"
   ```

10. After validation passes, atomically advance `main` to the selected OpenAI-main commit and
    `hydex/main` to the replay:

   ```bash
   OLD_MAIN=$(git rev-parse origin/main)
   OLD_HYDEX=$(git rev-parse origin/hydex/main)
   NEW_BASE=$(git rev-parse openai/main)
   NEW_HYDEX=$(git rev-parse HEAD)
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

This check proves that `origin/main` is an ancestor of the Hydex branch. Patch generation still
uses `origin/main` directly; the merge-base is only a contract check, not anchor inference.
