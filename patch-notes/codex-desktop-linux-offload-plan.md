# Hydex Offload Control for Codex Desktop Linux

- Status: proposed
- Last evaluated: 2026-08-01
- Target repository: [ilysenko/codex-desktop-linux](https://github.com/ilysenko/codex-desktop-linux)

## Objective

Add an opt-in Hydex offload selector to the Codex composer in the Linux desktop
wrapper. The control should match the Hydex VS Code integration:

- `Hydex auto` clears the runtime override and follows `model_offload.enabled`.
- `Hydex on` forces eligible requests through the configured local provider.
- `Hydex off` forces eligible requests through the primary provider.
- The selected value is applied to the first turn of a new task and to future
  turns in an existing task.

The implementation should remain a desktop-wrapper feature. It should not add
desktop-specific code to Hydex core or require changes to the upstream macOS
application source.

## Summary

This is a small-to-medium integration. Hydex already exposes the required
app-server protocol and routing behavior. The remaining work is a minified
webview patch in `codex-desktop-linux`, together with drift-resistant tests and
feature documentation.

A production-quality implementation is expected to take one to two focused
days. A proof of concept should take two to four hours. The expected change is
approximately 120-200 lines of patch code, 150-300 lines of tests, and a small
feature manifest and README.

No Rust changes are expected for the initial implementation.

## Existing Hydex Contract

Hydex app-server v2 already supports a nullable model-offload override on both
relevant request paths:

- `turn/start.modelOffloadOverride` applies to the first turn and becomes the
  sticky setting for later turns.
- `thread/settings/update.modelOffloadOverride` updates the sticky setting for
  subsequent turns.
- `ThreadSettings.modelOffloadOverride` reports the requested effective
  runtime override.

The wire values are:

| UI selection | App-server value | Meaning |
| --- | --- | --- |
| `Hydex auto` | `null` | Clear the runtime override and follow config. |
| `Hydex on` | `"force_on"` | Force local offload when the provider is valid. |
| `Hydex off` | `"force_off"` | Force primary routing. |

Omitting the field is not equivalent to Auto. Omission retains the thread's
current override, while `null` explicitly clears it. The desktop request bridge
must therefore send `null` for Auto rather than deleting the request field.

Relevant Hydex implementation references:

- `codex-rs/app-server-protocol/src/protocol/v2/turn.rs`
- `codex-rs/app-server-protocol/src/protocol/v2/thread.rs`
- `codex-rs/app-server/README.md`
- `codex-rs/app-server/tests/suite/v2/turn_start.rs`
- `codex-rs/app-server/tests/suite/v2/thread_settings_update.rs`

The VS Code implementation provides the behavioral baseline in
`hydex-plugin/scripts/apply_hydex_patch.py`.

## Desktop Integration Boundary

The change should be implemented as an opt-in Linux feature:

```text
linux-features/hydex-offload/
├── README.md
├── feature.json
├── patch.js
└── test.js
```

The feature should use `webview-asset` patch descriptors. It does not need an
Electron main-process patch, runtime hook, package hook, or staged resource.

This matches the wrapper's intended extension boundary: optional workflow
integrations live under `linux-features/`, are disabled by default, and are
checked independently for upstream asset drift.

### Evaluated desktop baseline

The assessment used the locally installed desktop build:

- Upstream app version: `26.721.81911`
- Wrapper version: `0.10.4`
- Wrapper source commit: `042adb7b23b1f47ca8d6a09da0b1abc5e00ff4a7`
- Main webview asset: `app-initial-CRKqnyc3.js`

That build contains one unambiguous app-server `sendRequest` implementation
and one composer model-picker component with the active `conversationId` in
scope. These are suitable independent patch seams. Names and bundle hashes are
minified implementation details and must be rediscovered and asserted rather
than treated as stable API.

## Proposed Design

### 1. Feature manifest

Add a disabled-by-default feature with an optional webview patch descriptor:

```json
{
  "id": "hydex-offload",
  "title": "Hydex Offload Control",
  "description": "Adds Auto, On, and Off controls for Hydex local model offload.",
  "defaultEnabled": false,
  "entrypoints": {
    "patchDescriptors": "./patch.js"
  }
}
```

The feature README should state that enabling the UI does not install Hydex.
The desktop app must resolve a Hydex `codex` executable, normally through an
installed Hydex package or an explicit `CODEX_CLI_PATH`.

### 2. Stored selection

Use the same storage key and values as the VS Code integration:

```text
hydex.offloadOverride = "force_on" | "force_off"
```

Absence of the key represents Auto. Invalid or unreadable values must fall
back to Auto. A helper should provide the normalized wire value:

```text
missing/invalid -> null
force_on       -> "force_on"
force_off      -> "force_off"
```

The minimal design intentionally makes this a desktop-wide preference. When a
turn is sent, the dropdown selection is authoritative over a stale sticky
thread override.

### 3. Request bridge patch

Patch the desktop app-server request client before it enqueues requests. For
`turn/start` and `thread/settings/update`, copy the params and inject the
normalized `modelOffloadOverride` value.

The patch must tolerate both known update payload shapes:

- Flat params: `{ threadId, ..., modelOffloadOverride }`
- Nested compatibility params:
  `{ threadSettings: { ..., modelOffloadOverride } }`

The current desktop baseline uses the flat app-server v2 shape. Supporting the
nested form costs little and keeps the patch aligned with the VS Code bridge.

All other request methods must remain byte-for-byte behaviorally unchanged.
The patch must be idempotent and carry a unique completion marker.

This bridge is required even if the dropdown sends an immediate update. A new
task may not have a loaded app-server thread when the user changes the
selection, so only injection into its first `turn/start` can guarantee correct
first-turn routing.

### 4. Composer dropdown patch

Insert a compact native `select` adjacent to the existing composer model
picker. Reuse the surrounding design tokens and composer sizing conventions.
The control should expose:

```text
Hydex auto
Hydex on
Hydex off
```

Requirements:

- Initialize from normalized `localStorage` state.
- Persist On and Off; remove the key for Auto.
- Provide an accessible label and tooltip such as `Hydex offload`.
- Participate in the existing responsive footer wrapper so the combined model
  and Hydex control can collapse on narrow composers.
- Remain hidden anywhere the local Codex model picker is not rendered,
  including cloud-only composer modes.
- Avoid adding a general desktop settings surface in the initial change.

The current baseline offers two reasonable insertion points:

1. Wrap the existing model-picker call in the footer control with a flex span
   containing the picker and Hydex control.
2. Extend the model-picker component's returned fragment with the Hydex
   control.

The first option is preferred because it keeps Hydex outside the upstream
model-picker implementation while retaining `conversationId` and existing
responsive measurement behavior.

### 5. Immediate update for existing tasks

After changing the stored value, use the desktop renderer's existing
`update-thread-settings-for-next-turn` bridge when a conversation id is
available:

```text
threadSettings: {
  modelOffloadOverride: null | "force_on" | "force_off"
}
```

Failures should use the desktop's normal error path or be surfaced as a small
toast. The UI must retain the selected preference even if the active task
cannot be updated, because request injection will apply it on the next eligible
turn.

The immediate update must be explicit. Merely storing the value and patching
future requests does not update an already loaded task until another settings
or turn request occurs.

### 6. CLI selection

Do not bundle Hydex as part of the first implementation. Document one of these
supported arrangements:

```bash
CODEX_CLI_PATH=/path/to/hydex/codex ./codex-app/start.sh
```

or install the Hydex Arch/RPM package so the wrapper resolves its `codex`
executable through the normal lookup order.

The feature cannot reliably infer from the ordinary Codex version string that
the selected binary supports Hydex fields. Explicit feature enablement is the
capability declaration for the initial integration.

## Patch Structure

`patch.js` should expose small independently testable transforms:

- `normalizeStoredOffloadOverride` or its injected equivalent.
- `applyHydexRequestBridgePatch(source)`.
- `applyHydexComposerControlPatch(source)`.
- One descriptor per owning asset or patch seam.

Each transform should:

- recognize an already-patched asset;
- require exactly one intended current anchor;
- avoid broad replacement across unrelated functions;
- warn and return the original source when its expected shape disappears;
- never leave a partially mutated asset;
- include a stable marker for idempotency and inspection.

If both mutations target the same asset in a given upstream build, keep them
as separate descriptors or perform both transactionally in one descriptor.
Do not commit a half-applied request/UI pair as a successful feature patch.

## Test Plan

### Unit fixtures

Add focused minified-JavaScript fixtures covering:

- request injection for `turn/start`;
- request injection for flat `thread/settings/update` params;
- request injection for nested update params;
- Auto injecting `null` rather than omitting the field;
- On and Off injecting the exact snake-cased enum wire values;
- unrelated request methods remaining unchanged;
- composer control insertion beside the intended model picker;
- normalization of missing, invalid, and inaccessible local storage;
- immediate update dispatch when a conversation id exists;
- no immediate update dispatch when no conversation id exists;
- repeat application producing identical output;
- malformed or partially matching assets warning without mutation.

### Feature framework tests

Verify that:

- `hydex-offload` is absent when not listed in `features.json`;
- enabling it loads only its expected `webview-asset` descriptors;
- descriptors target the current owning bundle and not adjacent bundles;
- missing current assets produce explicit `skipped-optional` report entries;
- applying the descriptors through the real patch runner updates a temporary
  extracted app fixture;
- the resulting JavaScript passes a syntax check.

### Current-DMG validation

Before merging or packaging:

1. Build from a fresh accepted upstream DMG.
2. Enable only `hydex-offload` first and inspect the patch report.
3. Run the feature test directly with Node.
4. Run the wrapper's complete relevant test suite.
5. Launch with `CODEX_CLI_PATH` pointing to Hydex.
6. Confirm the selector appears beside the model picker at normal and narrow
   composer widths.
7. Test Auto, On, and Off on both a new task and an existing task.
8. Verify routing using Hydex logs or a controlled local Responses endpoint.
9. Switch between tasks with different prior sticky overrides and verify the
   selected desktop preference is authoritative on the next turn.
10. Rebuild with the feature disabled and verify the upstream UI and request
    shapes are restored.

## Acceptance Criteria

The implementation is complete when:

- The feature is disabled by default and build-selectable through
  `linux-features/features.json`.
- Auto, On, and Off render beside the local composer model picker.
- The selection survives desktop restarts.
- Auto sends `modelOffloadOverride: null`.
- On sends `modelOffloadOverride: "force_on"`.
- Off sends `modelOffloadOverride: "force_off"`.
- The first turn of a new task uses the selected override.
- Changing the selector in an existing task queues an immediate settings
  update for subsequent turns.
- Official desktop behavior is unchanged when the feature is disabled.
- Patch application is idempotent and fails visibly on upstream drift.
- Tests cover request behavior, UI insertion, feature gating, and patch-runner
  integration.

## Risks and Mitigations

### Upstream minified asset drift

This is the main maintenance cost. Bundle names, component symbols, memo-cache
layouts, and JSX aliases can change with each upstream DMG.

Mitigations:

- target semantic string clusters and function shapes, not a single minified
  variable name;
- require unique matches;
- keep request and UI transforms independently diagnosable;
- test idempotency and partial drift;
- include the feature in fresh-DMG acceptance when enabled.

Expected maintenance is roughly 30-90 minutes after an upstream release that
changes one of the two patch seams.

### Feature enabled with vanilla Codex

Vanilla app-server builds may ignore unknown Hydex request fields, leaving a UI
that appears functional but does not change routing.

Mitigation: keep the feature opt-in, require an explicit Hydex CLI in the
README, and include a troubleshooting check such as `codex --help` containing
`--offload` and `--no-offload`.

Reliable automatic detection would require a new capability handshake and is
out of scope for the initial change.

### Global preference versus per-task state

The proposed `localStorage` design is a global desktop preference, while the
app-server override is sticky per thread. A task may briefly report an older
sticky setting after navigation.

Mitigation: inject the selected value into every relevant outgoing request and
send an immediate settings update on selection changes. Label the dropdown as
the authoritative desktop preference.

If per-task display synchronization becomes necessary, add a later phase that
hydrates from `thread/settings/updated` while defining explicit precedence
between task state and the global preference.

### Invalid forced-on configuration

Hydex rejects `force_on` when no valid local offload provider is resolved.

Mitigation: preserve the normal app-server error and show it through the
desktop error path. Do not silently fall back to the primary provider, because
that would make the selector misleading.

## Rollout

1. Land the feature disabled by default.
2. Validate it against the current accepted DMG and a packaged Hydex binary.
3. Enable it in a private/local `features.json` build.
4. Exercise it across at least one upstream desktop update.
5. Consider repository-wide feature documentation or broader enablement only
   after the patch anchors have survived an update cycle.

## Deferred Enhancements

- A second dropdown for `modelOffloadCompactionOverride` with Auto, Local, and
  Primary values.
- Per-task dropdown hydration from `ThreadSettings` notifications.
- A Hydex capability/version handshake that can hide or disable the control
  when a vanilla CLI is selected.
- Native packaging that bundles or depends on a matching Hydex build.
- A richer status indicator showing whether the last request actually routed
  locally rather than only showing the requested override.

## Non-Goals

- Changing Hydex routing, provider validation, or persistence semantics.
- Adding Hydex-only code to upstream Codex Desktop sources.
- Bundling a Hydex executable into the first desktop feature.
- Automatically installing or configuring a local Responses provider.
- Silently treating failed forced-on routing as Auto or Off.
- Adding a general desktop settings page before the composer control has been
  validated.
