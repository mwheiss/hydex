# Hydex AUR package

`hydex-bin` repackages the immutable public Hydex runtime release for Arch Linux and CachyOS. It
does not compile Rust and never downloads the private VSIX.

Render, verify, build, and inspect the package without publishing:

```bash
python3 packaging/aur/publish_aur_package.py \
  --archive packaging/release/dist/hydex-runtime-<version>-r<release>-x86_64-unknown-linux-musl.tar.gz \
  --checksum packaging/release/dist/hydex-runtime-<version>-r<release>-x86_64-unknown-linux-musl.tar.gz.sha256 \
  --build
```

Add `--publish` after configuring an AUR account and SSH key. The publisher clones the separate
`hydex-bin` AUR repository, commits only `PKGBUILD`, `.SRCINFO`, and the 0BSD packaging-recipe
license, pushes `master`, and verifies the remote commit.

Every package version pins a namespaced GitHub release URL and SHA-256. Do not use a mutable
`latest` asset in `PKGBUILD`: AUR helpers detect upgrades from committed `pkgver` and `pkgrel` in
`.SRCINFO`, which the publisher regenerates for every runtime release.
