%{!?hydex_version:%global hydex_version 0}
%{!?hydex_plugin_baseline:%global hydex_plugin_baseline unknown}
%{!?hydex_codex_sha256:%global hydex_codex_sha256 unknown}
%{!?hydex_code_mode_host_sha256:%global hydex_code_mode_host_sha256 unknown}

Name:           hydex
Version:        %{hydex_version}
Release:        1%{?dist}
Summary:        Codex CLI with Hydex local model offload
License:        Apache-2.0
URL:            https://github.com/mwheiss/hydex
BuildArch:      x86_64

Source0:        codex
Source1:        codex-code-mode-host
Source2:        rg
Source3:        bwrap
Source4:        codex-package.json
Source5:        LICENSE

Provides:       codex = %{version}-%{release}
Provides:       openai-codex = %{version}-%{release}
Conflicts:      openai-codex

%description
Hydex is a patch line for Codex CLI that retains OpenAI/Codex as the primary
control plane while optionally routing eligible inference to a local
Responses-compatible model endpoint.

This package uses the canonical Codex runtime layout and bundles the matching
static Linux x86_64 CLI, code-mode host, ripgrep, and bubblewrap executables.

%prep

%build

%install
install -d \
  %{buildroot}%{_libexecdir}/hydex/bin \
  %{buildroot}%{_libexecdir}/hydex/codex-path \
  %{buildroot}%{_libexecdir}/hydex/codex-resources \
  %{buildroot}%{_bindir} \
  %{buildroot}%{_datadir}/bash-completion/completions \
  %{buildroot}%{_datadir}/elvish/lib \
  %{buildroot}%{_datadir}/fish/vendor_completions.d \
  %{buildroot}%{_datadir}/powershell/Completions \
  %{buildroot}%{_datadir}/zsh/site-functions \
  %{buildroot}%{_datadir}/hydex \
  %{buildroot}%{_licensedir}/%{name}

install -m 0755 %{SOURCE0} %{buildroot}%{_libexecdir}/hydex/bin/codex
install -m 0755 %{SOURCE1} %{buildroot}%{_libexecdir}/hydex/bin/codex-code-mode-host
install -m 0755 %{SOURCE2} %{buildroot}%{_libexecdir}/hydex/codex-path/rg
install -m 0755 %{SOURCE3} %{buildroot}%{_libexecdir}/hydex/codex-resources/bwrap
install -m 0644 %{SOURCE4} %{buildroot}%{_libexecdir}/hydex/codex-package.json
install -m 0644 %{SOURCE5} %{buildroot}%{_licensedir}/%{name}/LICENSE

ln -s ../libexec/hydex/bin/codex %{buildroot}%{_bindir}/codex
ln -s ../libexec/hydex/bin/codex-code-mode-host \
  %{buildroot}%{_bindir}/codex-code-mode-host

%{SOURCE0} completion bash \
  > %{buildroot}%{_datadir}/bash-completion/completions/codex
%{SOURCE0} completion elvish \
  > %{buildroot}%{_datadir}/elvish/lib/codex.elv
%{SOURCE0} completion fish \
  > %{buildroot}%{_datadir}/fish/vendor_completions.d/codex.fish
%{SOURCE0} completion powershell \
  > %{buildroot}%{_datadir}/powershell/Completions/codex.ps1
%{SOURCE0} completion zsh \
  > %{buildroot}%{_datadir}/zsh/site-functions/_codex

cat > %{buildroot}%{_datadir}/hydex/build-info <<EOF
plugin_baseline=%{hydex_plugin_baseline}
codex_version=%{version}
codex_sha256=%{hydex_codex_sha256}
code_mode_host_sha256=%{hydex_code_mode_host_sha256}
EOF

%files
%license %{_licensedir}/%{name}/LICENSE
%{_bindir}/codex
%{_bindir}/codex-code-mode-host
%{_libexecdir}/hydex/
%{_datadir}/bash-completion/completions/codex
%{_datadir}/elvish/lib/codex.elv
%{_datadir}/fish/vendor_completions.d/codex.fish
%{_datadir}/powershell/Completions/codex.ps1
%{_datadir}/zsh/site-functions/_codex
%{_datadir}/hydex/build-info

%changelog
* Fri Jul 31 2026 Michael W. Heiss <mheiss@users.noreply.github.com> - %{version}-1
- Package Hydex using the canonical Codex runtime layout
