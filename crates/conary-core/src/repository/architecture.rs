// crates/conary-core/src/repository/architecture.rs

//! Pinned source-derived package architecture authority.

use crate::error::{Error, Result};
use crate::repository::versioning::VersionScheme;

/// Machine classes projected from the pinned upstream architecture tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeMachineArchitectureClassV1 {
    X86_64,
    X86_64V2,
    X86_64V3,
    X86_64V4,
    X86_64X32,
    X86_32,
    Aarch64,
    Alpha,
    Arc,
    Arm32,
    E2k,
    Hppa,
    Ia64,
    LoongArch64,
    M68k,
    Mips,
    Nios2,
    OpenRisc,
    PowerPc32,
    PowerPc64,
    PowerPc64Le,
    RiscV64,
    S390,
    S390x,
    SuperH,
    Sparc,
    Sparc64,
    Universal,
    Xtensa,
}

/// Three-valued native-only architecture admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeResolutionArchitectureDecisionV1 {
    Admitted,
    Excluded {
        class: NativeMachineArchitectureClassV1,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KnownPackageArchitecture {
    Independent,
    Machine(NativeMachineArchitectureClassV1),
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
    if package == KnownPackageArchitecture::Independent {
        return NativeResolutionArchitectureDecisionV1::Admitted;
    }
    let KnownPackageArchitecture::Machine(class) = package else {
        unreachable!("independent architecture returned above")
    };
    let Some(target_class) = target_machine_class(scheme, target_token) else {
        return NativeResolutionArchitectureDecisionV1::UnknownArchitectureToken {
            scheme,
            token: target_token.to_string(),
        };
    };
    let admitted = class == target_class;
    if admitted {
        NativeResolutionArchitectureDecisionV1::Admitted
    } else {
        NativeResolutionArchitectureDecisionV1::Excluded { class }
    }
}

pub(crate) fn known_package_architecture(
    scheme: VersionScheme,
    token: &str,
) -> Option<KnownPackageArchitecture> {
    match scheme {
        VersionScheme::Rpm => rpm_architecture(token),
        VersionScheme::Debian => debian_architecture(token),
        VersionScheme::Arch => arch_architecture(token),
        VersionScheme::Conary => conary_architecture(token),
        VersionScheme::Eopkg => None,
    }
}

fn target_machine_class(
    scheme: VersionScheme,
    token: &str,
) -> Option<NativeMachineArchitectureClassV1> {
    match known_package_architecture(scheme, token) {
        Some(KnownPackageArchitecture::Machine(class)) => Some(class),
        Some(KnownPackageArchitecture::Independent) => None,
        None => host_machine_class(token),
    }
}

pub(crate) fn host_machine_class(token: &str) -> Option<NativeMachineArchitectureClassV1> {
    use NativeMachineArchitectureClassV1 as Class;
    match token {
        "x86_64" | "amd64" => Some(Class::X86_64),
        "x86" | "i386" | "i686" => Some(Class::X86_32),
        "aarch64" | "arm64" => Some(Class::Aarch64),
        "arm" | "armv7" | "armhf" => Some(Class::Arm32),
        "powerpc64le" | "ppc64le" | "ppc64el" => Some(Class::PowerPc64Le),
        "s390x" => Some(Class::S390x),
        "riscv64" => Some(Class::RiscV64),
        _ => None,
    }
}

fn rpm_architecture(token: &str) -> Option<KnownPackageArchitecture> {
    use KnownPackageArchitecture::{Independent, Machine};
    use NativeMachineArchitectureClassV1 as Class;
    let class = match token {
        "noarch" => return Some(Independent),
        "fat" => Class::Universal,
        "x86_64" | "amd64" | "ia32e" | "em64t" => Class::X86_64,
        "x86_64_v2" => Class::X86_64V2,
        "x86_64_v3" => Class::X86_64V3,
        "x86_64_v4" => Class::X86_64V4,
        "athlon" | "geode" | "pentium3" | "pentium4" | "i386" | "i486" | "i586" | "i686"
        | "osfmach3_i386" | "osfmach3_i486" | "osfmach3_i586" | "osfmach3_i686" => Class::X86_32,
        "alpha" | "alphaev5" | "alphaev56" | "alphaev6" | "alphaev67" | "alphapca56" | "axp" => {
            Class::Alpha
        }
        "sparc" | "sun4" | "sun4c" | "sun4d" | "sun4m" | "sparcv8" => Class::Sparc,
        "sparc64" | "sparc64v" | "sparcv9" | "sparcv9v" | "sun4u" => Class::Sparc64,
        "IP" | "sgi" | "mips" | "mipsel" | "mips64" | "mips64el" | "mipsr6" | "mipsr6el"
        | "mips64r6" | "mips64r6el" => Class::Mips,
        "ppc" | "ppc8260" | "ppc8560" | "ppc32dy4" | "ppciseries" | "ppcpseries" | "powerpc"
        | "powerppc" | "rs6000" | "osfmach3_ppc" => Class::PowerPc32,
        "ppc64" | "ppc64iseries" | "ppc64p7" | "ppc64pseries" => Class::PowerPc64,
        "ppc64le" => Class::PowerPc64Le,
        "m68k" | "m68kmint" | "atarist" | "atariste" | "ataritt" | "atariclone" | "falcon"
        | "milan" | "hades" => Class::M68k,
        "ia64" => Class::Ia64,
        "armv3l" | "armv4b" | "armv4l" | "armv4tl" | "armv5tl" | "armv5tel" | "armv5tejl"
        | "armv6l" | "armv6hl" | "armv7l" | "armv7hl" | "armv7hnl" | "armv8l" | "armv8hl"
        | "armv8hnl" | "armv8hcnl" => Class::Arm32,
        "s390" | "i370" => Class::S390,
        "s390x" => Class::S390x,
        "sh" | "sh3" | "sh4" | "sh4a" => Class::SuperH,
        "xtensa" => Class::Xtensa,
        "aarch64" => Class::Aarch64,
        "riscv" | "riscv64" => Class::RiscV64,
        "loongarch64" => Class::LoongArch64,
        "e2k" | "e2kv4" | "e2kv5" | "e2kv6" | "e2k1cp" | "e2k8c" | "e2k8c2" | "e2k16c"
        | "e2k2c3" => Class::E2k,
        "hppa1.0" | "hppa1.1" | "hppa1.2" | "hppa2.0" | "parisc" => Class::Hppa,
        _ => return None,
    };
    Some(Machine(class))
}

fn debian_architecture(token: &str) -> Option<KnownPackageArchitecture> {
    use KnownPackageArchitecture::{Independent, Machine};
    use NativeMachineArchitectureClassV1 as Class;
    if token == "all" {
        return Some(Independent);
    }
    let class = match token {
        "x32" => Class::X86_64X32,
        "armel" | "armhf" | "uclibc-linux-armel" | "musl-linux-armhf" => Class::Arm32,
        "mipsn32" | "mipsn32el" | "mipsn32r6" | "mipsn32r6el" => Class::Mips,
        "hurd-amd64" | "dragonflybsd-amd64" | "freebsd-amd64" | "darwin-amd64"
        | "solaris-amd64" => Class::X86_64,
        "hurd-i386" | "freebsd-i386" | "darwin-i386" | "solaris-i386" => Class::X86_32,
        "freebsd-arm" | "darwin-arm" => Class::Arm32,
        "freebsd-arm64" | "darwin-arm64" => Class::Aarch64,
        "freebsd-powerpc" | "darwin-powerpc" | "aix-powerpc" => Class::PowerPc32,
        "freebsd-ppc64" | "darwin-ppc64" | "aix-ppc64" => Class::PowerPc64,
        "freebsd-riscv" => Class::RiscV64,
        "solaris-sparc" => Class::Sparc,
        "solaris-sparc64" => Class::Sparc64,
        "mint-m68k" => Class::M68k,
        _ => debian_cpu_class(token).or_else(|| {
            let cpu = token
                .strip_prefix("uclibc-linux-")
                .or_else(|| token.strip_prefix("musl-linux-"))
                .or_else(|| token.strip_prefix("openbsd-"))
                .or_else(|| token.strip_prefix("netbsd-"))?;
            debian_cpu_class(cpu)
        })?,
    };
    Some(Machine(class))
}

fn debian_cpu_class(token: &str) -> Option<NativeMachineArchitectureClassV1> {
    use NativeMachineArchitectureClassV1 as Class;
    match token {
        "alpha" => Some(Class::Alpha),
        "amd64" => Some(Class::X86_64),
        "arc" => Some(Class::Arc),
        "arm" | "armeb" => Some(Class::Arm32),
        "arm64" => Some(Class::Aarch64),
        "hppa" => Some(Class::Hppa),
        "loong64" => Some(Class::LoongArch64),
        "i386" => Some(Class::X86_32),
        "ia64" => Some(Class::Ia64),
        "m68k" => Some(Class::M68k),
        "mips" | "mipsel" | "mipsr6" | "mipsr6el" | "mips64" | "mips64el" | "mips64r6"
        | "mips64r6el" => Some(Class::Mips),
        "nios2" => Some(Class::Nios2),
        "or1k" => Some(Class::OpenRisc),
        "powerpc" | "powerpcel" => Some(Class::PowerPc32),
        "ppc64" => Some(Class::PowerPc64),
        "ppc64el" => Some(Class::PowerPc64Le),
        "riscv64" => Some(Class::RiscV64),
        "s390" => Some(Class::S390),
        "s390x" => Some(Class::S390x),
        "sh3" | "sh3eb" | "sh4" | "sh4eb" => Some(Class::SuperH),
        "sparc" => Some(Class::Sparc),
        "sparc64" => Some(Class::Sparc64),
        _ => None,
    }
}

fn arch_architecture(token: &str) -> Option<KnownPackageArchitecture> {
    use KnownPackageArchitecture::{Independent, Machine};
    match token {
        "any" => Some(Independent),
        "x86_64" => Some(Machine(NativeMachineArchitectureClassV1::X86_64)),
        _ => None,
    }
}

fn conary_architecture(token: &str) -> Option<KnownPackageArchitecture> {
    use KnownPackageArchitecture::{Independent, Machine};
    use NativeMachineArchitectureClassV1 as Class;
    match token {
        "noarch" => Some(Independent),
        "x86_64" => Some(Machine(Class::X86_64)),
        "i686" => Some(Machine(Class::X86_32)),
        "aarch64" => Some(Machine(Class::Aarch64)),
        "armv7" => Some(Machine(Class::Arm32)),
        "ppc64le" => Some(Machine(Class::PowerPc64Le)),
        "s390x" => Some(Machine(Class::S390x)),
        "riscv64" => Some(Machine(Class::RiscV64)),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
