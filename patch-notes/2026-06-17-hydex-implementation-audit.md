# 2026-06-17 - Hydex implementation audit

This audit compares the original Hydex planning docs and bootstrap prompt
against the current Codex patch on branch `hydex/main`. The integrated user
documentation now lives in `docs/hydex.md`.

## Summary

The requested Hydex v1 behavior is implemented at the core model-client, tool-shim,
persistence/replay, compaction-policy, remote-compaction recovery, and runtime
control layers.

The hard invariants are covered by implementation and targeted tests:

- OpenAI/Codex remains the primary control-plane provider.
- Offload is route-specific and only applies to eligible `/responses` model requests.
- Local routes do not receive OpenAI/ChatGPT auth, account metadata, attestation, or Agent Identity headers.
- Local `401` errors do not enter OpenAI auth recovery in the source path.
- Local compatibility transforms are wire-only.
- Canonical history keeps namespace/name pairs.
- Namespace tools are flattened only on the local wire and unflattened before dispatch/history.
- Hosted OpenAI tool specs are not sent to local as-is.
- `web.run` remains available under offload by exposing the namespace tool instead of hosted `web_search`.
- `offload_ever_used` is persisted and reconstructed for resume/replay.
- Encrypted OpenAI remote compaction items are recovered and promoted before a
  local-routed branch is sent to a local model.
- Runtime compaction routing can be requested from TUI and app-server clients.

## Implemented patch points

- Config:
  - `codex-rs/config/src/config_toml.rs`
  - `codex-rs/core/src/config/mod.rs`
  - `codex-rs/core/config.schema.json`
- Route-aware client setup and local request routing:
  - `codex-rs/core/src/client.rs`
- Local wire namespace flattening/unflattening:
  - `codex-rs/core/src/local_offload.rs`
- `web.run` planning under offload:
  - `codex-rs/core/src/tools/spec_plan.rs`
- Offload persistence and replay:
  - `codex-rs/protocol/src/protocol.rs`
  - `codex-rs/core/src/session/turn.rs`
  - `codex-rs/core/src/session/mod.rs`
  - `codex-rs/core/src/session/turn_context.rs`
  - `codex-rs/core/src/session/rollout_reconstruction.rs`
- Offload-aware compaction:
  - `codex-rs/core/src/compact.rs`
  - `codex-rs/core/src/session/turn.rs`
  - `codex-rs/core/src/tasks/compact.rs`
- Remote compaction recovery and promotion:
  - `codex-rs/core/src/compaction_recovery.rs`
  - `codex-rs/core/src/compaction_recovery_cache.rs`
  - `codex-rs/core/src/session/rollout_reconstruction.rs`
- Runtime controls and app-server API:
  - `codex-rs/tui/src/chatwidget/slash_dispatch.rs`
  - `codex-rs/app-server-protocol/src/protocol/v2/turn.rs`
  - `codex-rs/app-server-protocol/src/protocol/v2/thread.rs`
  - `codex-rs/app-server/src/request_processors/turn_processor.rs`

## Plan changes and deviations

These are the specific implementation-plan changes from the design docs.

1. Local flattened tool names changed from `ns_<namespace>_<function>` to `ns__<namespace>__<function>`.
   - Example: `web.run` is now `ns__web__run`.
   - Example: `mcp__codex_apps__google_calendar.search_events` is now `ns__mcp__codex_apps__google_calendar__search_events`.
   - Collision suffixes remain deterministic, for example `ns__web__run__2`.
   - Unflattening still uses the explicit per-request flat-name-to-canonical map, not delimiter parsing.

2. The persisted marker is narrower than the design sketch.
   - Implemented: `TurnContextItem.offload_ever_used: bool`.
   - Not implemented in v1: nested `model_offload` metadata with `used_this_turn`, `provider_id`, or `wire_model`.
   - This satisfies the safety requirement that replay/resume/fork reconstruct whether offload-aware policy state applies.

3. The config surface is narrower than the design-stage example.
   - Implemented:
     - `[model_offload] enabled`
     - `[model_offload] provider`
     - `[model_offload] model`
     - `[model_offload.context] context_window`
     - `[model_offload.context] effective_context_window_percent`
     - `[model_offload.context] auto_compact_token_limit`
     - `[model_offload.compaction] policy = "local" | "primary"`
     - `[model_offload.compaction] local_handoff_role = "user_summary" | "assistant_state"`
     - process/session runtime override via `--offload`, `--no-offload`,
       and `/offload on|off|auto|status`
   - Not implemented as separate v1 knobs:
     - `transport`
     - `flatten_namespaces`
     - `allow_function_tools`
     - `allow_namespace_tools`
     - `allow_hosted_tools`
     - `persist_usage_marker`
   - The omitted knobs are currently fixed by policy: HTTP-only local responses, namespace flattening enabled for local, hosted tools stripped from local wire, marker always persisted when offload is used.
   - A previous Hydex-only `[model_offload.compaction] model` override was
     removed. Primary/remote compaction now uses the currently selected primary
     Codex model.

