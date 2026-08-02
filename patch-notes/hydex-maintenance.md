# Hydex Maintenance Record

This record captures durable implementation decisions, verification expectations,
and maintained follow-ups for the Hydex patch line. Current user behavior is
documented in the [Hydex README](../README.md).

## Current Contract

The requested split-provider architecture is implemented across model routing,
local tool compatibility, persistence and replay, compaction, memory routing,
runtime controls, and app-server settings.

The maintained invariants are:

- OpenAI/Codex remains the primary provider and control plane.
- Offload is selected per Responses request, never as a global provider swap.
- Local requests receive no OpenAI/ChatGPT auth, account, attestation, Agent
  Identity, or Codex control-plane metadata.
- Local provider errors do not enter primary auth recovery.
- Local tool and history transforms are wire-only.
- Canonical history stores namespace/name pairs.
- Hosted OpenAI tool specs are not sent to local as ordinary functions.
- Local `web.run` calls still execute through the primary Codex search backend.
- Resume, replay, fork, and rollback reconstruct offload-aware policy state.
- A local request never receives an active encrypted remote compaction item.
- With no offload configuration, primary request behavior remains upstream.

## Design Decisions

### Local tool names

The final local format is `ns__<namespace>__<function>`, not the early
`ns_<namespace>_<function>` sketch. Unflattening uses an explicit map rather
than delimiter parsing. Ordinary names are reserved first and namespace
collisions receive deterministic lexical suffixes.

### Persisted state

The persisted policy marker is the narrow boolean `offload_ever_used`. Provider
IDs, local models, per-turn flags, and local flat names were intentionally not
added to canonical rollout history.

Remote compaction checkpoints record their producing primary model only when a
valid local provider is configured. This is provenance for later recovery and
does not alter the primary request.

### Route model

The implementation uses `Primary` and `LocalOffload` plus request kind and
session source, rather than a large route enum. Ordinary subagents are eligible
for offload. Compaction and memory add their own policy gates. Unclassified
internal and control-plane work remains primary.

### Compaction policy

The final config uses `policy = "local" | "primary"`. Upstream behavior is
always preserved before offload participates in the branch. Primary compaction
uses the currently selected primary model; the temporary separate compaction
model override was removed.

Local compaction defaults to raw assistant-state insertion. The prefixed
user-summary form remains an explicit compatibility mode because it mirrors
upstream readable compaction semantics.

### Runtime controls

Process flags, TUI `/offload` and `/compaction` commands, and nullable app-server
overrides were added. Runtime changes affect future requests and do not erase
historical branch state.

### Memory routing and validation

Memory generation can be disabled, kept primary, or routed local without
changing memory reads. Local memory and compaction are hard-gated durable
transforms. Ordinary local output uses deterministic structural checks only so
validation cannot duplicate side effects or become a quality judge.

## Compaction Bridge

Local continuation over encrypted primary compaction required more than the
original plan:

1. Record the producing primary model when local recovery may later be needed.
2. Recover the suffix-most active encrypted item through a primary request.
3. Project recovered text as assistant state by default.
4. Promote that projection through `CompactedItem.replacement_history`.
5. Use the in-session cache only as a bounded optimization.
6. On recovery failure, reconstruct readable state from the active persisted
   checkpoint rather than the newest raw rollout checkpoint.
7. If local re-entry forces another remote compaction, recover that new item
   before local sampling.

Malformed multiple-compaction history is defensive-only: Hydex warns, selects
the suffix-most active item, removes duplicates during promotion, and fails
closed when readable reconstruction would still retain encrypted state.

## Rebase Hardening

Post-rebase audits narrowed several boundaries that had accidentally become too
broad:

- Tool planning receives an explicit wire target; primary requests retain the
  upstream hosted-tool surface.
- Assistant-state prompts and local validation are selected only for requests
  that actually route through the offload provider.
- Primary/offload-off turns use primary context thresholds even after earlier
  local use.
- Manual Hydex context preflight does not intercept upstream readable
  compaction on a custom primary provider.
