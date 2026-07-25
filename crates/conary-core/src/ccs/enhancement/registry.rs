// conary-core/src/ccs/enhancement/registry.rs
//! Enhancement registry for managing enhancement plugins

use super::{EnhancementContext, EnhancementEngine, EnhancementResult, EnhancementType};
use std::collections::HashMap;
use std::sync::Arc;

/// Registry of enhancement engines
///
/// The registry maintains a collection of enhancement engines, one per
/// enhancement type. It provides methods to register engines and look
/// them up by type.
pub struct EnhancementRegistry {
    engines: HashMap<EnhancementType, Arc<dyn EnhancementEngine>>,
}

impl EnhancementRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            engines: HashMap::new(),
        }
    }

    /// Create a registry with all built-in enhancers
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        registry.register_builtins();
        registry
    }

    /// Register the built-in enhancement engines
    fn register_builtins(&mut self) {
        self.register(Arc::new(ProvenanceEnhancer));
    }

    /// Register an enhancement engine
    pub fn register(&mut self, engine: Arc<dyn EnhancementEngine>) {
        self.engines.insert(engine.enhancement_type(), engine);
    }

    /// Get an enhancement engine by type
    pub fn get(&self, enhancement_type: EnhancementType) -> Option<&Arc<dyn EnhancementEngine>> {
        self.engines.get(&enhancement_type)
    }

    /// Get all registered enhancement types
    pub fn registered_types(&self) -> Vec<EnhancementType> {
        self.engines.keys().copied().collect()
    }

    /// Check if an enhancement type is registered
    pub fn has(&self, enhancement_type: EnhancementType) -> bool {
        self.engines.contains_key(&enhancement_type)
    }
}

impl Default for EnhancementRegistry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

// ============================================================================
// Built-in Enhancement Engines
// ============================================================================

/// Provenance extraction enhancer
///
/// Extracts provenance information from the original package metadata
/// and stores it in the provenance table.
struct ProvenanceEnhancer;

impl EnhancementEngine for ProvenanceEnhancer {
    fn enhancement_type(&self) -> EnhancementType {
        EnhancementType::Provenance
    }

    fn should_enhance(&self, _ctx: &EnhancementContext) -> bool {
        // Enhance if we have original format metadata to extract
        true
    }

    fn enhance(&self, ctx: &mut EnhancementContext) -> EnhancementResult<()> {
        // Preserve the exact source-format and conversion metadata available at
        // this boundary; native-package extraction belongs to the converter.

        #[derive(serde::Serialize)]
        struct ExtractedProvenance {
            original_format: String,
            package_name: String,
            package_version: String,
            converted_by: String,
            conversion_note: String,
        }

        let provenance = ExtractedProvenance {
            original_format: ctx.original_format.clone(),
            package_name: ctx.metadata.name.clone(),
            package_version: ctx.metadata.version.clone(),
            converted_by: "conary".to_string(),
            conversion_note: format!(
                "Converted from {} by Conary v{}",
                ctx.original_format,
                env!("CARGO_PKG_VERSION")
            ),
        };

        // Store for audit trail
        ctx.store_extracted_provenance(&provenance)?;

        // Check if provenance record already exists
        let exists: bool = ctx
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM provenance WHERE trove_id = ?1)",
                [ctx.trove_id],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if !exists {
            // Link the durable package row to Conary's conversion provenance.
            ctx.conn.execute(
                "INSERT INTO provenance (trove_id, builder)
                 VALUES (?1, ?2)",
                rusqlite::params![ctx.trove_id, provenance.converted_by],
            )?;
        }

        tracing::info!(
            "Enhanced provenance for {} (format: {})",
            ctx.metadata.name,
            ctx.original_format
        );

        Ok(())
    }

    fn description(&self) -> &'static str {
        "Extract provenance information from original package metadata"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_builtins() {
        let registry = EnhancementRegistry::with_builtins();

        assert!(registry.has(EnhancementType::Provenance));
        assert_eq!(registry.registered_types().len(), 1);
    }

    #[test]
    fn test_registry_get() {
        let registry = EnhancementRegistry::with_builtins();

        let provenance = registry.get(EnhancementType::Provenance);
        assert!(provenance.is_some());
        assert_eq!(
            provenance.unwrap().enhancement_type(),
            EnhancementType::Provenance
        );
    }
}
