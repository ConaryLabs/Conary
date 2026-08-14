# Suite releases publish one installable RPM and no separate debug artifact;
# the workspace release profile owns stripping, so do not generate discarded subpackages.
%global debug_package %{nil}

# Fedora's phase prelude otherwise exports Rust flags whose debug and strip
# settings exist to feed split debuginfo. Reapply the distro build flags below
# after selecting only the Rust additions that remain part of this package.
%undefine _auto_set_build_flags
%global crate conary

Name:           conary
Version:        0.15.0
Release:        1%{?dist}
Summary:        Early-preview Linux package manager with native-package adoption

License:        MIT
URL:            https://github.com/ConaryLabs/Conary
Source0:        %{crate}-%{version}.tar.gz
Source1:        vendor.tar.gz

BuildRequires:  openssl-devel
BuildRequires:  xz-devel
BuildRequires:  libseccomp-devel
BuildRequires:  pkg-config
BuildRequires:  cmake
BuildRequires:  perl
BuildRequires:  systemd-rpm-macros

Requires:       openssl-libs
Requires:       xz-libs
Requires:       libseccomp

ExclusiveArch:  x86_64

%description
Conary is an early-preview Linux package manager for tracking and
reversibly adopting packages from DNF, APT, and pacman. The preview also
offers guarded dry-run and apply workflows for package changes.

%prep
%setup -q -n %{crate}-%{version}
%setup -q -T -D -a 1 -n %{crate}-%{version}

mkdir -p .cargo
cat > .cargo/config.toml <<'EOF'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
EOF

%build
# Cargo.toml owns release optimization, LTO, codegen units, and stripping.
# Retain Fedora's x86_64 frame pointers and embedded RPM package note, while
# the manual distro macro below populates native dependency toolchain flags.
RUSTFLAGS="-Cforce-frame-pointers=yes -Clink-arg=%{_package_note_flags}"
export RUSTFLAGS
%set_build_flags
echo "Conary effective RUSTFLAGS: $RUSTFLAGS"
cargo build --release --locked -p conary

%install
install -Dpm 0755 target/release/%{crate} %{buildroot}%{_bindir}/%{crate}

# Man page
install -Dpm 0644 apps/conary/man/%{crate}.1 %{buildroot}%{_mandir}/man1/%{crate}.1

# Booted-generation activation
install -Dpm 0644 packaging/systemd/%{crate}-generation-activation.service \
    %{buildroot}%{_unitdir}/%{crate}-generation-activation.service
install -d %{buildroot}%{_unitdir}/multi-user.target.wants
ln -s ../%{crate}-generation-activation.service \
    %{buildroot}%{_unitdir}/multi-user.target.wants/%{crate}-generation-activation.service

# Shell completions
install -d %{buildroot}%{_datadir}/bash-completion/completions
install -d %{buildroot}%{_datadir}/zsh/site-functions
install -d %{buildroot}%{_datadir}/fish/vendor_completions.d
target/release/%{crate} system completions bash > %{buildroot}%{_datadir}/bash-completion/completions/%{crate}
target/release/%{crate} system completions zsh  > %{buildroot}%{_datadir}/zsh/site-functions/_%{crate}
target/release/%{crate} system completions fish > %{buildroot}%{_datadir}/fish/vendor_completions.d/%{crate}.fish

# Config and data directories
install -d %{buildroot}%{_sysconfdir}/%{crate}
install -d %{buildroot}%{_sharedstatedir}/%{crate}

# License
install -Dpm 0644 LICENSE %{buildroot}%{_datadir}/licenses/%{crate}/LICENSE

%post
# Initialize the Conary database and seed default repos (including Remi CCS proxy).
# Safe to re-run: init reconciles managed defaults without replacing user-managed endpoints.
%{_bindir}/%{crate} system init

%files
%license LICENSE
%doc README.md
%{_bindir}/%{crate}
%{_mandir}/man1/%{crate}.1*
%{_unitdir}/%{crate}-generation-activation.service
%{_unitdir}/multi-user.target.wants/%{crate}-generation-activation.service
%{_datadir}/bash-completion/completions/%{crate}
%{_datadir}/zsh/site-functions/_%{crate}
%{_datadir}/fish/vendor_completions.d/%{crate}.fish
%dir %{_sysconfdir}/%{crate}
%dir %{_sharedstatedir}/%{crate}

%changelog
* Tue Mar 03 2026 Conary Contributors <contributors@conary.io> - 0.1.0-1
- Initial RPM package
- Pre-configured with Remi CCS repository
- Shell completions for bash, zsh, fish
- Man page
