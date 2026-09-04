// crates/conary-agent-contract/src/catalog.rs
//! Catalog metadata consumed by Conary agent adapters.

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

pub fn packaging_tools() -> Vec<CatalogItem> {
    vec![
        CatalogItem {
            name: "conary.packaging.inspect_project".to_string(),
            description: "Inspect local packaging project or artifact facts without building."
                .to_string(),
            when_to_use: "Use before planning cook or publish work".to_string(),
            risk: RiskLevel::ReadOnly,
            cache: CachePolicy::private_short(),
        },
        CatalogItem {
            name: "conary.packaging.diagnose_latest_failure".to_string(),
            description: "Diagnose the newest failed packaging operation record.".to_string(),
            when_to_use: "Use after a cook or publish command failed".to_string(),
            risk: RiskLevel::ReadOnly,
            cache: CachePolicy::private_short(),
        },
        CatalogItem {
            name: "conary.packaging.operation_records.list".to_string(),
            description: "List recent redacted packaging operation records.".to_string(),
            when_to_use: "Use to find operation ids for follow-up diagnosis".to_string(),
            risk: RiskLevel::ReadOnly,
            cache: CachePolicy::private_short(),
        },
        CatalogItem {
            name: "conary.packaging.operation_records.read".to_string(),
            description: "Read one redacted packaging operation record.".to_string(),
            when_to_use: "Use when an exact operation id is already known".to_string(),
            risk: RiskLevel::ReadOnly,
            cache: CachePolicy::private_short(),
        },
        CatalogItem {
            name: "conary.packaging.publish.plan".to_string(),
            description: "Plan static artifact publish and return confirmation material."
                .to_string(),
            when_to_use: "Use before applying an attested CCS artifact to a static repository"
                .to_string(),
            risk: RiskLevel::ReadOnly,
            cache: CachePolicy::private_short(),
        },
        CatalogItem {
            name: "conary.packaging.publish.apply".to_string(),
            description: "Apply a confirmed static artifact publish plan.".to_string(),
            when_to_use:
                "Use only with a fresh plan id, matching fingerprint, and explicit confirmation"
                    .to_string(),
            risk: RiskLevel::High,
            cache: CachePolicy::private_short(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_policy_serializes_contract_field_names() {
        let value = serde_json::to_value(CachePolicy::private_short()).unwrap();
        assert_eq!(value["ttlMs"], 30_000);
        assert_eq!(value["cacheScope"], "private");
    }

    #[test]
    fn packaging_tools_have_unique_names_and_declared_use() {
        let tools = packaging_tools();
        let names = tools
            .iter()
            .map(|item| item.name.as_str())
            .collect::<std::collections::BTreeSet<_>>();

        assert_eq!(tools.len(), 6);
        assert_eq!(names.len(), tools.len());
        assert!(tools.iter().all(|item| !item.when_to_use.is_empty()));
        assert!(tools.iter().all(|item| item.cache.ttl_ms > 0));
        assert_eq!(tools.last().unwrap().risk, RiskLevel::High);
    }
}
