# Hydex RPM packages

Build the Linux x86_64 Hydex runtime for RHEL 7 or RHEL 10:

```bash
packaging/rpm/build-rhel7-package.sh
packaging/rpm/build-rhel10-package.sh
```

Both wrappers use the shared `build-rpm-package.sh` helper and the latest refreshed Linux x64 plugin baseline under
`hydex-plugin/unpacked/`. It verifies that the Hydex CLI, code-mode host,
bundled ripgrep, and bundled bubblewrap are static PIE executables and that the
CLI version matches `codex-package.json`. The finished RPM is also rejected if
RPM discovers any host runtime dependency beyond RPM payload capabilities.

The shared builder also accepts `--runtime-root PATH` for a canonical surface
runtime. `packaging/build-preferred-local-packages.sh` compares the VS Code and
Codex Desktop runtime versions and uses the newer one for both RHEL packages
and the Arch package.

The RHEL 7 wrapper forces RPM v4 with a cpio/gzip payload, rejects RPM features
newer than RPM 4.11, and validates a test installation plus CLI help/version in
the official UBI 7.9 image. That proves RPM-format and RHEL 7 userland
compatibility in a container; perform a final smoke test on the deployment
host to cover its RHEL 7 kernel, SELinux policy, and sandbox configuration.

The package installs the canonical Codex package layout under
`/usr/libexec/hydex`, with entry points at `/usr/bin/codex` and
`/usr/bin/codex-code-mode-host`. It also installs shell completions and the
repository license.

Install or update the RHEL 7 package with:

```bash
sudo yum install ./hydex-<version>-1.el7.x86_64.rpm
sudo yum update ./hydex-<version>-1.el7.x86_64.rpm
```

Install or update the RHEL 10 package with:

```bash
sudo dnf install ./hydex-<version>-1.el10.x86_64.rpm
sudo dnf upgrade ./hydex-<version>-1.el10.x86_64.rpm
```

The musl static executables avoid a glibc-version dependency. Runtime behavior
still depends on the host kernel, user namespaces and SELinux policy for
sandboxing, and normal system resources such as CA certificates and Git.
