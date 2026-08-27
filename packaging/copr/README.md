# Hydex COPR package

The COPR workflow builds EL7, RHEL 9, and RHEL 10 repository RPMs from the same immutable public
runtime bundle used by AUR. It repackages validated static executables and does not compile Rust.

Build and locally validate the source and binary RPMs without publishing:

```bash
python3 packaging/copr/publish_copr_package.py \
  --archive packaging/release/dist/hydex-runtime-<version>-r<release>-x86_64-unknown-linux-musl.tar.gz \
  --checksum packaging/release/dist/hydex-runtime-<version>-r<release>-x86_64-unknown-linux-musl.tar.gz.sha256 \
  --build-local
```

Add `--publish` after configuring `~/.config/copr`. The publisher creates `mheiss/hydex` when it
does not exist, enabling `epel-7-x86_64`, `rhel-9-x86_64`, and `rhel-10-x86_64`, then submits the
single validated SRPM to all three chroots.

Generated specs, source RPMs, binary RPMs, and build scratch are ignored under
`packaging/copr/dist/`.
