# Hydex Local Model Offload

Hydex is this fork's route-specific model offload layer for Codex CLI. It keeps
the normal OpenAI/Codex provider as the authenticated primary control plane, but
can send eligible ordinary Responses model turns to a local Responses-compatible
endpoint.

The goal is split responsibility, not a global provider switch:

```text
primary OpenAI/Codex provider:
  auth, account state, backend APIs, web.run alpha/search, image APIs,
  apps/plugins, files, realtime, cloud, remote compaction when selected

local offload provider:
  eligible HTTP /responses inference, ordinary function tools,
  namespace tools after local wire flattening, local compaction when selected
```

When model offload is disabled, Codex behavior is unchanged.

## Configuration

Hydex uses the standard Codex config and cache locations. This is intentional:
Hydex and vanilla Codex can continue each other's sessions and share ordinary
settings. Vanilla Codex ignores Hydex-only config keys in normal mode, but
`--strict-config` rejects unknown Hydex keys.

Example config:

```toml
model_provider = "openai"
model = "gpt-5.1-codex"

[model_offload]
enabled = true
provider = "local_responses"
model = "local-codex-model"
memory_mode = "local" # off | primary | local

[model_offload.compaction]
policy = "local" # local | primary
local_handoff_role = "assistant_state" # assistant_state | user_summary

[model_offload.compaction.recovery]
model = "gpt-5.4" # auto | primary | <OpenAI model name>
reasoning_effort = "none"
projection = "assistant_state" # assistant_state | user_handoff

[model_offload.context]
# context_window = 200000
# effective_context_window_percent = 95
# auto_compact_token_limit = 180000

[model_offload.validation]
enabled = true
validator_attempts = 3
generation_retries = 1
retry_temperature = 0.01
final_text = true
tool_calls = true
structured_outputs = true
memory = true
compaction = true

[model_providers.local_responses]
name = "Local Responses Offload"
base_url = "http://127.0.0.1:8020/v1"
wire_api = "responses"
requires_openai_auth = false
supports_websockets = false
request_max_retries = 0
stream_max_retries = 0
stream_idle_timeout_ms = 300000
```

`model_offload.provider` must identify a non-OpenAI provider with
`wire_api = "responses"` and a `base_url`. Hydex rejects OpenAI-backed providers
and non-Responses wire APIs for local offload.

Remote compaction recovery is used only when a local-routed branch needs to
consume encrypted OpenAI remote compaction state. The default recovery model is
`gpt-5.4` with `reasoning_effort = "none"`. `model = "auto"` uses the OpenAI
model that produced the remote compaction item when provenance is known;
otherwise Hydex falls back to the current primary model with a debug warning.
`model = "primary"` always uses the currently selected primary model at recovery
time. Any other string is treated as an explicit OpenAI model name for the
recovery request. `projection = "assistant_state"` is the default; it inserts
recovered text as assistant-history state for the local model. `user_handoff`
wraps the recovered text in a concise local-compaction-style user handoff for
compatibility with local models that preserve user-provided context more
reliably.

## Runtime Control

Hydex local offload can be forced on or off for a process without editing
`config.toml`:

```bash
hydex --offload
hydex --no-offload
hydex exec --offload "summarize this"
hydex exec --no-offload "solve this on the primary model"
```

Inside the TUI, use:

```text
/offload
/offload status
/offload on
/offload off
/offload auto
```

The runtime override controls future routing only. Turning offload off does not
clear the persisted `offload_ever_used` marker. `/offload auto` clears the
runtime override and returns to the configured `model_offload.enabled` value.
`/offload on` requires a valid resolved local provider; Hydex rejects it with a
clear error if `model_offload.provider` is missing or invalid.

App-server clients can use `modelOffloadOverride` on both
`thread/settings/update` and the first `turn/start` request. Omitted means no
change, `null` clears the runtime override and follows config, and
`"force_on"` / `"force_off"` force the runtime state for that turn and later
turns. `"force_on"` has the same provider validation as `/offload on`.

Compaction routing has a separate runtime control:

