// conary-core/src/ccs/security_policy.rs
//! Generic Linux Security Module policy intent metadata.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;

pub const SECURITY_POLICY_INTENT_SCHEMA_V1: &str = "conary.security-policy-intent.v1";

macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $($(#[$variant_meta:meta])* $variant:ident => $value:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Default)]
        pub enum $name {
            $($(#[$variant_meta])* $variant,)+
            Unknown(String),
        }

        impl $name {
            pub fn as_str(&self) -> &str {
                match self {
                    $(Self::$variant => $value,)+
                    Self::Unknown(value) => value.as_str(),
                }
            }

            pub fn is_known(&self) -> bool {
                !matches!(self, Self::Unknown(_))
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                struct Visitor;

                impl<'de> serde::de::Visitor<'de> for Visitor {
                    type Value = $name;

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str("a string enum value")
                    }

                    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
                    where
                        E: serde::de::Error,
                    {
                        Ok(match value {
                            $($value => $name::$variant,)+
                            other => $name::Unknown(other.to_string()),
                        })
                    }
                }

                deserializer.deserialize_str(Visitor)
            }
        }
    };
}

string_enum! {
    pub enum SecurityPolicyProvider {
        Selinux => "selinux",
        Apparmor => "apparmor",
        Tomoyo => "tomoyo",
        Smack => "smack",
        Landlock => "landlock",
        #[default]
        Any => "any",
    }
}

string_enum! {
    pub enum SecurityPolicyFallback {
        #[default]
        Dormant => "dormant",
        Warning => "warning",
        Degraded => "degraded",
        BlockOnEnforcingTarget => "block-on-enforcing-target",
    }
}

string_enum! {
    pub enum SecurityPolicyReconciliationState {
        Applied => "applied",
        Dormant => "dormant",
        #[default]
        Pending => "pending",
        Degraded => "degraded",
        Blocked => "blocked",
        Review => "review",
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecurityPolicyIntent {
    pub schema: String,
    pub id: String,
    #[serde(default)]
    pub source: SecurityPolicySource,
    #[serde(default)]
    pub provider: SecurityPolicyProvider,
    pub operation: String,
    #[serde(default)]
    pub scope: SecurityPolicyScope,
    #[serde(default)]
    pub desired_state: BTreeMap<String, toml::Value>,
    #[serde(default)]
    pub requirements: SecurityPolicyRequirements,
    #[serde(default)]
    pub fallback: SecurityPolicyFallback,
    #[serde(default)]
    pub payload_evidence: SecurityPolicyPayloadEvidence,
    #[serde(default)]
    pub reconciliation: SecurityPolicyReconciliation,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

impl Eq for SecurityPolicyIntent {}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SecurityPolicySource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_distro: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_id: Option<String>,
}

impl Eq for SecurityPolicySource {}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SecurityPolicyScope {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

impl Eq for SecurityPolicyScope {}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SecurityPolicyRequirements {
    #[serde(default)]
    pub required_on_active_provider: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<String>,
}

impl Eq for SecurityPolicyRequirements {}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SecurityPolicyPayloadEvidence {
    #[serde(default)]
    pub payload_backed: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

impl Eq for SecurityPolicyPayloadEvidence {}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SecurityPolicyReconciliation {
    #[serde(default)]
    pub state: SecurityPolicyReconciliationState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_provider: Option<String>,
}

impl Eq for SecurityPolicyReconciliation {}
