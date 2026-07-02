# Hydex Future Plans: Local Compaction, Assistant State, and Context Memory

This document records follow-up plans and design notes from the Hydex local-compaction experiments. It is intended as a working planning document for the Hydex git repository, not final user-facing documentation.

## Current Situation

Hydex supports a split-path architecture:

- local model calls through `/v1/responses`
- hosted tools and remote compaction still available when needed
- persisted `offload_ever_used`
- remote encrypted compaction recovery
- assistant-state projection for recovered hosted v2 compaction
- assistant-state local compaction as the Hydex default
- legacy user-summary local compaction as an explicit compatibility mode

Empirical state so far:

- Hosted Codex v2 recovered cleartext behaves more like assistant continuation state than a normal handoff summary.
- The legacy open/local Codex-style user-summary handoff is robust, but handoff-framed and longer.
- Qwen-local compact assistant-state prompts can be much shorter, but may drop source-detail recall unless the prompt explicitly preserves salient source/code facts and trajectory state.
- Empty/almost-empty local outputs appear to be a local model/server/extraction failure mode that can occur across prompts, not exclusively a prompt-specific failure. It should be measured separately from semantic prompt quality.

## Implemented Local Assistant-State Default

Production Hydex now defaults:

```toml
[model_offload.compaction]
local_handoff_role = "assistant_state"
```

The bundled prompt lives in `codex-rs/prompts/src/compact.rs` as
`ASSISTANT_STATE_LOCAL_COMPACTION_PROMPT`. Both manual local compaction and auto
local compaction use it when `compact_prompt` is not explicitly configured.

Local assistant-state compaction stores the compacted payload as a structured
assistant-history message before the next user message. It does not prepend the
legacy `SUMMARY_PREFIX` and does not frame the payload as a handoff to another
model.

Explicit compatibility mode remains available:

```toml
[model_offload.compaction]
local_handoff_role = "user_summary"
```

That mode keeps the old `SUMMARIZATION_PROMPT` plus `SUMMARY_PREFIX` user
handoff behavior.

## Historical Candidate Status

### Keep as safety baseline: `baseline_handoff`

Baseline is the current handoff/user-summary style.

Strengths:

- robust
- easy for local models to produce
- preserves broad task/session state
- useful fallback when assistant-state compaction validates poorly

Weaknesses:

- verbose
- framed as a summary/handoff for another model
- less elegant when injected as assistant-role history
- can preserve metadata and broad file lists while missing detailed source facts

### Keep as lower-bound experiment: `v2_like_assistant_state`

This is an ultra-compact assistant-state skeleton.

Strengths:

- short
- semantically closer to assistant state than user handoff

Weaknesses:

- too skeletal
- less stable in earlier runs
- more prone to placeholder/fence/empty weirdness
- often omits detailed source facts

Use as a compact lower-bound/control, not a likely production prompt.

### Current compact candidate: `v2_observed_style_assistant_state`

This is a general-purpose compact assistant-state prompt inspired by observed hosted-v2 behavior, but no longer model-facing branded as "hosted v2."

Strengths:

- compact
- assistant-state style
- no handoff framing
- no benchmark-specific wording
- performed well in compact-only recall runs
- good candidate for assistant-role injection

Weaknesses:

- can be too sparse on source details
- may preserve session identity and routing state better than actionable file-level details

### Stronger source-detail candidate: `assistant_state_salient_source_facts`

This candidate keeps the compact assistant-state framing but explicitly asks for continuation-relevant source/code facts.

It should generally preserve, when present:

- entry points
- dispatch paths
- public APIs
- command/subcommand surfaces
- flags/options
- routes/endpoints
- protocols/schemas/config keys
- feature gates
- migrations/order constraints
- file-to-file relationships
- test/build entry points
- exact commands/errors/fixes/results

Important design constraint: the prompt must remain content-general. It must not include mdBook-specific names, benchmark terms, fixed token budgets, or seed-specific facts.

### Operating-rules candidate: `assistant_state_operating_notes`

This candidate adds durable action rules to the source-fact prompt.

It asks for:

