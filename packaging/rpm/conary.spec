%global crate conary

Name:           conary
Version:        0.7.0
Release:        1%{?dist}
Summary:        Modern package manager with atomic operations, rollback, and delta updates

License:        MIT OR Apache-2.0
URL:            https://github.com/ConaryLabs/Conary
Source0:        %{crate}-%{version}.tar.gz
Source1:        vendor.tar.gz

BuildRequires:  openssl-devel
BuildRequires:  xz-devel
BuildRequires:  pkg-config
BuildRequires:  cmake
BuildRequires:  perl

Requires:       openssl-libs
Requires:       xz-libs

ExclusiveArch:  x86_64 aarch64

%description
Conary is a modern package manager that brings atomic transactions,
automatic rollback, and delta updates to Linux systems. It works
alongside existing package managers (DNF, APT, pacman), tracking their
packages and providing a unified interface with enhanced capabilities.

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
cargo build --release --locked -p conary

%install
install -Dpm 0755 target/release/%{crate} %{buildroot}%{_bindir}/%{crate}

# Man page
install -Dpm 0644 apps/conary/man/%{crate}.1 %{buildroot}%{_mandir}/man1/%{crate}.1

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

# License files
install -Dpm 0644 LICENSE-MIT %{buildroot}%{_datadir}/licenses/%{crate}/LICENSE-MIT
install -Dpm 0644 LICENSE-APACHE %{buildroot}%{_datadir}/licenses/%{crate}/LICENSE-APACHE

%post
# Initialize the Conary database and seed default repos (including Remi CCS proxy).
# Safe to re-run: add_repository checks uniqueness.
%{_bindir}/%{crate} system init 2>/dev/null || :

%files
%license LICENSE-MIT LICENSE-APACHE
%doc README.md
%{_bindir}/%{crate}
%{_mandir}/man1/%{crate}.1*
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
