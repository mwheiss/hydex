# Local Arch Package

Build a pacman-managed Linux x64 Hydex package from the current refreshed VS Code plugin
workspace:

```bash
./packaging/arch/build-local-package.sh
```

The helper selects the newest unpacked Linux x64 plugin baseline by default. It verifies that the
bundled CLI is a static Hydex build with the offload flags, pairs it with the matching
`codex-code-mode-host`, verifies both source hashes, and creates:

```text
packaging/arch/hydex-bin-<version>-1-x86_64.pkg.tar.zst
```

Install or upgrade it with the exact command printed by the helper:

```bash
sudo pacman -U packaging/arch/hydex-bin-<version>-1-x86_64.pkg.tar.zst
```

The package declares that it replaces and conflicts with `openai-codex-bin`, so pacman performs
the replacement without a separate removal step. It owns `/usr/bin/codex`,
`/usr/bin/codex-code-mode-host`, shell completions, the license, and Hydex build metadata.

To package a specific unpacked plugin baseline:

```bash
./packaging/arch/build-local-package.sh \
  --baseline openai-chatgpt-<extension-version>-linux-x64
```

Generated package, `src/`, and `pkg/` files are ignored by Git. Remove the local installation with:

```bash
sudo pacman -R hydex-bin
```