4. Compaction policy names changed.
   - Design docs used `compaction_when_used = "standard" | "local" | "primary_remote"`.
   - Implemented config uses `[model_offload.compaction] policy = "local" | "primary"`.
   - `local` forces normal local `/responses` compaction after offload has been used.
   - `primary` preserves the primary provider's upstream compaction behavior, including remote v1/v2 selection when supported.
   - There is no distinct `standard` mode after offload; before offload is used, upstream behavior is always preserved exactly.

5. Route classification is intentionally smaller than the conceptual route enum.
   - Implemented runtime routes are `Primary` and `LocalOffload`.
   - The request kind and session source decide whether a request is eligible.
   - Review, guardian, subagent, and internal memory sources remain primary by default.

6. Remote compaction v2 is not globally disabled after offload.
   - If offload has been used and policy is `local`, compaction goes local.
   - If policy is `primary`, primary-provider compaction keeps the upstream v2 feature-gated behavior.
   - Remote v2 is not routed to the local provider.

7. Memory model-client call sites explicitly opt out of offload.
   - Memory write/runtime paths pass `ModelOffloadConfig::default()`.
   - This matches the later audit decision that memory workflows stay OpenAI/Codex-backed until explicit memory offload support exists.

8. Runtime offload control was added instead of a special compaction model override.
   - `--offload` and `--no-offload` force effective offload state for a process.
   - `/offload on`, `/offload off`, `/offload auto`, and `/offload status` update/report the
     session runtime override for future turns.
   - `/offload auto` clears the runtime override and follows config.
   - `ForceOn` is rejected unless a valid local offload provider was resolved.
   - Turning offload off does not clear `offload_ever_used`.
   - If compaction policy is `primary`, remote compaction uses the current
     primary model selected by the normal model setting.

9. App-server first-turn offload override is supported.
   - v2 `turn/start` accepts `modelOffloadOverride`.
   - Omitted means no change, `null` clears/follows config, and `"force_on"` /
     `"force_off"` set the runtime override before the first user input is
     submitted.
   - Thread settings updates use the same nullable/clear semantics.

10. Remote re-entry compaction was added for switching back into local mode.
   - Before an eligible local sampling request is sent, Hydex checks local
     context thresholds.
   - Above the local auto-compaction threshold but below the effective local
     context window, Hydex uses the normal configured compaction policy.
   - Above the effective local context window, Hydex forces primary remote
     compaction first, even when policy is `local`, then rebuilds the local
     request from compacted history.
   - Primary remote re-entry compaction uses the currently selected primary
     model; the removed compaction model override was not reintroduced.

11. Local threshold persistence was narrowed.
   - Earlier Hydex code could continue using local context thresholds for later
     primary/offload-off turns merely because `offload_ever_used` was true.
   - Implemented behavior now uses primary/upstream thresholds for primary or
     offload-off turns.
   - Local thresholds apply to actual local routes and the local sampling-boundary
     re-entry safety check.

12. Remote encrypted compaction recovery was added for local routing.
   - Config:
     - `[model_offload.compaction.recovery] model = "auto" | "primary" | <OpenAI model>`
     - `[model_offload.compaction.recovery] projection = "assistant_state" | "user_handoff"`
   - `auto` uses the producing compaction model from provenance when available,
     otherwise falls back to the current primary model with a debug warning.
   - Recovery sends the encrypted compaction item to the primary provider, strips
     duplicated cleartext history from the recovery request, and asks for a
     verbatim-simple compacted payload rendering.
   - Primary routes keep encrypted compaction items unchanged.

13. Recovered local branch state is canonicalized with existing checkpoints.
   - Default `assistant_state` projection inserts recovered text as structured
     assistant history before the next user turn.
   - `user_handoff` remains available as a compatibility projection.
   - Once a local branch bridges encrypted remote compaction, the recovered
     projection is installed through `CompactedItem.replacement_history`.
   - The recovery cache is auxiliary; replay depends on replacement history.

