// conary-agent-contract/src/catalog.rs
//! Catalog metadata for Conary agent-facing resources, tools, and prompts.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::result::RiskLevel;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CacheScope {
    Public,
    Private,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CachePolicy {
    #[serde(rename = "ttlMs")]
    pub ttl_ms: u64,
    #[serde(rename = "cacheScope")]
    pub cache_scope: CacheScope,
}

impl CachePolicy {
    pub const fn private_short() -> Self {
        Self {
            ttl_ms: 30_000,
            cache_scope: CacheScope::Private,
        }
    }

    pub const fn private_static() -> Self {
        Self {
            ttl_ms: 300_000,
            cache_scope: CacheScope::Private,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CatalogItem {
    pub name: String,
    pub description: String,
    pub when_to_use: String,
    pub risk: RiskLevel,
    pub cache: CachePolicy,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_policy_serializes_rc_field_names() {
        let value = serde_json::to_value(CachePolicy::private_short()).unwrap();
        assert_eq!(value["ttlMs"], 30_000);
        assert_eq!(value["cacheScope"], "private");
    }
}