- current objective or lack of one
- durable facts/constraints/identifiers
- decisions/failures/stale plans
- source/code state
- operating rules such as:
  - do not claim tests/tools ran unless observed
  - do not modify files unless asked
  - retrieve/re-read raw context before exact edits when full content is not preserved
  - treat next user message as authoritative

This is intended to capture the useful part of hosted-v2-like continuation guidance without copying stale final-response prose.

### Selected trajectory basis: `assistant_state_continuation_trace`

This candidate is designed to preserve the active trajectory of a multi-call turn.

Rationale:

Hosted-v2 cleartext sometimes contains terse "what to do next" or "recommended response" style text. Some of this may look thinking-like or stale, but it may be valuable when compaction occurs in the middle of a multi-call turn. It can preserve where the assistant was in the action trajectory, not only static facts.

This candidate asks for:

- durable state
- source/code state
- active trajectory:
  - what was actively being worked on
  - last meaningful observation/tool result/error/user clarification
  - current hypothesis or selected approach
  - next intended action/tool/edit/test
  - why that action follows from the last observation
  - what should invalidate or revise the trajectory

It explicitly avoids hidden chain-of-thought. It should preserve only concise, actionable rationale and next-action state.

This became the basis for the shipped assistant-state local compaction prompt,
with the final production wording broadened to preserve exact literal anchors
and source-state contracts. Historical comparisons remain useful against:

- `baseline_handoff`
- `v2_observed_style_assistant_state`
- `assistant_state_salient_source_facts`
- `assistant_state_operating_notes`

### Dropped: `wrapper_assistant_state`

This candidate is no longer worth pursuing.

It tested roughly "current/baseline compaction prompt, but inject as assistant state." After fixing markdown-fence issues, it still did not offer a useful Pareto point:

- similar length to baseline
- more instability
- no clear semantic advantage

Drop from future standard runs unless needed as a historical control.

## Prompt-Design Rules Learned

Prompts should be content-general.

Avoid model-facing prompt terms like:

- hosted v2
- benchmark-specific "canary" wording
- test invariants
- mdBook-specific file names
- fixed token targets
- seed-specific symbols such as `__livereload`

Use general terms instead:

- explicit invariants
- sentinel strings
- regression facts
- user-provided identifiers
- important file snapshots
- continuation-relevant source facts
- durable operating rules
- active trajectory

Do not use arbitrary token targets. Prefer:

```text
Prefer compactness, but let the payload length be determined by the density and importance of the session.
Do not drop exact constraints, source facts, identifiers, file paths, observed results, active trajectory, or durable action rules merely to be shorter.
```

## Sampling Policy

Compaction and continuation evaluation should use different sampling policies.

### Production Compaction

Compaction is an internal state transform, so prefer deterministic generation:

```text
temperature = 0
top_p = 1
seed = fixed if supported
```

Rationale:

- repeatable
- debuggable
- easier to diff and audit
- avoids random placeholder failures such as `---`

### Continuation Testing

Continuation testing should generally remain sampled, because the real question is robustness under normal operation.

Recommended evaluation phases:

```text
Phase 1: deterministic compaction + deterministic continuation
  Cheap sanity check. Confirms exact payload behavior and catches obvious failures.

Phase 2: deterministic compaction + sampled continuation
  Robustness test over 20/50/100 continuation samples.
```

## Timeout and Empty-Output Handling

### Timeout handling

A normal urllib timeout is not sufficient for streaming/SSE model calls because it is a socket inactivity timeout, not a hard wall-clock deadline. If the server keeps the stream alive, a request can exceed the intended timeout indefinitely.

The current harness should use a hard wall-clock per-request deadline:

- default timeout: 300 seconds
- retry exact same request once after timeout
- record `request_attempts`
- report request retries in the summary

### Empty/almost-empty responses

Empty or almost-empty completed responses are a separate failure mode from timeouts.

Observed examples include:

- empty final assistant output
- placeholder final output such as `---`
- potentially useful reasoning/hidden content but empty extracted final output

For now, keep these as diagnostic hard failures in harness scoring. Later summaries should compute both:

```text
operational score:
  includes empty outputs as hard failures

semantic score:
  excludes empty/almost-empty outputs
```

This separates:

```text
prompt + local stack reliability
```

from:

```text
prompt quality conditional on the model actually responding
```