14. Recovery cache and failure behavior were implemented.
   - The in-session cache key includes the encrypted compaction projection hash,
     prompt version, recovery model, and algorithm version.
   - Cache hits avoid a repeated recovery call, but missing cache entries fall
     back to the normal recovery path.
   - Forced local continuation errors clearly if encrypted remote compaction
     cannot be recovered.
   - Automatic/configured local mode may degrade the current turn to primary
     continuation and logs that degraded fallback.

15. Runtime compaction controls were added.
   - TUI commands:
     - `/compaction`
     - `/compaction status`
     - `/compaction local`
     - `/compaction primary`
     - `/compaction auto`
   - The override is separate from `/offload`; `/offload on` with
     `/compaction primary` is supported.
   - Local compaction remains guarded by effective offload state and branch
     history; stale or impossible local requests are forced back to primary.

16. App-server compaction override was added.
   - v2 `turn/start` and `thread/settings/update` accept
     `modelOffloadCompactionOverride`.
   - Omitted leaves the current override unchanged, `null` clears/follows config,
     and `"local"` / `"primary"` request the runtime compaction policy.
   - `ThreadSettings` reports `modelOffloadCompactionOverride`.

17. Local assistant-state compaction was added as an opt-in mode.
   - `[model_offload.compaction] local_handoff_role = "user_summary"` preserves
     the default local compaction handoff as a user summary.
   - `local_handoff_role = "assistant_state"` keeps the same local compaction
     model call and summary prompt, but stores the resulting summary as
     structured assistant history in replacement history.

18. Retro-local fallback was implemented for recovery failure.
   - If primary-provider encrypted compaction recovery fails, Hydex reloads the
     persisted rollout, reconstructs readable source history before the newest
     active encrypted remote-compaction checkpoint, replays later readable suffix
     items, and promotes that readable branch through
     `CompactedItem.replacement_history`.
   - The fallback fails closed if an older encrypted remote compaction remains
     active in the reconstructed prefix or suffix.
   - Forced local mode errors clearly only after both primary-provider recovery
     and retro-local fallback fail.
   - Automatic/configured local mode still degrades to primary continuation if
     neither recovery path can produce local-readable history.

## Remaining gaps or follow-ups

- Upstream-inherited caveat: pre-turn compaction still runs before incoming
  user/context items are recorded, so that initial check excludes new turn
  input. Hydex now adds a local re-entry sampling-boundary check, but the
  broader upstream pre-turn estimate gap remains.
- Retro-local fallback currently reconstructs and promotes readable branch state;
  follow-on local compaction is still handled by the existing local sampling
  boundary/context-window preflight instead of a separate always-compact rescue
  request.
- No current documentation-only gap is known. The original outer Hydex planning
  skeleton has been consolidated into the actual Codex checkout as
  `docs/hydex.md`; stale planning-only module and test skeletons were not copied
  into the branch.

## Verification run

After the upstream refresh and conflict resolution:

- `cargo check -p codex-core`
- `cargo test -p codex-core offload --lib`
- `cargo check -p codex-cli --bin codex`

The focused offload test run passed 23 tests.

Additional audit pass verification:

- `cargo test -p codex-core local_offload_401_does_not_trigger_primary_auth_recovery --test all`
- `cargo test -p codex-core offload --lib`
- `HYDEX_LLAMA_SERVER_SMOKE=1 cargo test -p codex-core live_local_offload_responses_turn_completes --test all -- --ignored --nocapture`
- `cargo test -p codex-tui status_model_offload --lib`
- `cargo check -p codex-cli --bin codex`

The local-401 regression proves a local route receives no `Authorization` header, uses the
local wire model override, makes exactly one `/v1/responses` request, and surfaces the
`401` without primary auth recovery retrying.

The live offload smoke test is committed as an ignored integration test in
`codex-rs/core/tests/suite/live_hydex_offload.rs`. It was run outside the sandbox against
the llama-server on `http://localhost:8020/v1`, discovered the server's reported model
from `/v1/models`, and completed a local-routed Responses turn.

The TUI `/status` card now preserves the primary logical model line and adds a
`Model offload` line when Hydex offload is configured or runtime-forced, showing
effective state, local wire model, local provider, sanitized local endpoint, and
whether state is configured or forced.

Recent recovery/control verification:

- `just test -p codex-core compaction_recovery`
- `just test -p codex-core turn_session_force_primary`
- `just test -p codex-core compaction_runtime_override`
- `just test -p codex-tui compaction_`
- `just test -p codex-tui status_model_offload`
- `just test -p codex-app-server-protocol model_offload_compaction`
- `just test -p codex-app-server-protocol schema_fixtures`
- `just test -p codex-app-server model_offload_compaction`
