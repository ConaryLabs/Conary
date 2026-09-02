// crates/conary-core/src/repository/architecture.rs

//! Pinned source-derived package architecture authority.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use crate::error::{Error, Result};
use crate::repository::supported_profiles::{
    ProfileTargetPackageAbi, SupportedProfile, profile_by_id,
};
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
/// Endianness stated by dpkg's pinned `cputable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NativeMachineEndiannessV1 {
    Big,
    Little,
}

/// Exact Rust compile-target facts used to derive native package identity.
///
/// Tests construct this value explicitly so every supported cross target is
/// proved without depending on the machine that runs the test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeHostTargetFactsV1 {
    pub target_triple: String,
    pub target_arch: String,
    pub pointer_width: u16,
    pub endianness: NativeMachineEndiannessV1,
    pub target_abi: String,
}

/// Float calling convention, which is a machine dimension only on 32-bit ARM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NativeArmFloatAbiV1 {
    Soft,
    Hard,
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
        arm_float_abi: Option<NativeArmFloatAbiV1>,
    },
    Dpkg {
        cpu: String,
        cpu_bits: u16,
        endianness: NativeMachineEndiannessV1,
        arm_float_abi: Option<NativeArmFloatAbiV1>,
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
    Machine {
        identity: NativeMachineIdentityV1,
        package_abi: Option<ProfileTargetPackageAbi>,
    },
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
}

static ARCHITECTURE_AUTHORITY: OnceLock<ArchitectureAuthority> = OnceLock::new();

