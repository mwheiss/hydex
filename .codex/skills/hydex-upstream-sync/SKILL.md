---
name: hydex-upstream-sync
description: Sync the fork main branch with current openai/codex main or a Codex release tag bundled by the Hydex VS Code plugin, then replay hydex/main onto that upstream base while preserving Hydex local-offload behavior. Use when asked to rebase, replay, refresh, or bring Hydex in line with OpenAI/Codex main or a plugin-bundled Codex version.
---

# Hydex Upstream Sync

## Purpose

Bring the fork's `main` to current OpenAI Codex `main`, or replay the Hydex patch line onto the exact OpenAI Codex release tag bundled by the current Hydex VS Code plugin.

Hydex history contains synthetic upstream sync commits, so avoid a literal `git rebase origin/main` from `hydex/main`. Use a patch-stack transplant from the previous upstream anchor.

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

3. Replay Hydex onto that tag, not moving OpenAI `main`:

   ```bash
   BASE_ANCHOR=$(git merge-base hydex/main origin/main)
   UPSTREAM_TAG=$(python3 .codex/skills/hydex-upstream-sync/scripts/resolve_plugin_codex_base.py \
     --plugin-dir hydex-plugin | awk -F= '/^upstream_tag=/{print $2}')
   SCRATCH=hydex/rebase-plugin-${UPSTREAM_TAG}
   python3 .codex/skills/hydex-upstream-sync/scripts/prepare_hydex_upstream_sync.py \
     --base-anchor "$BASE_ANCHOR" \
     --hydex-branch hydex/main \
     --upstream "$UPSTREAM_TAG" \
     --scratch-branch "$SCRATCH" \
     --allow-untracked \
     --patch-out "/tmp/hydex-main-delta-${UPSTREAM_TAG}.patch"
   ```

   If `origin/main` is intentionally kept in lockstep with OpenAI `main`, leave it alone for this
   tag-pinned plugin workflow. The scratch branch base is the plugin's Codex release tag.

4. Resolve conflicts and validate as in the validation section below.

5. Commit and push the scratch branch, then advance `hydex/main` with `--force-with-lease`.

6. Rebuild and patch the plugin from the resulting Hydex commit:

   ```bash
   cd hydex-plugin
   .codex/skills/hydex-plugin-refresh/scripts/refresh_hydex_plugin.py --repo ..
   git status --short --branch
   git add vendor metadata .codex/skills scripts README.md analysis
   git commit -m "Refresh Hydex plugin for Codex <version>"
   git push
   ```

   The plugin refresh script stamps the Hydex workspace version to the bundled
   `codex-package.json` version before building, then verifies that the bundled Hydex
   `codex --version` matches.

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

3. Sync the fork `main` with OpenAI main.

   First confirm this is a fast-forward:

   ```bash
   git merge-base --is-ancestor origin/main openai/main
   git log --oneline origin/main..openai/main
   ```

   Then update the fork main and refresh local refs:

   ```bash
   git push origin openai/main:main
   git fetch origin
   ```

   If this is not a fast-forward, stop and inspect the fork-only `main` commits before pushing.

4. Enable rerere:

   ```bash
   git config rerere.enabled true
   git config rerere.autoupdate true
   ```

5. Infer the previous upstream anchor.

   After `origin/main` has been updated, the previous Hydex upstream anchor is normally:

   ```bash
   BASE_ANCHOR=$(git merge-base hydex/main origin/main)
   git rev-parse --short "$BASE_ANCHOR"
   git log --oneline "$BASE_ANCHOR"..hydex/main
   ```

   This is the commit where the current Hydex patch stack diverges from the old upstream main.

6. Create a scratch replay branch and apply the Hydex delta:

   ```bash
   SCRATCH=hydex/rebase-apply-$(date -u +%Y%m%d-openai)
   python3 .codex/skills/hydex-upstream-sync/scripts/prepare_hydex_upstream_sync.py \
     --base-anchor "$BASE_ANCHOR" \
     --hydex-branch hydex/main \
     --upstream origin/main \
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

10. Advance `hydex/main` after validation passes:

   ```bash
   git push --force-with-lease origin HEAD:hydex/main
   git branch -f hydex/main HEAD
   git checkout hydex/main
   git status --short --branch
   ```

   Use `--force-with-lease`, never blind force push.

## Current Known Good Reference

The July 8 2026 OpenAI-main transplant produced:

```text
openai/main and origin/main: f1affbac5e
scratch branch: hydex/rebase-apply-20260708-openai
scratch commit: f504e63ee9
previous upstream anchor: 07d631875e
```

Treat these as examples, not permanent constants. Prefer the `git merge-base hydex/main origin/main` inference after syncing fork main.
