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

pub fn default_read_resources() -> Vec<CatalogItem> {
    vec![
        CatalogItem {
            name: "remi.health".to_string(),
            description: "Read Remi service health".to_string(),
            when_to_use: "Use before Remi admin or package-service operations".to_string(),
            risk: RiskLevel::ReadOnly,
            cache: CachePolicy::private_short(),
        },
        CatalogItem {
            name: "conary-test.bootstrap.status".to_string(),
            description: "Read local developer bootstrap status".to_string(),
            when_to_use: "Use before running local smoke validation".to_string(),
            risk: RiskLevel::ReadOnly,
            cache: CachePolicy::private_short(),
        },
    ]
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

    #[test]
    fn default_resources_are_read_only_and_explain_when_to_use() {
        let resources = default_read_resources();
        assert!(
            resources
                .iter()
                .all(|item| item.risk == RiskLevel::ReadOnly)
        );
        assert!(resources.iter().all(|item| !item.when_to_use.is_empty()));
        assert!(resources.iter().all(|item| item.cache.ttl_ms > 0));
    }
}
