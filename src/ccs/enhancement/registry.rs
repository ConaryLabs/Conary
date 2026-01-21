// src/ccs/enhancement/registry.rs
//! Enhancement registry for managing enhancement plugins

use super::{EnhancementContext, EnhancementEngine, EnhancementError, EnhancementResult, EnhancementType};
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
        self.register(Arc::new(CapabilityEnhancer));
        self.register(Arc::new(ProvenanceEnhancer));
        self.register(Arc::new(SubpackageEnhancer));
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

/// Capability inference enhancer
///
/// Uses the capability inference module to determine what system resources
/// a package needs based on its files and dependencies.
struct CapabilityEnhancer;

impl EnhancementEngine for CapabilityEnhancer {
    fn enhancement_type(&self) -> EnhancementType {
        EnhancementType::Capabilities
    }

    fn should_enhance(&self, _ctx: &EnhancementContext) -> bool {
        // Enhance if we have files to analyze
        // Skip packages that already have capabilities stored
        true // For now, always attempt - the context will handle deduplication
    }

    fn enhance(&self, ctx: &mut EnhancementContext) -> EnhancementResult<()> {
        use crate::capability::inference::{infer_capabilities, InferenceOptions};

        // Clone metadata before mutable borrow of files
        let metadata = ctx.metadata.clone();

        // Load files for analysis
        ctx.load_file_contents()?;
        let files = ctx.get_files()?;

        // Run capability inference
        let options = InferenceOptions::default();
        let inferred = infer_capabilities(files, &metadata, &options)
            .map_err(|e| EnhancementError::InferenceFailed(e.to_string()))?;

        // Store raw inference for audit trail
        ctx.store_inferred_capabilities(&inferred)?;

        // Convert to capability declaration and store
        let declaration = inferred.to_declaration();
        let declaration_json = serde_json::to_string(&declaration)?;

        // Insert or update capabilities table
        ctx.conn.execute(
            "INSERT INTO capabilities (trove_id, declaration_json, declaration_version)
             VALUES (?1, ?2, 1)
             ON CONFLICT(trove_id) DO UPDATE SET
                declaration_json = excluded.declaration_json,
                declared_at = CURRENT_TIMESTAMP",
            rusqlite::params![ctx.trove_id, declaration_json],
        )?;

        tracing::info!(
            "Enhanced capabilities for {} (tier {}, confidence {:?})",
            ctx.metadata.name,
            inferred.tier_used,
            inferred.confidence.primary
        );

        Ok(())
    }

    fn description(&self) -> &'static str {
        "Infer security capabilities from package binaries and files"
    }
}

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
        // Extract provenance based on original format
        // For now, we create a basic provenance record
        // Phase 3 will add full extraction from RPM/DEB/Arch metadata

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
            // Create a basic provenance record
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

/// Subpackage relationship enhancer
///
/// Detects and records relationships between base packages and their
/// subpackages (e.g., nginx-devel is a subpackage of nginx).
struct SubpackageEnhancer;

impl EnhancementEngine for SubpackageEnhancer {
    fn enhancement_type(&self) -> EnhancementType {
        EnhancementType::Subpackages
    }

    fn should_enhance(&self, _ctx: &EnhancementContext) -> bool {
        true
    }

    fn enhance(&self, ctx: &mut EnhancementContext) -> EnhancementResult<()> {
        // Detect subpackage pattern based on format
        let suffixes: &[(&str, &str)] = match ctx.original_format.as_str() {
            "rpm" => &[
                ("-devel", "devel"),
                ("-doc", "doc"),
                ("-docs", "doc"),
                ("-debuginfo", "debuginfo"),
                ("-debugsource", "debugsource"),
                ("-libs", "libs"),
                ("-common", "common"),
                ("-data", "data"),
            ],
            "deb" => &[
                ("-dev", "devel"),
                ("-doc", "doc"),
                ("-dbg", "debuginfo"),
                ("-common", "common"),
                ("-data", "data"),
            ],
            "arch" => &[
                ("-docs", "doc"),
            ],
            _ => &[],
        };

        let name = &ctx.metadata.name;

        for (suffix, component_type) in suffixes {
            if name.ends_with(suffix) {
                let base = name.trim_end_matches(suffix);
                ctx.store_subpackage_relationship(base, component_type)?;

                tracing::info!(
                    "Detected subpackage relationship: {} is {} of {}",
                    name,
                    component_type,
                    base
                );
                return Ok(());
            }
        }

        // Not a subpackage - nothing to do
        Ok(())
    }

    fn description(&self) -> &'static str {
        "Detect and record subpackage relationships"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_builtins() {
        let registry = EnhancementRegistry::with_builtins();

        assert!(registry.has(EnhancementType::Capabilities));
        assert!(registry.has(EnhancementType::Provenance));
        assert!(registry.has(EnhancementType::Subpackages));

        assert_eq!(registry.registered_types().len(), 3);
    }

    #[test]
    fn test_registry_get() {
        let registry = EnhancementRegistry::with_builtins();

        let caps = registry.get(EnhancementType::Capabilities);
        assert!(caps.is_some());
        assert_eq!(
            caps.unwrap().enhancement_type(),
            EnhancementType::Capabilities
        );
    }
}