```text
/compaction
/compaction status
/compaction local
/compaction primary
/compaction auto
```

`/compaction auto` clears the runtime override and follows
`model_offload.compaction.policy`. `/compaction local` requests local compaction
when offload is enabled and the active branch state makes local compaction
eligible. `/compaction primary` requests primary/OpenAI compaction for future
compactions. This is intentionally independent from `/offload`, so
`/offload on` plus `/compaction primary` is a valid mode: ordinary turns route
local while compaction remains on the primary backend.

App-server v2 clients can use `modelOffloadCompactionOverride` on both
`thread/settings/update` and first `turn/start`. Omitted means no change, `null`
clears the runtime override and follows config, and `"local"` / `"primary"`
request the runtime compaction policy. The current `ThreadSettings` snapshot
reports both `modelOffloadOverride` and `modelOffloadCompactionOverride`.

Recommended workflow:

1. Set the primary model to a cheaper OpenAI/Codex model such as `gpt-5.4`.
2. Keep `[model_offload] enabled = true` for normal local turns.
3. Use `/offload off` and `/model gpt-5.5` when a particularly hard turn should
   run on the primary model.
4. Use `/offload on` and `/model gpt-5.4` to return to local/offloaded mode.

## Routing

Hydex currently has two runtime model request routes:

| Route | Provider |
|---|---|
| `Primary` | The normal configured Codex/OpenAI provider. |
| `LocalOffload` | The configured local Responses-compatible provider. |

Normal turn inference is locally routed when offload is enabled and the session
source is eligible. CLI, VS Code, exec, MCP, custom, and unknown session sources
are eligible. Internal, review, guardian, subagent, memory, and control-plane
workflows stay primary by default.

Primary routes keep the existing Codex request setup, auth recovery, backend
metadata, WebSocket behavior, and provider model. Local routes use HTTP
Responses streaming, omit OpenAI/ChatGPT auth and account metadata, disable
local WebSocket/prewarm behavior, and use `[model_offload].model` when set.

Local provider failures do not trigger OpenAI auth recovery.

Memory generation is separately controlled by `[model_offload].memory_mode`.
`off` disables memory generation without deleting existing memories or changing
memory reads. `primary` keeps memory generation on the primary provider. `local`
routes memory generation through the configured local offload provider. When
unset, Hydex defaults to `local` only when local offload is effectively enabled
and a valid provider is resolved; otherwise it preserves upstream primary memory
behavior.

Hydex strips OpenAI auth/control-plane headers and Codex metadata from local
model requests. Local endpoints should still tolerate Codex Responses-style
request body fields. A stricter local request scrubber may be added later if
needed.

## Local Output Validation

`[model_offload.validation]` enables a shallow sanity gate for completed
local/offloaded outputs. This is not a quality, factuality, helpfulness, or
style judge. It rejects only clearly broken local-model outputs such as empty or
placeholder payloads, visible reasoning leakage, obvious repetition loops,
malformed protocol-like JSON, or tool-call stubs where plain text is expected.

The deterministic gate runs before accepting durable local compaction payloads,
before committing local memory payloads, and before dispatching or recording
completed local sampling items. Tool-call validation happens before tool
execution, so a rejected local tool-call item is not executed first.

For local memory and local compaction, rejected payloads are hard-gated and are
not committed as memory or replacement history. If the cheap gate passes, local
memory and local compaction also run a bounded model-based sanity validator on
the local provider. The validator has a strict JSON contract:
`{"accept": true}` or `{"accept": false}`. Malformed validator output is retried
up to `validator_attempts`; validator unavailability is distinct from an
explicit rejection. Explicit rejection retries the original local generation up
to `generation_retries` before failing the hard-gated memory/compaction path.

For ordinary local final text and tool-call items, the current production path
uses the deterministic gate only. A rejected completed item surfaces a
controlled local-output failure. Streaming text deltas may already have been
shown to the UI before the completed item is validated, but the item is not
accepted into canonical history after rejection.

`retry_temperature` is emitted only for deterministic local helper calls: local
memory generation, local compaction generation, and local validation requests.
Ordinary primary or local user turns do not send a temperature field.

