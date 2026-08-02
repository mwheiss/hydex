# Hydex

Hydex is a route-specific local inference layer for Codex CLI. OpenAI/Codex
remains the authenticated primary provider and control plane. Eligible model
inference can be sent to a local Responses-compatible endpoint.

```text
primary provider:
  auth, account and backend APIs, hosted tools, files, realtime,
  remote compaction, and other control-plane work

local offload provider:
  eligible HTTP /responses inference, ordinary function tools,
  namespace tools after wire-only flattening, and local compaction
```

With no `[model_offload]` configuration, primary model calls and persisted
history retain upstream Codex behavior.

Hydex is maintained as a focused patch line over OpenAI Codex. See the
[upstream Codex README](./README-codex.md) for the original project overview,
installation methods, and general Codex documentation.

## Build and Install

Hydex keeps the `codex` executable name so it can replace vanilla Codex without
changing editor or app-server integrations. Build it from source with the
upstream Rust toolchain:

```bash
cd codex-rs
cargo build --release -p codex-cli --bin codex
```

Linux x86_64 package helpers are available for [Arch Linux](./packaging/arch/)
and [RHEL 10](./packaging/rpm/). General source-build prerequisites remain in
the [upstream installation guide](./docs/install.md).

## Configuration

Hydex intentionally uses the standard Codex config, cache, and session
locations. Hydex and vanilla Codex can continue each other's sessions and share
ordinary settings. Vanilla Codex ignores Hydex-only config keys in normal mode;
`--strict-config` rejects them as unknown.

```toml
model_provider = "openai"
model = "gpt-5.4"

[model_offload]
enabled = true
provider = "local_responses"
model = "local-codex-model"
memory_mode = "local" # off | primary | local

[model_offload.compaction]
policy = "local" # local | primary
local_handoff_role = "assistant_state" # assistant_state | user_summary

[model_offload.compaction.recovery]
model = "gpt-5.4" # auto | primary | an explicit OpenAI model
reasoning_effort = "none"
projection = "assistant_state" # assistant_state | user_handoff

[model_offload.context]
# Fallbacks used only when the endpoint omits useful runtime metadata.
# context_window = 180000
# effective_context_window_percent = 95
# auto_compact_token_limit = 162000

[model_offload.validation]
enabled = true
validator_attempts = 3
generation_retries = 1
retry_temperature = 0.01
memory_temperature = 0.0
compaction_temperature = 0.0
validator_temperature = 0.0
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

`model_offload.provider` must identify a non-OpenAI provider with a `base_url`
and `wire_api = "responses"`. Hydex rejects OpenAI-backed providers and other
wire APIs for local offload.

## Runtime Controls

Process-level flags apply to interactive and exec sessions:

```bash
codex --offload
codex --no-offload
codex exec --offload "summarize this"
codex exec --no-offload "run this on the primary model"
```

The TUI exposes session-level controls:

```text
/offload [status]
/offload on
/offload off
/offload auto

