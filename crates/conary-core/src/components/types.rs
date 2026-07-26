// conary-core/src/components/types.rs

//! Explicit component identifiers used by package metadata and install requests.

/// Typed component names that package authors may declare.
///
/// Conary never derives these values from payload paths. Native packages that
/// do not carry explicit Conary component metadata are represented losslessly
/// as one `runtime` component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComponentType {
    Runtime,
    Lib,
    Devel,
    Doc,
    Config,
    Debuginfo,
    Test,
}

impl ComponentType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Lib => "lib",
            Self::Devel => "devel",
            Self::Doc => "doc",
            Self::Config => "config",
            Self::Debuginfo => "debuginfo",
            Self::Test => "test",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "runtime" => Some(Self::Runtime),
            "lib" => Some(Self::Lib),
            "devel" => Some(Self::Devel),
            "doc" => Some(Self::Doc),
            "config" => Some(Self::Config),
            "debuginfo" => Some(Self::Debuginfo),
            "test" => Some(Self::Test),
            _ => None,
        }
    }

    pub fn is_default(&self) -> bool {
        matches!(self, Self::Runtime | Self::Lib | Self::Config)
    }

    pub fn all() -> &'static [ComponentType] {
        &[
            Self::Runtime,
            Self::Lib,
            Self::Devel,
            Self::Doc,
            Self::Config,
            Self::Debuginfo,
            Self::Test,
        ]
    }

    pub fn defaults() -> &'static [ComponentType] {
        &[Self::Runtime, Self::Lib, Self::Config]
    }
}

impl std::fmt::Display for ComponentType {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, ":{}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_names_are_an_exact_closed_set() {
        for component in ComponentType::all() {
            assert_eq!(ComponentType::parse(component.as_str()), Some(*component));
        }
        assert_eq!(ComponentType::parse("shared-library"), None);
    }
}
