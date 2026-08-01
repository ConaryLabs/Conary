// crates/conary-core/src/image/arch.rs

//! Target architecture for disk image and boot asset planning.

use serde::{Deserialize, Serialize};

/// Target architecture for image and boot asset planning
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TargetArch {
    /// x86_64 / AMD64
    #[default]
    X86_64,
    /// AArch64 / ARM64
    Aarch64,
    /// RISC-V 64-bit
    Riscv64,
}

impl TargetArch {
    /// Get the GNU target triple
    pub fn triple(&self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64-conary-linux-gnu",
            Self::Aarch64 => "aarch64-conary-linux-gnu",
            Self::Riscv64 => "riscv64-conary-linux-gnu",
        }
    }

    /// Get the kernel architecture name
    pub fn kernel_arch(&self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "arm64",
            Self::Riscv64 => "riscv",
        }
    }

    /// Strings the `file` command uses to describe binaries for this arch.
    ///
    /// Returns a slice of patterns; if *any* appears in the `file` output the
    /// binary targets the expected architecture.
    pub fn file_arch_patterns(&self) -> &'static [&'static str] {
        match self {
            Self::X86_64 => &["x86-64", "x86_64"],
            Self::Aarch64 => &["aarch64", "ARM aarch64"],
            Self::Riscv64 => &["RISC-V", "riscv64"],
        }
    }

    /// Parse from string
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "x86_64" | "amd64" | "x64" => Some(Self::X86_64),
            "aarch64" | "arm64" => Some(Self::Aarch64),
            "riscv64" => Some(Self::Riscv64),
            _ => None,
        }
    }
}

impl std::fmt::Display for TargetArch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::X86_64 => write!(f, "x86_64"),
            Self::Aarch64 => write!(f, "aarch64"),
            Self::Riscv64 => write!(f, "riscv64"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_target_arch_triple() {
        assert_eq!(TargetArch::X86_64.triple(), "x86_64-conary-linux-gnu");
        assert_eq!(TargetArch::Aarch64.triple(), "aarch64-conary-linux-gnu");
        assert_eq!(TargetArch::Riscv64.triple(), "riscv64-conary-linux-gnu");
    }

    #[test]
    fn test_target_arch_parse() {
        assert_eq!(TargetArch::parse("x86_64"), Some(TargetArch::X86_64));
        assert_eq!(TargetArch::parse("amd64"), Some(TargetArch::X86_64));
        assert_eq!(TargetArch::parse("aarch64"), Some(TargetArch::Aarch64));
        assert_eq!(TargetArch::parse("arm64"), Some(TargetArch::Aarch64));
        assert_eq!(TargetArch::parse("riscv64"), Some(TargetArch::Riscv64));
        assert_eq!(TargetArch::parse("unknown"), None);
    }
}