## Tools

Local offload accepts ordinary function tools. Codex namespace tools are flattened
only on the local wire:

```text
canonical Codex history:
  namespace = "web"
  name      = "run"

local wire:
  namespace = None
  name      = "ns__web__run"
```

Examples:

```text
web.run                                      -> ns__web__run
mcp__codex_apps__google_calendar.search     -> ns__mcp__codex_apps__google_calendar__search
```

The implementation uses a per-request flat-name to canonical `ToolName` map for
unflattening. It does not decode by splitting on `__`, because MCP namespaces can
already contain that sequence. Collision suffixes are deterministic, for example
`ns__web__run__2`.

The same wire-only transform is applied to outbound prior function-call history
in a local request. Canonical rollout history remains namespace/name based.

Hosted OpenAI tool specs such as `web_search`, `image_generation`, and
`tool_search` are not sent to the local endpoint as-is. When local offload is
active and `web.run` can be exposed, Codex prefers the namespace tool so a local
model can call flattened `ns__web__run`; execution still goes through the primary
OpenAI/Codex `alpha/search` backend.

## Compaction

Hydex preserves upstream compaction behavior until offload has actually been
used. After that, `[model_offload.compaction]` controls compaction routing:

| Policy | Behavior |
|---|---|
| `local` | Use the normal local model-call compaction path. |
| `primary` | Keep primary-provider compaction behavior, including remote v1/v2 when supported. |

If `policy = "primary"`, remote/primary compaction uses the currently selected
primary Codex model. Hydex no longer has a separate compaction model override.
If `policy = "local"`, compaction uses the local offload model when offload is
effectively enabled, offload has already been used in the session, and the local
policy applies.

`local_handoff_role` controls how local compaction inserts its compacted payload
into the replacement history. `assistant_state` is the default: local compaction
uses Hydex's assistant-continuation prompt and stores the raw compacted payload
as structured assistant history before the next user turn. `user_summary` is the
legacy option and preserves the older local compaction behavior: retained recent
user messages are followed by a synthesized user summary with the legacy summary
prefix.

Manual compaction and auto-compaction both use the offload-aware policy.

If a local-routed branch contains an encrypted OpenAI remote compaction item,
Hydex performs a primary-provider recovery preflight before constructing the
local request. The recovery request keeps the encrypted compaction item, strips
ordinary historical cleartext, adds a small diagnostic preface, and asks the
primary model to render the compacted payload as directly as possible. Primary
routing continues to send encrypted compaction items unchanged.

Once recovery succeeds, Hydex promotes the recovered cleartext into the active
local branch using the existing `CompactedItem.replacement_history` checkpoint
mechanism. The recovery cache is only an optimization: branch replay is grounded
in the promoted replacement history, not in a side cache. In
`assistant_state` mode, recovered text is inserted as a structured assistant
message before the next user turn. In `user_handoff` mode, recovered text is
wrapped as a concise user handoff message.

If primary-provider recovery fails, Hydex attempts a retro-local fallback before
giving up on local continuation. The fallback reloads persisted rollout history,
finds the active replacement-history checkpoint that contains encrypted
remote compaction, reconstructs the readable source history before that
checkpoint, appends later readable suffix items, and promotes the result as a
new local-readable `replacement_history` checkpoint. If an older encrypted
remote compaction remains in the reconstructed prefix or suffix, Hydex fails
closed rather than guessing. Explicitly forced local continuation surfaces a
clear error when both primary-provider recovery and retro-local reconstruction
fail. In automatic/configured local mode, Hydex may still degrade the current
turn back to primary continuation and logs that degraded fallback.

When switching a large primary/offload-off session back into local mode, Hydex
checks local context thresholds before sending the first local sampling request.
If the pending local request is above the local auto-compaction threshold but
below the local effective context window, Hydex compacts with the normal
configured policy. If the pending request is already too large for the local
effective context window, or local compaction would still leave it too large,
Hydex forces primary remote compaction first, then rebuilds the local request
from compacted history. This remote re-entry compaction uses the currently
selected primary model.

