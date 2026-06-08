// conary-core/src/scriptlet/sandbox.rs

use serde::{Deserialize, Serialize};

/// Sandbox mode for scriptlet execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxMode {
    /// No sandboxing - direct execution
    #[serde(rename = "never", alias = "none")]
    None,
    /// Automatic - sandbox based on script risk analysis
    Auto,
    /// Always sandbox all scripts
    #[default]
    Always,
}

impl SandboxMode {
    /// Parse sandbox mode from string (auto, always, never)
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "never" | "none" | "off" | "false" => Some(Self::None),
            "auto" => Some(Self::Auto),
            "always" | "on" | "true" => Some(Self::Always),
            _ => None,
        }
    }

    /// Stable string for diagnostics and changeset metadata.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "never",
            Self::Auto => "auto",
            Self::Always => "always",
        }
    }
}

/// Sandbox boundary actually used for a scriptlet execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectiveSandbox {
    /// Live-root protected mode with namespace isolation.
    ProtectedLiveRoot,
    /// Direct legacy execution on the live host.
    Direct,
    /// Alternate-root execution for bootstrap/offline targets.
    TargetRoot,
}

impl EffectiveSandbox {
    /// Stable string for diagnostics and changeset metadata.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProtectedLiveRoot => "protected-live-root",
            Self::Direct => "direct",
            Self::TargetRoot => "target-root",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SandboxMode;

    #[test]
    fn test_sandbox_mode_default_is_always() {
        assert_eq!(SandboxMode::default(), SandboxMode::Always);
    }

    #[test]
    fn test_sandbox_mode_parse() {
        // "none" variants
        assert_eq!(SandboxMode::parse("never"), Some(SandboxMode::None));
        assert_eq!(SandboxMode::parse("none"), Some(SandboxMode::None));
        assert_eq!(SandboxMode::parse("off"), Some(SandboxMode::None));
        assert_eq!(SandboxMode::parse("false"), Some(SandboxMode::None));

        // "auto"
        assert_eq!(SandboxMode::parse("auto"), Some(SandboxMode::Auto));

        // "always" variants
        assert_eq!(SandboxMode::parse("always"), Some(SandboxMode::Always));
        assert_eq!(SandboxMode::parse("on"), Some(SandboxMode::Always));
        assert_eq!(SandboxMode::parse("true"), Some(SandboxMode::Always));

        // Case insensitivity
        assert_eq!(SandboxMode::parse("AUTO"), Some(SandboxMode::Auto));
        assert_eq!(SandboxMode::parse("NEVER"), Some(SandboxMode::None));
        assert_eq!(SandboxMode::parse("Always"), Some(SandboxMode::Always));

        // Invalid
        assert_eq!(SandboxMode::parse("invalid"), None);
        assert_eq!(SandboxMode::parse(""), None);
    }

    #[test]
    fn sandbox_mode_serde_round_trips_goal7_matrix_spellings() {
        assert_eq!(
            serde_json::from_str::<SandboxMode>("\"never\"").expect("never deserializes"),
            SandboxMode::None
        );
        assert_eq!(
            serde_json::from_str::<SandboxMode>("\"none\"").expect("none alias deserializes"),
            SandboxMode::None
        );
        assert_eq!(
            serde_json::from_str::<SandboxMode>("\"auto\"").expect("auto deserializes"),
            SandboxMode::Auto
        );
        assert_eq!(
            serde_json::from_str::<SandboxMode>("\"always\"").expect("always deserializes"),
            SandboxMode::Always
        );
        assert_eq!(
            serde_json::to_string(&SandboxMode::None).expect("serialize none"),
            "\"never\""
        );
    }
}
