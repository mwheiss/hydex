# Hydex runtime release bundle

Build the common public Linux x86_64 runtime consumed by AUR and COPR from an explicitly
validated plugin baseline:

```bash
python3 packaging/release/build_runtime_bundle.py \
  --baseline openai-chatgpt-<extension-version>-linux-x64 \
  --release <positive-integer> \
  --hydex-commit <full-hydex-source-commit>
```

For the combined VS Code/Desktop workflow, first normalize each surface with
`prepare_surface_runtime.py`, compare them with `select_surface_runtime.py`,
and pass the selected newer root directly:

```bash
python3 packaging/release/build_runtime_bundle.py \
  --runtime-root <selected-runtime-root> \
  --release <positive-integer> \
  --hydex-commit <full-hydex-source-commit>
```

`packaging/build-preferred-local-packages.sh` uses the same selection rule for
the local Arch, RHEL 7, and RHEL 10 packages. If plugin and desktop versions
are equal, the plugin runtime is the deterministic tie-break.

The deterministic `.tar.gz` contains the stripped static Hydex CLI, matching code-mode host,
ripgrep, bubblewrap, canonical `codex-package.json`, component licenses, an internal
`SHA256SUMS`, and a provenance manifest. The adjacent `.sha256` authenticates the complete
archive. Generated output under `packaging/release/dist/` is ignored.

Release tags use this namespace:

```text
hydex-runtime-v<codex-version>-r<release>
```

Publish the two versioned files on the public `mwheiss/hydex` GitHub repository only after the
source and packaging commits are reachable there. Refuse partial or mismatched existing tag or
release state, never overwrite an asset, and download both uploaded assets for digest verification.
Never include the private patched VSIX or any unpacked extension files in this public bundle.