## Auto-Compaction Thresholds

`[model_offload.context]` lets local offload use a local model context window for
auto-compaction pressure without changing the global `model_context_window`.

If `context_window` is unset, Hydex keeps current Codex threshold behavior. If it
is set and the current sampling route is local, Hydex derives thresholds with
the same ratios as upstream model metadata:

```text
effective_context_window = context_window * effective_context_window_percent / 100
auto_compact_token_limit = min(configured_limit, context_window * 9 / 10)
```

`effective_context_window_percent` defaults to `95`. If
`auto_compact_token_limit` is omitted, it defaults to `context_window * 9 / 10`.

For example:

```toml
[model_offload.context]
context_window = 200000
```

derives:

```text
effective context window = 190000
auto-compact token limit = 180000
```

## Persistence and Compatibility

Hydex persists `TurnContextItem.offload_ever_used` in rollout history. Old
histories without the field default to `false`. Resume, replay, and fork
reconstruct this marker so offload-aware compaction policy can be applied after
a session has used local inference.

Primary/offload-off turns use the primary/upstream auto-compaction thresholds
even after earlier local offload. Local context thresholds apply to actual local
turns and to the local sampling-boundary re-entry check before switching a
branch back to local routing.

The persisted marker is deliberately minimal. Hydex does not persist local flat
tool names, local provider IDs, or local wire models into canonical history.

Vanilla Codex compatibility:

- vanilla Codex can load Hydex sessions because unknown JSON fields are ignored;
- vanilla Codex ignores `[model_offload]` in normal config loading;
- vanilla Codex rejects `[model_offload]` only when `--strict-config` is used.

Known inherited caveat: Codex pre-turn compaction currently runs before incoming
user/context items are recorded, so that first pre-turn check does not account
for new turn input. Hydex adds a local re-entry check at the sampling boundary,
but the upstream pre-turn estimate gap remains more visible when the local
offload model has a smaller context window than the primary model.

## Implementation Map

Main patch points:

- config parsing and schema:
  - `codex-rs/config/src/config_toml.rs`
  - `codex-rs/core/src/config/mod.rs`
  - `codex-rs/core/config.schema.json`
- route-aware client setup and Responses routing:
  - `codex-rs/core/src/client.rs`
- local wire tool/history transforms:
  - `codex-rs/core/src/local_offload.rs`
- `web.run` planning under offload:
  - `codex-rs/core/src/tools/spec_plan.rs`
- persisted marker and replay:
  - `codex-rs/protocol/src/protocol.rs`
  - `codex-rs/core/src/session/turn_context.rs`
  - `codex-rs/core/src/session/mod.rs`
  - `codex-rs/core/src/session/rollout_reconstruction.rs`
- compaction policy and local threshold selection:
  - `codex-rs/core/src/compact.rs`
  - `codex-rs/core/src/tasks/compact.rs`
  - `codex-rs/core/src/session/turn.rs`
- local output validation:
  - `codex-rs/core/src/local_output_validation.rs`
  - `codex-rs/memories/write/src/phase1.rs`
- remote encrypted compaction recovery and cache:
  - `codex-rs/core/src/compaction_recovery.rs`
  - `codex-rs/core/src/compaction_recovery_cache.rs`
  - `codex-rs/core/src/session/rollout_reconstruction.rs`
- app-server runtime override surface:
  - `codex-rs/app-server-protocol/src/protocol/v2/turn.rs`
  - `codex-rs/app-server-protocol/src/protocol/v2/thread.rs`
  - `codex-rs/app-server/src/request_processors/turn_processor.rs`

Useful verification:

```bash
just test -p codex-core offload
just test -p codex-tui compaction_
just test -p codex-app-server-protocol model_offload_compaction
just test -p codex-app-server model_offload_compaction
cargo check -p codex-cli --bin codex
```

Live local smoke test, when a llama-server compatible endpoint is running on
`localhost:8020`:

```bash
HYDEX_LLAMA_SERVER_SMOKE=1 cargo test -p codex-core live_local_offload_responses_turn_completes --test all -- --ignored --nocapture
```