Production Hydex should probably retry empty/tiny/placeholder local responses once, but the harness should not silently hide them during prompt comparison.

## Validation and Fallback Hardening

Assistant-state compaction is now the Hydex local default. Additional
validation/retry/fallback remains useful hardening rather than a blocker for
enabling the mode.

Future validation should reject at least:

- empty or tiny payloads
- placeholder-only payloads such as `---`, `OK`, `Done`
- visible reasoning/thinking leaks
- markdown fences for assistant-state modes
- missing mandatory explicit invariants or key fact groups when those existed in the source
- payloads that claim tool/test actions not present in source history

Suggested future production behavior:

```text
primary deterministic compaction
  -> validate

if invalid:
  retry once at low temperature, e.g. 0.1
  -> validate

if still invalid:
  fall back to baseline_handoff / user_summary
```

The harness may continue testing invalid payloads to measure downstream damage.
Production Hydex should eventually avoid storing invalid assistant-state payloads
as active compaction checkpoints when a validated retry or legacy user-summary
fallback can produce a safer local-readable state.

## Evaluation Status and Next Tests

### Completed / partially completed

- Hydex-shaped request harness built.
- Compact-only continuation mode added.
- Assistant-state prefix can be dropped for pure assistant-role injection.
- Cache-optimal ordering added.
- Hard wall-clock timeout/retry added.
- Generic prompt-audit pass removed seed-specific language from candidate prompts.
- Wrapper candidate dropped.

### Next high-value run

Run the latest harness comparing:

```text
baseline_handoff
v2_like_assistant_state
v2_observed_style_assistant_state
assistant_state_salient_source_facts
assistant_state_operating_notes
assistant_state_continuation_trace
```

Settings:

```text
deterministic compaction
sampled continuation
compact_only history
assistant_state_prefix = none
cache_optimal ordering
hard 300s request timeout
```

### Detailed source-recall probes

Current detailed probes are still mdBook-seed-specific, which is fine for the harness, but the compaction prompts themselves must remain generic.

The probes should continue to test whether compacted state can answer details that summaries often drop:

- live-reload endpoint
- init flags
- test command details
- version definition
- guide/CLI command files
- basic follow-up discipline

Future additional probe types should include:

- multi-call-turn resumption
- stale-plan supersession
- "did we actually run this?"
- exact command/test provenance
- trajectory continuation after a tool result
- action invalidation after a new user message

## Remote v2 Interpretation

Remote hosted v2 should not be treated as a literal prompt target.

Better interpretation:

```text
remote v2 cleartext is evidence of a useful state representation,
not a requirement to copy every line or every style quirk.
```

Useful buckets:

```text
A. Durable facts
   repo, files, exact results, constraints, user preferences

B. Active trajectory
   what was being worked on, last observation, selected approach, next intended action

C. Durable operating rules
   do not claim tests ran unless observed; next user message overrides prior trajectory

D. Ephemeral final-response wording
   exact suggested response text, "answer briefly now", etc.
```

A, B, and C are likely valuable. D may be useful only immediately after compaction and can become stale.

The `assistant_state_continuation_trace` candidate is intended to preserve A/B/C while avoiding verbose hidden reasoning or over-specific final-response prose.

## Context DB / MCP Memory Direction

Long-term, compaction should not try to preserve all facts directly. It should preserve:

- the active state
- active trajectory
- durable operating rules
- the existence of relevant detailed information
- retrieval handles or context-set identifiers

The detailed information should live outside the model context window in a local memory/database service exposed through MCP.

Desired compact-state shape:

```text
Current task state:
- active objective / no current task
- current trajectory / last observation / next intended action
- key constraints and invariants

Recoverable context:
- repo snapshot stored in memory/context DB
- file facts stored under known handles
- raw tool outputs stored under known handles
- decisions and failed attempts stored under known handles

Retrieval policy:
- before answering detailed questions about file contents, commands, errors, tests, or identifiers, query the context DB
- do not claim tests/tools ran unless present in live history or context DB
```

This could make local compaction very small while preserving high recall.

## Existing Memory / Context DB Candidates

Do not build a greenfield Hydex memory DB first. Evaluate existing MCP memory systems.

### Primary candidates

#### `codex-agent-mem`

Most aligned with Hydex/Codex operational memory.

