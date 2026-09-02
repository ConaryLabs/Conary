// crates/conary-core/src/repository/architecture.rs

//! Pinned source-derived package architecture authority.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use crate::error::{Error, Result};
use crate::repository::versioning::VersionScheme;

const RPM_TABLES: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/architecture/rpm-6.0.1-rpmrc-architecture-tables.txt"
));
const DPKG_CPUTABLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/architecture/dpkg-1.23.7-cputable"
));
const DPKG_TUPLETABLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/architecture/dpkg-1.23.7-tupletable"
));
const ARCH_TABLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/architecture/arch-2026-08-02-architecture-authority.txt"
));

/// Endianness stated by dpkg's pinned `cputable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NativeMachineEndiannessV1 {
    Big,
    Little,
}

/// Exact machine identity projected from the owning pinned architecture table.
///
/// The variants deliberately retain the dimensions each upstream authority
/// actually publishes. `CrossScheme` is used only where those published fields
/// justify one complete identity across package schemes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum NativeMachineIdentityV1 {
    CrossScheme {
        cpu: String,
        cpu_bits: u16,
        endianness: NativeMachineEndiannessV1,
        abi: String,
        libc: String,
        os: String,
    },
    Dpkg {
        cpu: String,
        cpu_bits: u16,
        endianness: NativeMachineEndiannessV1,
        abi: String,
        libc: String,
        os: String,
    },
    Rpm {
        canonical_architecture: String,
    },
    Makepkg {
        carch: String,
    },
    Conary {
        architecture: String,
    },
}

/// Three-valued native-only architecture admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeResolutionArchitectureDecisionV1 {
    Admitted,
    Excluded {
        identity: NativeMachineIdentityV1,
    },
    UnknownArchitectureToken {
        scheme: VersionScheme,
        token: String,
    },
}

impl NativeResolutionArchitectureDecisionV1 {
    #[must_use]
    pub const fn is_admitted(&self) -> bool {
        matches!(self, Self::Admitted)
    }

