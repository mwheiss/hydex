# Replay Validation

Run from the replay's `codex-rs` directory. Follow the repository `AGENTS.md`: use `just`, never
direct `cargo test`; run `just fmt` after changes; use scoped `just fix` for changed crates and do
not rerun tests after the final fix/format pass.

## Generated outputs

Regenerate only what the change requires:

```bash
PATH=/home/mheiss/.local/bin:/home/mheiss/.cargo/bin:$PATH just write-config-schema
PATH=/home/mheiss/.local/bin:/home/mheiss/.cargo/bin:$PATH just write-app-server-schema
```

If the upstream `just write-app-server-schema` recipe is stale, use the checked-in Python schema
fixture driver named by the current repository rather than inventing a replacement. If Rust
dependencies changed, run `just bazel-lock-update` and include `MODULE.bazel.lock`.

## Focused Hydex coverage

At minimum, run the current equivalents of:

```bash
PATH=/home/mheiss/.local/bin:/home/mheiss/.cargo/bin:$PATH just test -p codex-core offload
PATH=/home/mheiss/.local/bin:/home/mheiss/.cargo/bin:$PATH just test -p codex-core compaction_recovery
PATH=/home/mheiss/.local/bin:/home/mheiss/.cargo/bin:$PATH just test -p codex-app-server-protocol schema_fixtures
PATH=/home/mheiss/.local/bin:/home/mheiss/.cargo/bin:$PATH just test -p codex-app-server model_offload
```

Also test any crate whose source changed, validate rollout reconstruction when protocol/history
logic moved, and run configured CLI/core checks. Run a complete `just test` only with the user's
approval when common, core, or protocol changes require it.

Use `TMPDIR=/home/mheiss/.cache/hydex-build/tmp`. App-server tests that need local listeners may
require the authorized host path.

## Known upstream blocker handling

If full workspace checks fail only because an upstream `rusty_v8` prebuilt archive is unavailable,
record the exact version, URL/status, and affected command. Run and report focused non-V8 checks
separately. Never call that a full workspace pass.

## Final source audit

```bash
PATH=/home/mheiss/.local/bin:/home/mheiss/.cargo/bin:$PATH just fix -p <changed-crate>
PATH=/home/mheiss/.local/bin:/home/mheiss/.cargo/bin:$PATH just fmt
git status --short
git diff --check
git diff --stat "$BASE_ANCHOR"..HEAD
```

Review the replay commit stack and confirm that generated files, config schema, Cargo/Bazel locks,
and upstream README mirror are consistent. Put every intended source/packaging change on the replay
tip before publication; never commit it on the stale canonical checkout.