- Local context discovery and the final transformed-request guard are shared by
  ordinary turns, compaction, memory, and validator calls.
- Missing local context metadata fails before history promotion, offload-marker
  persistence, or inference.
- Detached local memory requires terminal `response.completed` before commit.
- Hydex-owned stream retries perturb only explicitly greedy local calls from
  `0.0` to the configured low retry temperature as a pragmatic escape from
  generation loops reported as transport failures. Omitted/nonzero local
  temperatures and primary retry behavior remain unchanged.
- Local namespace collision ownership is independent of tool and history order.

## Verification Contract

Stable regression coverage should continue to include:

- no-offload primary routing and primary request metadata;
- local auth/header stripping and local `401` behavior;
- namespace tool flattening, historical calls, collisions, and `web.run`;
- runtime overrides in CLI, TUI, and app-server first-turn/update paths;
- persisted `offload_ever_used` reconstruction;
- local context discovery, configured fallback, fail-closed behavior, and
  pre-send request sizing;
- remote compaction recovery, producing-model selection, rollback-aware
  fallback, and forced-remote re-entry recovery;
- assistant-state and user-handoff projection shapes;
- local memory/compaction validation and retry bounds;
- primary custom-provider compaction parity.

The ignored live local test remains useful for endpoint compatibility, while
mock request-shape and integration tests are the deterministic CI contract.

## Known Caveat

Upstream pre-turn threshold checks still exclude incoming user/context items.
Hydex compensates at local re-entry and final pre-send boundaries so an oversized
local request is not sent.

## Future Work

### Codex Desktop Linux offload control

The cross-repository plan for an opt-in Auto/On/Off composer control is tracked
in [Hydex Offload Control for Codex Desktop Linux](./codex-desktop-linux-offload-plan.md).
The Hydex app-server contract is already present; remaining work belongs in the
desktop wrapper's `linux-features/` webview patch boundary.

### Compaction prompt evaluation

Continue measuring the shipped assistant-state prompt against the explicit
`user_summary` fallback. Useful probes include exact identifiers, file anchors,
observed commands and test results, stale-plan supersession, and continuation
after an interrupted multi-call turn. Keep evaluation prompts content-general;
do not add benchmark-specific strings to production prompts.

### Retrieval-backed long-term context

Evaluate existing local MCP memory or context services before creating a Hydex
database. A useful backend should preserve provenance, support exact search,
understand rollback/checkpoint identity, and expose compact retrieval handles.
Only add core integration after a no-core-change MCP experiment demonstrates a
clear benefit.

### Persistent recovery cache

The current in-session cache rarely participates after successful promotion,
because replacement history removes the encrypted item from the active branch.
A persistent cache is justified only if measurements show repeated recovery of
the same encrypted checkpoint across abandoned attempts or alternate branches.
It must remain auxiliary and bounded.

### Request-body compatibility

Hydex currently strips sensitive local headers and Codex control-plane metadata.
A stricter body-field allowlist may be useful for endpoints that reject unknown
Responses fields. Any scrubber must remain local-wire-only and preserve the
ordinary primary request exactly.

### Earlier detached-request sizing

Detached memory can build its initial prompt before endpoint discovery has
refined configured metadata. The shared pre-send guard remains safe, but earlier
discovery could avoid work when the endpoint advertises a smaller window.

### Live compatibility matrix

Keep ignored smoke coverage for representative Responses-compatible servers.
Request-shape tests remain the stable CI contract; live tests should verify
metadata discovery, assistant-role rendering, tool-call round trips, completion
termination, and error propagation without becoming mandatory CI dependencies.

## Explicit Non-Goals

- No quality scoring, critique/rewrite loop, reranking, or multi-candidate
  sampling in the sanity validator.
- No special recovery/decryption rollout item while replacement-history
  checkpoints remain sufficient.
- No automatic persistence of local wire names or provider credentials.
- No change to primary/OpenAI routing for a feature that only improves local
  endpoint compatibility.
