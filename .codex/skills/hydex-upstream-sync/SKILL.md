---
name: hydex-upstream-sync
description: Sync Hydex onto current upstream Codex main. Use when asked to rebase, replay, refresh, or bring hydex/main in line with origin/main while preserving Hydex local-offload behavior.
---

# Hydex Upstream Sync

## Purpose

Replay the Hydex patch line onto current upstream `origin/main` without rediscovering the old giant rebase trap.

Hydex history contains synthetic upstream sync commits, so a literal `git rebase origin/main` from `hydex/main` may replay the ancient MVP from an old merge-base. Prefer a patch-stack transplant from the last known upstream anchor.

## Default Workflow

1. Confirm repo and branch state:

```bash
git status --short --branch
git fetch origin
```

Leave unrelated untracked files alone.

2. Enable rerere:

```bash
git config rerere.enabled true
git config rerere.autoupdate true
```

3. Identify the previous upstream anchor.

Known anchor from the July 2026 sync:

```text
a86d525e4d
```

If this is stale, infer the newer anchor from the latest Hydex upstream sync commit or branch notes before proceeding.

4. Use the helper script for the mechanical setup:

```bash
python3 .codex/skills/hydex-upstream-sync/scripts/prepare_hydex_upstream_sync.py \
  --base-anchor a86d525e4d \
  --hydex-branch hydex/main \
  --upstream origin/main \
  --scratch-branch hydex/rebase-apply
```

The script creates a binary patch for `<base-anchor>..hydex/main`, checks out a fresh branch from `origin/main`, and applies the patch with `git apply --3way`.

5. Resolve conflicts by preserving these Hydex invariants:

- Primary/OpenAI/Codex routes keep upstream auth, account, attestation, Agent Identity, proxy, and control-plane behavior.
- Local/offload routes never receive OpenAI/ChatGPT auth tokens, account headers, attestation, or Agent Identity headers.
- Local transforms are wire-only; canonical history keeps namespace/name pairs.
- Remote compaction v1/v2 stays primary unless local routing explicitly recovers/projects it first.
- `web.run` stays executable through the primary Codex search endpoint even when model inference is local.
- Memory routing changes only when Hydex offload config says so; vanilla/no-offload behavior stays upstream.

6. Regenerate and validate when config/protocol fields are touched:

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
```

Use existing escalation policy for tests/checks that need caches or network.

7. Commit and push the scratch branch:

```bash
git commit -m "Hydex: replay offload patch on current main"
git push -u origin <scratch-branch>
```

8. Do not move `hydex/main` until explicitly approved.

Updating `hydex/main` from the scratch branch may require:

```bash
git push --force-with-lease origin <scratch-branch>:hydex/main
```

Ask first. This rewrites the Hydex branch shape.

## Current Known Good Reference

The July 2026 current-main transplant produced:

```text
origin/main: ff06ab7172
scratch branch: hydex/rebase-apply
scratch commit: bbd0c7d316
previous upstream anchor: a86d525e4d
```

Treat these as examples, not permanent constants.
