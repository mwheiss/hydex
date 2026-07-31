# Hydex RPM package

Build the Linux x86_64 Hydex runtime as a RHEL 10 RPM:

```bash
packaging/rpm/build-rhel10-package.sh
```

The helper uses the latest refreshed Linux x64 plugin baseline under
`hydex-plugin/unpacked/`. It verifies that the Hydex CLI, code-mode host,
bundled ripgrep, and bundled bubblewrap are static PIE executables and that the
CLI version matches `codex-package.json`. The finished RPM is also rejected if
RPM discovers any host runtime dependency beyond RPM payload capabilities.

The package installs the canonical Codex package layout under
`/usr/libexec/hydex`, with entry points at `/usr/bin/codex` and
`/usr/bin/codex-code-mode-host`. It also installs shell completions and the
repository license.

Install or update the resulting package on RHEL 10 with:

```bash
sudo dnf install ./hydex-<version>-1.el10.x86_64.rpm
sudo dnf upgrade ./hydex-<version>-1.el10.x86_64.rpm
```

The musl static executables avoid a glibc-version dependency. Runtime behavior
still depends on the host kernel, user namespaces and SELinux policy for
sandboxing, and normal system resources such as CA certificates and Git.
