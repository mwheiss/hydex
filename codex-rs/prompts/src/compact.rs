pub const SUMMARIZATION_PROMPT: &str = include_str!("../templates/compact/prompt.md");
pub const SUMMARY_PREFIX: &str = include_str!("../templates/compact/summary_prefix.md");
pub const ASSISTANT_STATE_LOCAL_COMPACTION_PROMPT: &str = r#"Produce compact assistant continuation notes for your own future continuation after context compaction.

This will be inserted later as a prior assistant message before the next user message. It is not user-facing and not a handoff. Keep the successful v2-shaped workflow/source-map style, but make exact literal anchors part of the source-state contract instead of an optional afterthought.

Output style:
- Use terse bullets under the section names below.
- Do not address "another model", "the next assistant", or the user.
- Do not wrap the output in markdown fences.
- Do not include hidden chain-of-thought, scratchpad, or verbose reasoning.
- Do not claim work, file edits, tool calls, tests, builds, inspections, reads, or searches unless actually observed.

Need / current state:
- active user goal, current objective, or explicitly no active task
- domain/workflow type and immediate response obligation, if any
- repo/project/session status if applicable: repo, SHA/branch, clean/dirty status, fixed snapshots/list

Important facts to preserve:
- exact user constraints, preferences, identifiers, sentinel/canary strings, deadlines, migration/order rules, file-editing rules, tone/style requirements, and must-not rules
- observed commands, errors, fixes, tests/builds, tool results, measurements, and exact commands when available
- stale or superseded decisions, clearly marked stale

Decisions / rationale / open issues:
- choices already made, options compared, rejected approaches, and why they matter
- assumptions, caveats, unresolved questions, and what would invalidate the current trajectory

Artifacts / evidence:
- files, drafts, scripts, outputs, data, plots, PDFs, links, emails, events, generated artifacts, or other objects already created/inspected
- distinguish observed facts from inference/recommendation

Main notable source state, if applicable:
- concrete per-file facts for important provided source/config/doc files
- entry points, dispatch paths, public APIs, command/subcommand surfaces, flags/options, routes/endpoints, config keys, feature gates, migrations, and non-obvious invariants
- for each important source/config file, preserve at least one short exact anchor when present and useful: declaration, constant, macro, flag, config key, route, env var, command, or path
- avoid broad architecture dumps unless directly active or unusually important

Literal anchors:
- copy exact short expressions/literals likely to be asked about or needed for edits
- use the form `path: exact anchor` where possible
- include version/release definitions, constants/statics/macros, endpoint/route strings, CLI flags, command names, config keys, environment variables, function names, protocol tokens, important paths, exact quotes, formulas, parameter values, scores, and dates
- prefer exact declarations such as `NAME = ...` over paraphrases such as "version is ..."

Active trajectory / next response:
- last meaningful user clarification, observation, tool result, or error
- selected approach and next intended action/answer, if any
- what remains to verify

Rules:
- next user message is authoritative
- do not edit files unless asked
- do not claim unobserved tools/tests/builds/searches
- re-read raw context before exact edits, restoration, or claims when full content is not preserved

Output only the compact continuation notes."#;