/compaction [status]
/compaction local
/compaction primary
/compaction auto
```

`/offload auto` and `/compaction auto` clear their runtime overrides and follow
configuration. Turning offload off changes future routing but does not erase
the historical `offload_ever_used` marker. Forcing offload on requires a valid
resolved local provider.

App-server v2 exposes the same controls on both first `turn/start` and later
`thread/settings/update` requests:

- `modelOffloadOverride`: `"force_on"`, `"force_off"`, or `null` to follow
  config.
- `modelOffloadCompactionOverride`: `"local"`, `"primary"`, or `null` to
  follow config.

Omitting either field leaves the current override unchanged. `ThreadSettings`
reports both requested overrides.

## Routing

Hydex has two model request routes:

| Route | Behavior |
| --- | --- |
| `Primary` | Existing provider, auth, metadata, retries, WebSocket behavior, and selected primary model. |
| `LocalOffload` | Configured local provider and model over HTTP Responses streaming. |

Eligible ordinary turns, exec and VS Code sessions, MCP/custom sessions, and
ordinary subagents can route local. Review, Guardian, spawned/thread, and other
agent-labelled workers follow the same eligibility rules. Compaction and memory
requests additionally follow their own policies. Control-plane helpers and
other unclassified internal workflows stay primary.

Local requests:

- use HTTP rather than WebSocket or prewarm;
- omit OpenAI/ChatGPT auth, account, attestation, Agent Identity, and Codex
  control-plane metadata;
- do not trigger OpenAI auth recovery when the local provider fails;
- use `[model_offload].model` when configured.

Local endpoints still need to accept the Codex Responses-style request body.
Hydex currently strips sensitive headers and metadata rather than maintaining a
strict allowlist of every request-body field.

### Memory Routing

`memory_mode` controls memory generation separately from memory reads:

| Mode | Behavior |
| --- | --- |
| `off` | Skip memory generation. Existing memories and reads are unchanged. |
| `primary` | Preserve the normal primary-backed memory workflow. |
| `local` | Route detached generation and memory-consolidation turns locally. |

When omitted, memory generation defaults to local only when offload is
effectively enabled and a valid provider exists. Otherwise it remains primary.
Local memory generation uses `memory_temperature`, requires a terminal
`response.completed`, and passes its output gates before durable commit.

## Local Tools

Ordinary function tools are sent to the local endpoint. Namespace tools are
flattened only on the local wire:

```text
web.run                                  -> ns__web__run
mcp__codex_apps__calendar.search_events  -> ns__mcp__codex_apps__calendar__search_events
```

Canonical rollout history keeps the original namespace/name pair. A per-request
map unflattens returned calls before dispatch and history recording; names are
never decoded by splitting on `__` because MCP namespaces can contain that
sequence.

Ordinary names are reserved before namespace names. Namespace collisions are
assigned deterministic lexical suffixes such as `__2`, independent of tool or
history order. Prior namespace calls also participate in the map when their
tool is no longer advertised on the current turn.

Hosted OpenAI tool specs such as `web_search`, `image_generation`, and
`tool_search` are not sent to the local endpoint as ordinary tools. When
available, `web.run` is exposed as a namespace tool and flattened locally; its
execution still uses the primary Codex search backend.

## Output Validation

Hydex applies bounded structural checks to completed local output. These checks
are not a judge of correctness, quality, style, factuality, or completeness.
They reject clearly broken output such as:

- empty, placeholder, or content-free durable payloads;
- obvious repetition loops;
- structural `<think>...</think>` leakage;
- malformed protocol-like JSON;
- tool-call stubs where text is expected;
- runaway output beyond the endpoint-derived bounds.

Ordinary final text and tool calls use the deterministic gate only. Tool calls
are checked before execution. A rejected completed item is not accepted into
canonical history, although already streamed text may have reached the UI.

Local memory and compaction are durable state transforms, so they are hard
gated. After deterministic checks they use a non-recursive local validator that
must return exactly one of:

```json
{"accept": true}
```

```json
{"accept": false}
```

Malformed validator output and validator transport failures consume the bounded
`validator_attempts` budget. Validator attempts after the first use
`retry_temperature`. Rejection reruns the original memory or compaction
generation up to `generation_retries`; it does not critique, rewrite, rank, or
sample multiple candidates.

Ordinary user turns omit temperature unless explicitly configured elsewhere.
The first local memory, compaction, and validator calls default to temperature
`0.0`. Their optional config fields replace that default. A rejected memory or
compaction generation retry uses `retry_temperature`, which defaults to `0.01`.
When Hydex itself retries an explicitly greedy (`temperature = 0.0`) local
sampling request after a retryable stream failure, the new attempt uses
`retry_temperature`. This is an intentional local-server workaround: generation
loops and malformed framing can surface as transport-style failures, and a tiny
perturbation may avoid repeating them. Calls that omit temperature, including
ordinary local turns using the endpoint default, continue to omit it on retry.
Nonzero temperatures are likewise not replaced. Primary/OpenAI retries are
unchanged.

## Compaction

Hydex preserves upstream compaction behavior until local offload has actually
participated in the branch. Thereafter:

| Policy | Behavior |
| --- | --- |
| `local` | Use the local model through the normal `/responses` compaction path when effective offload state permits it. |
| `primary` | Preserve primary-provider compaction, including upstream remote v1/v2 selection. |

Primary compaction uses the currently selected primary model. There is no
separate compaction-model override.

### Local Handoff Shape

`local_handoff_role = "assistant_state"` is the default. Hydex uses its bundled
assistant-continuation prompt and installs the raw result as structured
assistant history before the next user message.

`user_summary` is the explicit compatibility mode. It preserves upstream
readable-compaction behavior by inserting a prefixed user summary. Both are
canonical replacement-history checkpoints, not additive context fragments.

### Encrypted Remote Compaction Recovery

A local model cannot consume an encrypted OpenAI remote compaction item. Before
local routing, Hydex therefore:

1. keeps the active encrypted item in a primary recovery request;
2. removes duplicated ordinary cleartext history from that request;
3. asks the selected primary recovery model for a direct cleartext rendering;
4. projects the result as assistant state by default;
5. installs the projected history through `CompactedItem.replacement_history`.

Primary routing leaves encrypted items untouched. The in-session recovery cache
is only an optimization; promoted replacement history is the durable branch
state used by resume, replay, fork, and rollback.

Recovery model selection:

- the default is explicit `gpt-5.4` with reasoning effort `none`;
- `auto` uses the recorded producing model, or falls back to the current primary
  model with a warning when provenance is unavailable;
- `primary` always uses the current primary model;
- any other value is an explicit OpenAI model name.

`projection = "assistant_state"` inserts raw recovered text as assistant
history. `user_handoff` wraps it as an upstream-style user handoff for local
models that preserve user-provided state more reliably.

Well-formed active history has one compacted-state representation. Defensive
handling chooses the suffix-most active encrypted item, removes malformed
duplicates during projection, and warns. Recovery failure first attempts
rollback-aware retro-local reconstruction from the active checkpoint. Forced
local mode errors if no readable state can be produced; automatic local mode
may degrade the current turn to primary and report that fallback.

If a primary remote compaction is forced while preparing an oversized local
turn, Hydex immediately recovers and promotes the newly created encrypted item
before constructing the local request.

## Local Context Limits

Before the first local request in each model-client session, Hydex probes useful
runtime metadata from common Responses-compatible servers. Supported shapes
include Codex/ik_llama and OpenAI-style model lists, vLLM, llama.cpp, Ollama,
LM Studio, SGLang, and TGI metadata. Runtime values override configured
fallback fields. Training-only limits such as llama.cpp `n_ctx_train` are not
treated as the active context window.

If neither discovery nor `[model_offload.context]` supplies a context window,
Hydex refuses local routing before compaction, encrypted-history promotion,
offload-marker persistence, or inference. The normal core error path carries
this failure to CLI, TUI, app-server, VS Code, and desktop clients. Primary and
offload-disabled routes are unaffected.

Missing thresholds use the upstream ratios:

```text
effective_context_window = context_window * effective_context_window_percent / 100
auto_compact_token_limit = min(configured_limit, context_window * 9 / 10)
```

`effective_context_window_percent` defaults to `95`; the auto-compaction limit
defaults to 90 percent of the raw window.

Every transformed local request is estimated again immediately before network
send. Oversized requests are rejected rather than relying on the endpoint to
truncate them. Normal turns first use the richer local re-entry compaction
flow; memory and validator requests fail closed.

When re-entering local mode with a large primary history, Hydex may compact
before sampling. It uses configured policy below the effective local window. If
the request already exceeds that window, or local compaction cannot fit, Hydex
forces primary remote compaction and then recovers the result before local
sampling.

Primary and forced-primary turns always use primary/upstream thresholds, even
after previous local use.

## Persistence and Compatibility

Hydex persists only `TurnContextItem.offload_ever_used` as offload policy state.
Old histories default it to `false`. Resume, replay, and fork reconstruct it.
Local wire names, provider IDs, and local model IDs are not persisted into
canonical history.

A configured valid local provider also records the primary model that produced
a remote compaction checkpoint. This metadata does not change the primary
request and allows later `recovery.model = "auto"` selection. Sessions without
a local provider keep the upstream checkpoint shape.

Known upstream caveat: the first pre-turn compaction check occurs before the
incoming user/context items are recorded. Hydex's local re-entry flow and final
pre-send guard still prevent an oversized local request from being sent.

Implementation decisions, verification expectations, and future work are kept in
[the Hydex maintenance record](./patch-notes/hydex-maintenance.md).
