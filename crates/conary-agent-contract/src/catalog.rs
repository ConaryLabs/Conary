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
        CatalogItem {
            name: "conary-test.suites".to_string(),
            description: "Read local conary-test suite manifest inventory".to_string(),
            when_to_use: "Use before selecting local conary-test smoke or validation suites"
                .to_string(),
            risk: RiskLevel::ReadOnly,
            cache: CachePolicy::private_short(),
        },
    ]
}

// These are catalog definitions only. Do not register them as live MCP prompts
// until the stateless MCP adapter decision is satisfied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PromptCatalogItem {
    pub name: String,
    pub description: String,
    pub deterministic_inputs: Vec<String>,
    pub expected_result: String,
    pub cache: CachePolicy,
}

pub fn first_slice_prompts() -> Vec<PromptCatalogItem> {
    vec![
        PromptCatalogItem {
            name: "inspect_remi_health".to_string(),
            description: "Inspect Remi health before admin or package-service work".to_string(),
            deterministic_inputs: vec!["conary://remi/health".to_string()],
            expected_result: "InspectResult".to_string(),
            cache: CachePolicy::private_short(),
        },
        PromptCatalogItem {
            name: "debug_failing_test".to_string(),
            description: "Collect run, artifact, and log evidence for a failing conary-test run"
                .to_string(),
            deterministic_inputs: vec![
                "conary-test://runs/{run_id}".to_string(),
                "conary-test://runs/{run_id}/artifacts/{artifact_id}".to_string(),
            ],
            expected_result: "ExplainResult".to_string(),
            cache: CachePolicy::private_short(),
        },
        PromptCatalogItem {
            name: "bootstrap_local_dev_environment".to_string(),
            description: "Inspect local prerequisites and propose the next bootstrap proof step"
                .to_string(),
            deterministic_inputs: vec!["conary-local://bootstrap/status".to_string()],
            expected_result: "PlanResult".to_string(),
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

    #[test]
    fn default_read_resources_include_conary_test_suites() {
        let resources = default_read_resources();
        let suites = resources
            .iter()
            .find(|item| item.name == "conary-test.suites")
            .expect("conary-test suites catalog entry should exist");

        assert_eq!(
            suites.description,
            "Read local conary-test suite manifest inventory"
        );
        assert_eq!(
            suites.when_to_use,
            "Use before selecting local conary-test smoke or validation suites"
        );
        assert_eq!(suites.risk, RiskLevel::ReadOnly);
        assert_eq!(suites.cache, CachePolicy::private_short());
    }

    #[test]
    fn first_slice_prompt_catalog_is_limited_to_three_prompts() {
        let prompts = first_slice_prompts();
        assert_eq!(prompts.len(), 3);
        assert!(
            prompts
                .iter()
                .all(|prompt| !prompt.deterministic_inputs.is_empty())
        );
    }
}