fn authority() -> &'static ArchitectureAuthority {
    ARCHITECTURE_AUTHORITY.get_or_init(|| ArchitectureAuthority {
        rpm: parse_rpm_authority(),
        debian: parse_dpkg_authority(),
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

/// Reject a package architecture outside its source format/profile authority.
pub fn require_known_package_architecture_for_profile(
    profile_id: &str,
    scheme: VersionScheme,
    token: &str,
) -> Result<()> {
    let profile = profile_by_id(profile_id)
        .ok_or_else(|| Error::ConfigError(format!("unsupported source profile '{profile_id}'")))?;
    if known_package_architecture_for_profile(profile, scheme, token).is_some() {
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
    profile: &SupportedProfile,
    package_token: &str,
    target_token: &str,
) -> NativeResolutionArchitectureDecisionV1 {
    let scheme = profile.version_scheme();
    let Some(package) = known_package_architecture_for_profile(profile, scheme, package_token)
    else {
        return NativeResolutionArchitectureDecisionV1::UnknownArchitectureToken {
            scheme,
            token: package_token.to_string(),
        };
    };
    let Some(target_identity) = host_machine_identity(target_token) else {
        return NativeResolutionArchitectureDecisionV1::UnknownArchitectureToken {
            scheme,
            token: target_token.to_string(),
        };
    };
    let identity = match package {
        KnownPackageArchitecture::Independent => target_identity.clone(),
        KnownPackageArchitecture::Machine {
            identity,
            package_abi,
        } => {
            if package_abi != Some(profile.target_package_abi()) {
                return NativeResolutionArchitectureDecisionV1::Excluded { identity };
            }
            identity
        }
    };
    if identity == target_identity {
        NativeResolutionArchitectureDecisionV1::Admitted
    } else {
        NativeResolutionArchitectureDecisionV1::Excluded { identity }
    }
}

/// Decide native-only admission against an already-derived host identity.
#[must_use]
pub fn native_resolution_architecture_decision_for_identity(
    profile: &SupportedProfile,
    package_token: &str,
    target_identity: &NativeMachineIdentityV1,
) -> NativeResolutionArchitectureDecisionV1 {
    let scheme = profile.version_scheme();
    let Some(package) = known_package_architecture_for_profile(profile, scheme, package_token)
    else {
        return NativeResolutionArchitectureDecisionV1::UnknownArchitectureToken {
            scheme,
            token: package_token.to_string(),
        };
    };
    let identity = match package {
        KnownPackageArchitecture::Independent => target_identity.clone(),
        KnownPackageArchitecture::Machine {
            identity,
            package_abi,
        } => {
            if package_abi != Some(profile.target_package_abi()) {
                return NativeResolutionArchitectureDecisionV1::Excluded { identity };
            }
            identity
        }
    };
    if identity == *target_identity {
        NativeResolutionArchitectureDecisionV1::Admitted
    } else {
        NativeResolutionArchitectureDecisionV1::Excluded { identity }
    }
}

/// Return the current compile target's exact facts.
#[must_use]
pub fn native_host_target_facts() -> NativeHostTargetFactsV1 {
    let target_arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "x86") {
        "x86"
    } else if cfg!(target_arch = "arm") {
        "arm"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else if cfg!(target_arch = "powerpc64") {
        "powerpc64"
    } else if cfg!(target_arch = "s390x") {
        "s390x"
    } else if cfg!(target_arch = "riscv64") {
        "riscv64"
    } else {
        std::env::consts::ARCH
    };
    let pointer_width = if cfg!(target_pointer_width = "64") {
        64
    } else if cfg!(target_pointer_width = "32") {
        32
    } else {
        0
    };
    let endianness = if cfg!(target_endian = "big") {
        NativeMachineEndiannessV1::Big
    } else {
        NativeMachineEndiannessV1::Little
    };
    let target_abi = if cfg!(target_abi = "eabihf") {
        "eabihf"
    } else if cfg!(target_abi = "eabi") {
        "eabi"
    } else {
        ""
    };
    NativeHostTargetFactsV1 {
        target_triple: env!("CONARY_BUILD_TARGET").to_string(),
        target_arch: target_arch.to_string(),
        pointer_width,
        endianness,
        target_abi: target_abi.to_string(),
    }
}

/// Derive one host machine identity from libc-independent compile-target facts.
pub fn native_machine_identity_from_target_facts(
    facts: &NativeHostTargetFactsV1,
) -> Result<NativeMachineIdentityV1> {
    let identity = match (
        facts.target_arch.as_str(),
        facts.pointer_width,
        facts.endianness,
        facts.target_abi.as_str(),
    ) {
        ("x86_64", 64, NativeMachineEndiannessV1::Little, "") => shared_identity("x86_64"),
        ("arm", 32, NativeMachineEndiannessV1::Little, "eabihf") => {
            arm_identity(NativeArmFloatAbiV1::Hard)
        }
        ("arm", 32, NativeMachineEndiannessV1::Little, "eabi") => {
            arm_identity(NativeArmFloatAbiV1::Soft)
        }
        ("aarch64", 64, NativeMachineEndiannessV1::Little, "") => shared_identity("aarch64"),
        ("powerpc64", 64, NativeMachineEndiannessV1::Little, "") => shared_identity("powerpc64le"),
        ("s390x", 64, NativeMachineEndiannessV1::Big, "") => shared_identity("s390x"),
        ("riscv64", 64, NativeMachineEndiannessV1::Little, "") => shared_identity("riscv64"),
        _ => {
            return Err(Error::UnsupportedNativeHostTarget {
                triple: facts.target_triple.clone(),
            });
        }
    };
    Ok(identity)
}

/// Derive the running binary's native machine identity.
pub fn native_host_machine_identity() -> Result<NativeMachineIdentityV1> {
    native_machine_identity_from_target_facts(&native_host_target_facts())
}

/// Project a typed identity to the exact native token owned by one pinned table.
#[must_use]
pub fn native_token_for_machine_identity(
    scheme: VersionScheme,
    identity: &NativeMachineIdentityV1,
) -> Option<String> {
    match scheme {
        VersionScheme::Rpm => data_lines(RPM_TABLES).find_map(|line| {
            let fields = line.split(':').map(str::trim).collect::<Vec<_>>();
            if fields.first().copied() != Some("arch_canon") {
                return None;
            }
            let canonical = fields[2].split_whitespace().next()?;
            package_machine_identity(known_package_architecture(VersionScheme::Rpm, canonical))
                .is_some_and(|candidate| candidate == *identity)
                .then(|| canonical.to_string())
        }),
        VersionScheme::Debian => preferred_dpkg_token(identity),
        VersionScheme::Arch => [
            "x86_64", "i686", "armv7h", "aarch64", "ppc64le", "s390x", "riscv64",
        ]
        .into_iter()
        .find(|token| host_machine_identity(token).as_ref() == Some(identity))
        .map(str::to_string),
        VersionScheme::Conary | VersionScheme::Eopkg => [
            "x86_64", "i686", "armv7", "aarch64", "ppc64le", "s390x", "riscv64",
        ]
        .into_iter()
        .find(|token| {
            package_machine_identity(known_package_architecture(VersionScheme::Conary, token))
                .is_some_and(|candidate| candidate == *identity)
        })
        .map(str::to_string),
    }
}

fn preferred_dpkg_token(identity: &NativeMachineIdentityV1) -> Option<String> {
    let cpus = data_lines(DPKG_CPUTABLE)
        .map(|line| {
            line.split_whitespace()
                .next()
                .expect("dpkg CPU row has a name")
        })
        .collect::<Vec<_>>();
    for line in data_lines(DPKG_TUPLETABLE) {
        let output = line.split_whitespace().nth(1)?;
        if output.contains("<cpu>") {
            for cpu in &cpus {
                let token = output.replace("<cpu>", cpu);
                if package_identity_matches(
                    known_package_architecture(VersionScheme::Debian, &token),
                    identity,
                    Some(ProfileTargetPackageAbi::Gnu),
                ) {
                    return Some(token);
                }
            }
        } else if package_identity_matches(
            known_package_architecture(VersionScheme::Debian, output),
            identity,
            Some(ProfileTargetPackageAbi::Gnu),
        ) {
            return Some(output.to_string());
        }
    }
    None
}

pub(crate) fn known_package_architecture(
    scheme: VersionScheme,
    token: &str,
) -> Option<KnownPackageArchitecture> {
    match scheme {
        VersionScheme::Rpm => authority().rpm.get(token).cloned(),
        VersionScheme::Debian => authority().debian.get(token).cloned(),
        VersionScheme::Arch => None,
        VersionScheme::Conary => conary_architecture(token),
        VersionScheme::Eopkg => None,
    }
}

pub(crate) fn known_package_architecture_for_profile(
    profile: &SupportedProfile,
    scheme: VersionScheme,
    token: &str,
) -> Option<KnownPackageArchitecture> {
    if profile.version_scheme() != scheme {
        return None;
    }
    if scheme != VersionScheme::Arch {
        return known_package_architecture(scheme, token);
    }
    if !profile
        .package_architecture_tokens()
        .iter()
        .any(|allowed| allowed == token)
    {
        return None;
    }
    if token == "any" {
        return Some(KnownPackageArchitecture::Independent);
    }
    host_machine_identity(token).map(|identity| KnownPackageArchitecture::Machine {
        identity,
        package_abi: Some(profile.target_package_abi()),
    })
}

pub(crate) fn host_machine_identity(token: &str) -> Option<NativeMachineIdentityV1> {
    let common = match token {
        "x86_64" | "amd64" => Some(shared_identity("x86_64")),
        "x86" | "i386" | "i686" => Some(shared_identity("i686")),
        "aarch64" | "arm64" => Some(shared_identity("aarch64")),
        "powerpc64le" | "ppc64le" | "ppc64el" => Some(shared_identity("powerpc64le")),
        "s390x" => Some(shared_identity("s390x")),
        "riscv64" => Some(shared_identity("riscv64")),
        "armv7" | "armv7h" | "armhf" | "armv7hl" => Some(arm_identity(NativeArmFloatAbiV1::Hard)),
        "arm" | "armel" | "armv7l" => Some(arm_identity(NativeArmFloatAbiV1::Soft)),
        _ => None,
    };
    if common.is_some() {
        return common;
    }
    let identities = [
        VersionScheme::Rpm,
        VersionScheme::Debian,
        VersionScheme::Arch,
    ]
    .into_iter()
    .filter_map(|scheme| known_package_architecture(scheme, token))
    .filter_map(|architecture| package_machine_identity(Some(architecture)))
    .collect::<std::collections::BTreeSet<_>>();
    (identities.len() == 1)
        .then(|| identities.into_iter().next())
        .flatten()
}

fn package_machine_identity(
    architecture: Option<KnownPackageArchitecture>,
) -> Option<NativeMachineIdentityV1> {
    match architecture {
        Some(KnownPackageArchitecture::Machine { identity, .. }) => Some(identity),
        Some(KnownPackageArchitecture::Independent) | None => None,
    }
}

fn package_identity_matches(
    architecture: Option<KnownPackageArchitecture>,
    machine: &NativeMachineIdentityV1,
    package_abi: Option<ProfileTargetPackageAbi>,
) -> bool {
    matches!(
        architecture,
        Some(KnownPackageArchitecture::Machine {
            identity,
            package_abi: candidate_abi,
        }) if identity == *machine && candidate_abi == package_abi
    )
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
        "armv7hl" => arm_identity(NativeArmFloatAbiV1::Hard),
        "armv7l" => arm_identity(NativeArmFloatAbiV1::Soft),
        exact => NativeMachineIdentityV1::Rpm {
            canonical_architecture: exact.to_string(),
        },
    };
    KnownPackageArchitecture::Machine {
        identity,
        package_abi: Some(ProfileTargetPackageAbi::Glibc),
    }
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
    let identity = match shared_dpkg_cpu(cpu_name, abi) {
        Some(shared) => shared,
        None => NativeMachineIdentityV1::Dpkg {
            cpu: cpu.gnu_name.clone(),
            cpu_bits: if abi == "x32" { 32 } else { cpu.bits },
            endianness: cpu.endianness,
            arm_float_abi: None,
        },
    };
    let package_abi = match (libc, os) {
        ("gnu", "linux") => Some(ProfileTargetPackageAbi::Gnu),
        ("musl", "linux") => Some(ProfileTargetPackageAbi::Musl),
        ("uclibc", "linux") => Some(ProfileTargetPackageAbi::Uclibc),
        _ => None,
    };
    KnownPackageArchitecture::Machine {
        identity,
        package_abi,
    }
}

fn shared_dpkg_cpu(cpu_name: &str, abi: &str) -> Option<NativeMachineIdentityV1> {
    match (cpu_name, abi) {
        ("arm", "eabihf") => Some(arm_identity(NativeArmFloatAbiV1::Hard)),
        ("arm", "eabi") => Some(arm_identity(NativeArmFloatAbiV1::Soft)),
        ("amd64", "base") => Some(shared_identity("x86_64")),
        ("i386", "base") => Some(shared_identity("i686")),
        ("arm64", "base") => Some(shared_identity("aarch64")),
        ("ppc64el", "base") => Some(shared_identity("powerpc64le")),
        ("s390x", "base") => Some(shared_identity("s390x")),
        ("riscv64", "base") => Some(shared_identity("riscv64")),
        _ => None,
    }
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
        "armv7" => arm_identity(NativeArmFloatAbiV1::Hard),
        _ => return None,
    };
    Some(Machine {
        identity,
        package_abi: None,
    })
}

fn arm_identity(float_abi: NativeArmFloatAbiV1) -> NativeMachineIdentityV1 {
    NativeMachineIdentityV1::CrossScheme {
        cpu: "arm".to_string(),
        cpu_bits: 32,
        endianness: NativeMachineEndiannessV1::Little,
        arm_float_abi: Some(float_abi),
    }
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
        arm_float_abi: None,
    }
}

fn data_lines(data: &str) -> impl Iterator<Item = &str> {
    data.lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
}

#[cfg(test)]
mod tests;