Promising features:

- Codex-focused
- local SQLite / FTS-style memory
- compact working-memory packs
- operational state
- provenance
- completion checks
- context pack hashes / not-modified behavior

Likely best fit for compaction-aware working memory.

#### EchoVault

Simple local-first MCP memory server.

Promising features:

- supports Codex, Claude Code, Cursor, OpenCode
- Markdown vault under local filesystem
- SQLite/FTS search
- optional semantic search
- small tool surface
- Obsidian-readable

Likely best first practical test because it is simple and low-friction.

#### AgentVault Memory

Interesting because it focuses on raw conversation history ingestion/search, not only memories explicitly saved by the agent.

This matters for "perfect recall":

```text
agent-decided memory:
  cleaner but can forget what was not saved

raw-history search:
  noisier but better for exact recall
```

#### codebase-memory-mcp

Complementary structural code memory, not a conversation memory replacement.

Useful for:

- symbol/function/class lookup
- call chains
- code structure
- route and cross-file relationships

Could pair well with an episodic/session memory server.

### Secondary candidates

- Basic Memory
- agentmemory
- Memorix
- memsearch
- Graphiti
- Cognee
- Cipher / ByteRover
- Supermemory

These may be useful later, especially for cross-agent or graph memory, but the first tests should be local-first and easy to debug.

## Proposed MCP Memory A/B

Use the same compact-state prompt with different memory backends:

```text
A: assistant_state_continuation_trace only
B: assistant_state_continuation_trace + EchoVault memory_context/search
C: assistant_state_continuation_trace + codex-agent-mem context pack
D: assistant_state_continuation_trace + AgentVault raw-history search
E: assistant_state_continuation_trace + codebase-memory-mcp for source-structure recall
```

Test failure modes:

- exact command/test history
- obscure identifiers/sentinel strings
- why a previous approach failed
- source-file details compressed away
- stale vs active plans
- whether an action was actually run
- retrieval before answering source-detail questions
- continuation after an interrupted multi-call turn

## Possible Hydex Integration Design

### Minimal integration

No Hydex core changes. Configure MCP memory server and add compact-state retrieval instructions.

Pros:

- fast to test
- no core risk
- works with existing MCP infrastructure

Cons:

- model may fail to retrieve consistently
- compaction has no structured knowledge of what was stored

### Medium integration

Hydex writes compaction artifacts and raw events to a memory MCP or local sidecar.

Potential event types:

- raw user message
- assistant message
- tool call
- tool result
- file snapshot
- decision
- failed attempt
- test result
- compact state
- trajectory checkpoint
- explicit invariant / sentinel / identifier

Compaction payload includes memory handles.

### Deep integration

Hydex owns memory lifecycle:

- append-only session store
- memory snapshots per compaction
- retrieval manifest inserted into compact state
- validation checks against memory
- context pack diffing / hashes
- rollback-aware checkpoint memory

This may be valuable later, but should only happen after existing MCP memory options have been tested.

## Open Decisions

- Should future prompt variants replace the shipped assistant-state default or
  remain evaluation-only candidates?
- How much active trajectory should be preserved before it becomes stale-plan risk?
- Should empty/almost-empty responses be excluded from semantic prompt-quality scores while retained in operational scores?
- Which MCP memory backend should be tested first: EchoVault or codex-agent-mem?
- Should compaction explicitly emit a retrieval manifest?
- Should Hydex automatically save raw events/tool results into memory, or rely on an external memory server to ingest history files?
- Should local compaction preserve a recent tail verbatim, Gemini-style?
- Should repeated compactions update an anchored prior state rather than regenerate from full surviving history?

## Near-Term Action Items

1. Run the latest compact-only/no-prefix/cache-optimal harness against the
   shipped assistant-state local compaction prompt.
2. Report both operational and non-empty semantic scores.
3. Compare payload contents against local GPT-5.5 readable and recovered hosted v2.
4. Add or refine multi-call-turn trajectory probes.
5. Install and test EchoVault with Hydex/Codex MCP config.
6. Install and test `codex-agent-mem`.
7. Test whether compact state can store memory handles rather than detailed facts.
8. Decide whether to add production validation/retry/fallback around the shipped
   assistant-state mode.
