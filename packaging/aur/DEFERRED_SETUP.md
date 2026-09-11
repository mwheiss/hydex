# Deferred AUR setup for `hydex-bin`

Use this runbook when AUR account registration is available again. The package automation is
already implemented and validated; no `PKGBUILD` or `.SRCINFO` version editing is needed.

## Current deferred state

- AUR registrations were suspended when initial publication was attempted on 2026-08-27.
- `hydex-bin` was unclaimed in the AUR RPC at that time. Recheck before the first push.
- The publisher is `packaging/aur/publish_aur_package.py` on Hydex `hydex/main`.
- The current validated source release is
  `hydex-runtime-v0.150.0-alpha.8-r1`, with archive SHA-256
  `27d2e8b1f5e2031e7e77bc6e30fd4e9ed41979d4316c25ac3a4bdbda640a4f3d`.
- The local Ed25519 key intended for AUR has fingerprint
  `SHA256:4HrlRq6ACrlxWNd6l+c6y/Wh9ZWEeaqFVsMpVWz4clk`.
- The official AUR Ed25519 host key has already been verified and trusted as
  `SHA256:RFzBCUItH9LZS0cKB5UE6ceAYhBD5C8GeOBip8Z11+4`.

## One-time account setup

1. Recheck that the package name remains unclaimed:

   ```bash
   curl -sS 'https://aur.archlinux.org/rpc/v5/info?arg[]=hydex-bin' | jq .
   ```

   Continue only when `resultcount` is `0`. If another maintained package now owns the name, do
   not create a duplicate. Follow the AUR adoption/request process only if it is genuinely
   abandoned.

2. Register or sign in at <https://aur.archlinux.org/>.

3. Open **My Account**, copy the complete output of this command into the SSH public-key field,
   and save the account:

   ```bash
   cat ~/.ssh/id_ed25519.pub
   ```

4. Confirm that AUR recognizes the key:

   ```bash
   ssh -o BatchMode=yes aur@aur.archlinux.org help
   ```

   The command should print the AUR SSH command list instead of `Permission denied (publickey)`.

## Initial publication

Use the most recent versioned runtime archive produced by the standard Hydex refresh. For the
currently validated release, run:

```bash
cd /home/mheiss/hydex
export TMPDIR=/home/mheiss/.cache/hydex-build/tmp
python3 packaging/aur/publish_aur_package.py \
  --archive packaging/release/dist/hydex-runtime-0.150.0-alpha.8-r1-x86_64-unknown-linux-musl.tar.gz \
  --checksum packaging/release/dist/hydex-runtime-0.150.0-alpha.8-r1-x86_64-unknown-linux-musl.tar.gz.sha256 \
  --build \
  --publish
```

The publisher will:

- verify the adjacent archive checksum and embedded provenance manifest;
- render `pkgver`, `pkgrel`, the immutable GitHub release URL, and SHA-256;
- regenerate `.SRCINFO` with `makepkg --printsrcinfo`;
- download and verify the public source with `makepkg --verifysource`;
- build `hydex-bin` and reject `namcap` errors;
- clone the separate `ssh://aur@aur.archlinux.org/hydex-bin.git` repository;
- commit only `PKGBUILD`, `.SRCINFO`, and the 0BSD packaging-recipe license;
- push AUR `master`, fetch it back, and require exact local/remote identity.

## Readback after publication

1. Open <https://aur.archlinux.org/packages/hydex-bin> and confirm the displayed version.
2. Re-run the AUR RPC query and inspect its `Version` and `URLPath` fields.
3. Clone the public package repository into a temporary directory and verify its source URL and
   checksum:

   ```bash
   git clone https://aur.archlinux.org/hydex-bin.git
   cd hydex-bin
   makepkg --verifysource
   makepkg --printsrcinfo | diff -u .SRCINFO -
   ```

## Future releases

No manual AUR changes are required. Whenever the Hydex runtime changes, the standard refresh
publishes a new immutable GitHub runtime release and then invokes the AUR publisher. Plugin-only
VSIX refreshes do not touch AUR.

Do not introduce a mutable `latest` source into `PKGBUILD`. AUR helpers discover upgrades through
the committed `pkgver` and `pkgrel` in `.SRCINFO`; the versioned tag, filename, and checksum keep
every historical build reproducible.

References: [AUR submission guidelines](https://wiki.archlinux.org/title/AUR_submission_guidelines)
and [.SRCINFO](https://wiki.archlinux.org/title/.SRCINFO).
