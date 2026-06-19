# 2026-06-17 - Hydex implementation audit

This audit compares the Hydex planning docs and bootstrap prompt against the current
Codex patch on branch `hydex/main`. The Hydex patch is committed as
`b4206c2b67 Hydex: local model offload MVP` on top of upstream `origin/main` at
`a5229e0686`.

## Summary

The requested Hydex v1 behavior is implemented at the core model-client, tool-shim,
persistence/replay, and compaction-policy layers.

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
     - `[model_offload.compaction] model`
   - Not implemented as separate v1 knobs:
     - `transport`
     - `flatten_namespaces`
     - `allow_function_tools`
     - `allow_namespace_tools`
     - `allow_hosted_tools`
     - `persist_usage_marker`
   - The omitted knobs are currently fixed by policy: HTTP-only local responses, namespace flattening enabled for local, hosted tools stripped from local wire, marker always persisted when offload is used.

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

## Remaining gaps or follow-ups

- The outer Hydex planning docs and example config still use older names such as `ns_web_run` and `compaction_when_used`; they should be refreshed before treating them as end-user documentation.

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
`Model offload` line only when Hydex offload is enabled, showing the local wire model,
local provider, and sanitized local endpoint.
