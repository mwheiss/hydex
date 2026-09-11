---
name: hydex-upstream-sync
description: Replay one Hydex source patch line onto an exact OpenAI Codex tag or main commit, validate it, and when authorized publish paired base/patch refs with provenance and local alignment. Use for Hydex source rebases, exact surface-version replays, mainline syncs, replay conflicts, or source-ref finalization. For a VS Code plus Desktop release, use hydex-plugin-refresh as the coordinator.
---

# Hydex Upstream Sync

## Scope

This skill owns **one Hydex source replay onto one exact OpenAI Codex base**. It does not choose
between VS Code and Desktop versions, build host packages, patch webviews, or decide which runtime
becomes the public/system package. `$hydex-plugin-refresh` owns that cross-surface matrix and invokes
this skill once for every distinct version that needs a replay.

The replay is commit-preserving by default. The aggregate patch transplant remains a recovery
fallback, not the normal path.

## Read the right reference

- Always read [references/replay.md](references/replay.md) before preparing a replay.
- Read [references/validation.md](references/validation.md) for required checks and whenever there
  are conflicts or generated-output changes.
- Read [references/publication.md](references/publication.md) only when root publication or local
  replay cleanup is authorized.

## Branch contract

```text
origin/main        exact OpenAI tag/main commit used as the canonical Hydex replay base
origin/hydex/main  validated Hydex patch line based on origin/main
openai/main        separately fetched current OpenAI main, when needed
```

The old replay anchor is always the exact current `origin/main`, not a merge-base inferred against
the new target. Before replay and after publication require:

```bash
test "$(git merge-base origin/hydex/main origin/main)" = "$(git rev-parse origin/main)"
```

For a surface version, resolve `X.Y.Z` to the peeled OpenAI tag commit `rust-vX.Y.Z^{commit}`. Do
not guess from application or extension marketing versions.

## Invariants to preserve through conflicts

- Primary/OpenAI routes keep upstream auth, account, attestation, Agent Identity, proxy, search,
  and control-plane behavior.
- Local/offload routes never receive OpenAI/ChatGPT tokens, account headers, attestation, or Agent
  Identity headers.
- Local transforms are wire-only; canonical history retains namespace/name pairs.
- Remote compaction v1/v2 remains primary unless local routing explicitly recovers/projects it.
- `web.run` remains executable through the primary Codex search endpoint during local inference.
- Memory routing changes only under Hydex offload configuration; vanilla/no-offload stays upstream.
- Preserve Hydex commit boundaries and fail closed on unresolved conflicts.

## Deterministic entrypoints

Resolve a specific surface version:

```bash
python3 .codex/skills/hydex-upstream-sync/scripts/resolve_plugin_codex_base.py \
  --version <codex-version> --fetch-tag
```

Or omit `--version` and point `--plugin-dir` at an unpacked VSIX baseline. Prepare the replay in an
isolated worktree:

```bash
python3 .codex/skills/hydex-upstream-sync/scripts/prepare_hydex_upstream_sync.py \
  --base-anchor origin/main \
  --hydex-branch hydex/main \
  --upstream <peeled-tag-or-openai/main> \
  --scratch-branch <versioned-replay-branch> \
  --worktree <dedicated-replay-worktree>
```

The helper prints changed-path overlap and predicted conflicts, preserves the Hydex README, takes
generated schemas from the new upstream until regeneration, and refreshes `README-codex.md` from
upstream. Keep the replay worktree outside `/tmp` when it will be retained for provenance. Use
`TMPDIR=/home/mheiss/.cache/hydex-build/tmp` for Rust/Cargo work.

## Completion

A replay is complete only when:

- the exact target base and replay tip are recorded;
- required formatting, schema/lock regeneration, focused tests, and checks pass or their precise
  upstream blocker is reported;
- the replay is published on a versioned remote ref when authorized;
- paired canonical refs were advanced atomically with exact leases when this replay is selected as
  canonical;
- remote readback proves exact SHA identities and base ancestry;
- clean local canonical branches are aligned without `reset --hard`;
- active provenance is retained and only verified superseded local worktrees are removed.

Return the validated replay commit and exact Codex version to `$hydex-plugin-refresh`; do not build
plugin, desktop, Arch, or RPM artifacts from this skill.