    pub fn into_result(self) -> Result<Self> {
        match self {
            Self::UnknownArchitectureToken { scheme, token } => {
                Err(Error::UnknownArchitectureToken {
                    scheme: scheme.as_str().to_string(),
                    token,
                })
            }
            decision => Ok(decision),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KnownPackageArchitecture {
    Independent,
    Machine(NativeMachineIdentityV1),
}

#[derive(Debug)]
struct DpkgCpu {
    gnu_name: String,
    bits: u16,
    endianness: NativeMachineEndiannessV1,
}

#[derive(Debug)]
struct ArchitectureAuthority {
    rpm: BTreeMap<String, KnownPackageArchitecture>,
    debian: BTreeMap<String, KnownPackageArchitecture>,
    arch: BTreeMap<String, KnownPackageArchitecture>,
}

static ARCHITECTURE_AUTHORITY: OnceLock<ArchitectureAuthority> = OnceLock::new();

fn authority() -> &'static ArchitectureAuthority {
    ARCHITECTURE_AUTHORITY.get_or_init(|| ArchitectureAuthority {
        rpm: parse_rpm_authority(),
        debian: parse_dpkg_authority(),
        arch: parse_makepkg_authority(),
    })
}

/// Reject a package architecture that is absent from its pinned source table.
pub fn require_known_package_architecture(scheme: VersionScheme, token: &str) -> Result<()> {
    if known_package_architecture(scheme, token).is_some() {
        return Ok(());
    }
    Err(Error::UnknownArchitectureToken {
        scheme: scheme.as_str().to_string(),
        token: token.to_string(),
    })
}

/// Decide native-only admission without collapsing an unknown token to false.
#[must_use]
pub fn native_resolution_architecture_decision(
    scheme: VersionScheme,
    package_token: &str,
    target_token: &str,
) -> NativeResolutionArchitectureDecisionV1 {
    let Some(package) = known_package_architecture(scheme, package_token) else {
        return NativeResolutionArchitectureDecisionV1::UnknownArchitectureToken {
            scheme,
            token: package_token.to_string(),
        };
    };
    let Some(target_identity) = target_machine_identity(scheme, target_token) else {
        return NativeResolutionArchitectureDecisionV1::UnknownArchitectureToken {
            scheme,
            token: target_token.to_string(),
        };
    };
    let identity = match package {
        KnownPackageArchitecture::Independent => target_identity.clone(),
        KnownPackageArchitecture::Machine(identity) => identity,
    };
    if identity == target_identity {
        NativeResolutionArchitectureDecisionV1::Admitted
    } else {
        NativeResolutionArchitectureDecisionV1::Excluded { identity }
    }
}

pub(crate) fn known_package_architecture(
    scheme: VersionScheme,
    token: &str,
) -> Option<KnownPackageArchitecture> {
    match scheme {
        VersionScheme::Rpm => authority().rpm.get(token).cloned(),
        VersionScheme::Debian => authority().debian.get(token).cloned(),
        VersionScheme::Arch => authority().arch.get(token).cloned(),
        VersionScheme::Conary => conary_architecture(token),
        VersionScheme::Eopkg => None,
    }
}

fn target_machine_identity(scheme: VersionScheme, token: &str) -> Option<NativeMachineIdentityV1> {
    match known_package_architecture(scheme, token) {
        Some(KnownPackageArchitecture::Machine(identity)) => Some(identity),
        Some(KnownPackageArchitecture::Independent) => None,
        None => host_machine_identity(token),
    }
}

pub(crate) fn host_machine_identity(token: &str) -> Option<NativeMachineIdentityV1> {
    match token {
        "x86_64" | "amd64" => Some(shared_identity("x86_64")),
        "x86" | "i386" | "i686" => Some(shared_identity("i686")),
        "aarch64" | "arm64" => Some(shared_identity("aarch64")),
        "powerpc64le" | "ppc64le" | "ppc64el" => Some(shared_identity("powerpc64le")),
        "s390x" => Some(shared_identity("s390x")),
        "riscv64" => Some(shared_identity("riscv64")),
        _ => None,
    }
}

fn parse_rpm_authority() -> BTreeMap<String, KnownPackageArchitecture> {
    let mut architectures = BTreeMap::new();
    for line in data_lines(RPM_TABLES) {
        let fields = line.split(':').map(str::trim).collect::<Vec<_>>();
        match fields.first().copied() {
            Some("arch_canon") => {
                let input = fields[1];
                let canonical = fields[2]
                    .split_whitespace()
                    .next()
                    .expect("rpm arch_canon fixture has a canonical architecture");
                let identity = rpm_identity(canonical);
                architectures.insert(input.to_string(), identity.clone());
                architectures
                    .entry(canonical.to_string())
                    .or_insert(identity);
            }
            Some("arch_compat" | "buildarch_compat") => {
                for token in std::iter::once(fields[1]).chain(fields[2].split_whitespace()) {
                    architectures
                        .entry(token.to_string())
                        .or_insert_with(|| rpm_identity(token));
                }
            }
            _ => panic!("unexpected RPM architecture fixture line: {line}"),
        }
    }
    architectures
}

fn rpm_identity(canonical: &str) -> KnownPackageArchitecture {
    if canonical == "noarch" {
        return KnownPackageArchitecture::Independent;
    }
    let identity = match canonical {
        "x86_64" => shared_identity("x86_64"),
        "i686" => shared_identity("i686"),
        "aarch64" => shared_identity("aarch64"),
        "ppc64le" => shared_identity("powerpc64le"),
        "s390x" => shared_identity("s390x"),
        "riscv64" => shared_identity("riscv64"),
        exact => NativeMachineIdentityV1::Rpm {
            canonical_architecture: exact.to_string(),
        },
    };
    KnownPackageArchitecture::Machine(identity)
}

fn parse_dpkg_authority() -> BTreeMap<String, KnownPackageArchitecture> {
    let cpus = data_lines(DPKG_CPUTABLE)
        .map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            let endianness = match fields[4] {
                "big" => NativeMachineEndiannessV1::Big,
                "little" => NativeMachineEndiannessV1::Little,
                value => panic!("unexpected dpkg cputable endianness: {value}"),
            };
            (
                fields[0].to_string(),
                DpkgCpu {
                    gnu_name: fields[1].to_string(),
                    bits: fields[3]
                        .parse()
                        .expect("dpkg cputable pointer bits are numeric"),
                    endianness,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();

    let mut architectures = BTreeMap::new();
    architectures.insert("all".to_string(), KnownPackageArchitecture::Independent);
    for line in data_lines(DPKG_TUPLETABLE) {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        let tuple_template = fields[0];
        let output_template = fields[1];
        if tuple_template.contains("<cpu>") || output_template.contains("<cpu>") {
            for (cpu_name, cpu) in &cpus {
                let tuple = tuple_template.replace("<cpu>", cpu_name);
                let output = output_template.replace("<cpu>", cpu_name);
                architectures
                    .entry(output)
                    .or_insert_with(|| dpkg_identity(&tuple, cpu_name, cpu));
            }
        } else {
            let cpu_name = tuple_template
                .rsplit('-')
                .next()
                .expect("dpkg tuple has a CPU component");
            // tupletable's historical FreeBSD spelling is `riscv`; cputable
            // owns its machine dimensions under the `riscv64` CPU row.
            let cpu_lookup_name = if cpu_name == "riscv" {
                "riscv64"
            } else {
                cpu_name
            };
            let cpu = cpus
                .get(cpu_lookup_name)
                .unwrap_or_else(|| panic!("dpkg tuple references unknown CPU '{cpu_name}'"));
            architectures
                .entry(output_template.to_string())
                .or_insert_with(|| dpkg_identity(tuple_template, cpu_name, cpu));
        }
    }
    architectures
}

fn dpkg_identity(tuple: &str, cpu_name: &str, cpu: &DpkgCpu) -> KnownPackageArchitecture {
    let fields = tuple.splitn(4, '-').collect::<Vec<_>>();
    assert_eq!(fields.len(), 4, "dpkg tuple must contain four components");
    let (abi, libc, os) = (fields[0], fields[1], fields[2]);
    let identity = match shared_dpkg_cpu(cpu_name, abi, libc, os) {
        Some(shared) => shared,
        None => NativeMachineIdentityV1::Dpkg {
            cpu: cpu.gnu_name.clone(),
            cpu_bits: cpu.bits,
            endianness: cpu.endianness,
            abi: abi.to_string(),
            libc: libc.to_string(),
            os: os.to_string(),
        },
    };
    KnownPackageArchitecture::Machine(identity)
}

fn shared_dpkg_cpu(
    cpu_name: &str,
    abi: &str,
    libc: &str,
    os: &str,
) -> Option<NativeMachineIdentityV1> {
    if (abi, libc, os) != ("base", "gnu", "linux") {
        return None;
    }
    match cpu_name {
        "amd64" => Some(shared_identity("x86_64")),
        "i386" => Some(shared_identity("i686")),
        "arm64" => Some(shared_identity("aarch64")),
        "ppc64el" => Some(shared_identity("powerpc64le")),
        "s390x" => Some(shared_identity("s390x")),
        "riscv64" => Some(shared_identity("riscv64")),
        _ => None,
    }
}

fn parse_makepkg_authority() -> BTreeMap<String, KnownPackageArchitecture> {
    let carch = data_lines(ARCH_TABLE)
        .find_map(|line| line.strip_prefix("CARCH="))
        .expect("makepkg fixture declares CARCH");
    let mut architectures = BTreeMap::new();
    architectures.insert(carch.to_string(), makepkg_identity(carch));
    for token in data_lines(ARCH_TABLE).filter_map(|line| line.strip_prefix("PACKAGE_ARCH=")) {
        let identity = if token == "any" {
            KnownPackageArchitecture::Independent
        } else {
            makepkg_identity(token)
        };
        architectures.insert(token.to_string(), identity);
    }
    architectures
}

fn makepkg_identity(carch: &str) -> KnownPackageArchitecture {
    let identity = match carch {
        "x86_64" => shared_identity("x86_64"),
        exact => NativeMachineIdentityV1::Makepkg {
            carch: exact.to_string(),
        },
    };
    KnownPackageArchitecture::Machine(identity)
}

fn conary_architecture(token: &str) -> Option<KnownPackageArchitecture> {
    use KnownPackageArchitecture::{Independent, Machine};
    let identity = match token {
        "noarch" => return Some(Independent),
        "x86_64" => shared_identity("x86_64"),
        "i686" => shared_identity("i686"),
        "aarch64" => shared_identity("aarch64"),
        "ppc64le" => shared_identity("powerpc64le"),
        "s390x" => shared_identity("s390x"),
        "riscv64" => shared_identity("riscv64"),
        exact @ "armv7" => NativeMachineIdentityV1::Conary {
            architecture: exact.to_string(),
        },
        _ => return None,
    };
    Some(Machine(identity))
}

fn shared_identity(cpu: &str) -> NativeMachineIdentityV1 {
    let (cpu_bits, endianness) = match cpu {
        "x86_64" | "aarch64" | "powerpc64le" | "riscv64" => (64, NativeMachineEndiannessV1::Little),
        "i686" => (32, NativeMachineEndiannessV1::Little),
        "s390x" => (64, NativeMachineEndiannessV1::Big),
        _ => panic!("unknown cross-scheme CPU identity '{cpu}'"),
    };
    NativeMachineIdentityV1::CrossScheme {
        cpu: cpu.to_string(),
        cpu_bits,
        endianness,
        abi: "base".to_string(),
        libc: "gnu".to_string(),
        os: "linux".to_string(),
    }
}

fn data_lines(data: &str) -> impl Iterator<Item = &str> {
    data.lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
}

#[cfg(test)]
mod tests;
