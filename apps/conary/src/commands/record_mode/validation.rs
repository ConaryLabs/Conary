// apps/conary/src/commands/record_mode/validation.rs

use std::path::Path;

use anyhow::Result;

use crate::commands::cook::{CookRecordedDraftOptions, run_cook_for_recorded_draft};

pub(crate) fn validation_request(
    output_dir: &Path,
    operation_id: &str,
) -> CookRecordedDraftOptions {
    CookRecordedDraftOptions {
        recipe: output_dir.join("recipe.toml"),
        output_dir: output_dir.join("dist"),
        source_cache: output_dir.join("sources"),
        operation_id: operation_id.to_string(),
    }
}

pub(crate) fn validate_recorded_draft(
    output_dir: &Path,
    operation_id: &str,
) -> Result<conary_core::diagnostics::PackagingCommandOutput> {
    let request = validation_request(output_dir, operation_id);
    run_cook_for_recorded_draft(request)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_request_uses_dist_and_sources_under_output() {
        let output = std::path::PathBuf::from("recorded/demo");
        let request = validation_request(&output, "record-1");
        assert_eq!(
            request.recipe,
            std::path::PathBuf::from("recorded/demo/recipe.toml")
        );
        assert_eq!(
            request.output_dir,
            std::path::PathBuf::from("recorded/demo/dist")
        );
        assert_eq!(
            request.source_cache,
            std::path::PathBuf::from("recorded/demo/sources")
        );
    }

    #[test]
    fn validation_wrapper_reports_missing_recipe_error() {
        let temp = tempfile::tempdir().unwrap();
        let error = validate_recorded_draft(temp.path(), "record-1").unwrap_err();

        assert!(
            error.to_string().contains("Unsupported source target"),
            "{error:#}"
        );
    }
}
