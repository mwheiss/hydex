---
name: hydex-plugin-refresh
description: Refresh the private Hydex VS Code plugin patch workspace with the current Hydex CLI release binary. Use when updating or rebuilding the Linux x64 patched VSIX, checking that the bundled binary exposes Hydex flags, validating patched webview assets, or reporting plugin artifact hashes.
---

# Hydex Plugin Refresh

Use this skill to rebuild the current Hydex CLI and refresh the private Linux x64 VS Code VSIX patch workspace at `hydex-plugin/`.

## Workflow

1. Check both repositories:

   ```bash
   git status --short --branch
   git -C hydex-plugin status --short --branch
   git log --oneline -1
   ```

   Treat `hydex-plugin/` as a separate git repo. Do not add it to the parent Hydex repo.

2. Build the current Hydex release binary from `codex-rs/`:

   ```bash
   cargo build -p codex-cli --release
   ```

   If a dependency download or other network-like failure is caused by sandboxing, rerun outside the sandbox with approval. A warning from `codex-app-server` about `unused_mut` may already be present; do not fix unrelated warnings in this workflow.

3. Run the helper script:

   ```bash
   .codex/skills/hydex-plugin-refresh/scripts/refresh_hydex_plugin.py
   ```

   The script patches `hydex-plugin/unpacked/openai-chatgpt-26.5623.141536-linux-x64/extension`, repacks `hydex-plugin/dist/hydex-chatgpt-26.5623.141536-linux-x64.vsix`, and validates the result.

4. Report:

   - Hydex commit used.
   - VSIX path.
   - release binary hash before strip.
   - bundled stripped binary hash.
   - patched VSIX hash.
   - validation results.
   - whether `hydex-plugin` has tracked changes to commit.

## Important Details

- The current plugin baseline is Linux x64 only: `openai-chatgpt-26.5623.141536-linux-x64.vsix`.
- `dist/` and `unpacked/` are intentionally ignored by `hydex-plugin`; a refresh may produce no git changes.
- The patch script replaces `extension/bin/linux-x86_64/codex` while preserving the upstream filename as the extension launch target.
- The patch must preserve the separate identity `mwheiss.hydex` and must not restore upstream `openai.chatgpt`.
- The patched webview must keep sending `modelOffloadOverride` on `turn/start` and `thread/settings/update`.
- The bundled CLI help should include `--offload` and `--no-offload`.

## Validation Commands

The helper script runs these checks:

```bash
unzip -t hydex-plugin/dist/hydex-chatgpt-26.5623.141536-linux-x64.vsix
node --check hydex-plugin/unpacked/openai-chatgpt-26.5623.141536-linux-x64/extension/webview/assets/composer-ChmJYQgb.js
node --check hydex-plugin/unpacked/openai-chatgpt-26.5623.141536-linux-x64/extension/webview/assets/thread-context-inputs-pO1QHnPh.js
hydex-plugin/unpacked/openai-chatgpt-26.5623.141536-linux-x64/extension/bin/linux-x86_64/codex --help
```

If asset filenames change in a future upstream VSIX, inspect the new bundle and update `scripts/apply_hydex_patch.py` rather than weakening validation.
