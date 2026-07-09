---
name: hydex-upstream-sync
description: Sync the fork main branch with current openai/codex main, then replay hydex/main onto that upstream base while preserving Hydex local-offload behavior. Use when asked to rebase, replay, refresh, or bring Hydex in line with OpenAI/Codex main.
---

# Hydex Upstream Sync

## Purpose

Bring the fork's `main` to current OpenAI Codex `main`, then replay the Hydex patch line onto that updated upstream base.

Hydex history contains synthetic upstream sync commits, so avoid a literal `git rebase origin/main` from `hydex/main`. Use a patch-stack transplant from the previous upstream anchor.

## Full Workflow

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
