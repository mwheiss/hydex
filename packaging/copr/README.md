# Hydex COPR package

The COPR workflow builds RHEL and EPEL 7, 8, 9, and 10 x86_64 repository RPMs from the same
immutable public runtime bundle used by AUR. It repackages validated static executables and does
not compile Rust.

Build and locally validate the source and binary RPMs without publishing:

```bash
python3 packaging/copr/publish_copr_package.py \
  --archive packaging/release/dist/hydex-runtime-<version>-r<release>-x86_64-unknown-linux-musl.tar.gz \
  --checksum packaging/release/dist/hydex-runtime-<version>-r<release>-x86_64-unknown-linux-musl.tar.gz.sha256 \
  --package-release <package-release> \
  --build-local
```

Add `--publish` after configuring `~/.config/copr`. The publisher creates `mheiss/hydex` when it
does not exist, enables all eight RHEL/EPEL 7-10 x86_64 chroots, and submits the single validated
SRPM to the complete matrix. `--package-release` can advance an installed package for a
packaging-only change without changing the immutable runtime release.

The installed command surface contains both naming families:
`codex`/`hydex` and `codex-code-mode-host`/`hydex-code-mode-host`.

Generated specs, source RPMs, binary RPMs, and build scratch are ignored under
`packaging/copr/dist/`.
