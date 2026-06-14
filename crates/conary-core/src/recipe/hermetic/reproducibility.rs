// conary-core/src/recipe/hermetic/reproducibility.rs

use crate::recipe::hermetic::evidence::ReproducibilityRecord;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReproducibilityConfig {
    pub source_date_epoch: Option<i64>,
    pub path_remap_count: usize,
    pub env: BTreeMap<String, String>,
}

impl ReproducibilityConfig {
    pub fn record(&self) -> ReproducibilityRecord {
        ReproducibilityRecord {
            source_date_epoch: self.source_date_epoch,
            path_remap_count: self.path_remap_count,
            env_keys: self.env.keys().cloned().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_reproducibility_config_records_empty_scaffold() {
        let config = ReproducibilityConfig::default();

        assert_eq!(
            config.record(),
            ReproducibilityRecord {
                source_date_epoch: None,
                path_remap_count: 0,
                env_keys: Vec::new(),
            }
        );
    }
}
