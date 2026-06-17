// apps/conary/src/commands/record_mode/draft.rs

use std::path::PathBuf;

use anyhow::Result;
use conary_core::recipe::recording::{
    DraftRecipeInput, InstalledFileEvidence, derive_draft_recipe,
    installed_file_paths_from_evidence,
};

pub(crate) struct DraftMaterialization {
    pub(crate) output_dir: PathBuf,
    pub(crate) package_name: String,
    pub(crate) package_version: String,
    pub(crate) command: Vec<String>,
    pub(crate) recording_destdir: String,
    pub(crate) installed_files: Vec<InstalledFileEvidence>,
    pub(crate) network_likely: bool,
}

pub(crate) fn materialize_draft_recipe(input: DraftMaterialization) -> Result<PathBuf> {
    std::fs::create_dir_all(&input.output_dir)?;
    let recipe = derive_draft_recipe(DraftRecipeInput {
        package_name: input.package_name,
        package_version: input.package_version,
        command: input.command,
        recording_destdir: input.recording_destdir,
        installed_files: installed_file_paths_from_evidence(&input.installed_files),
        network_likely: input.network_likely,
    })?;
    let path = input.output_dir.join("recipe.toml");
    std::fs::write(&path, recipe)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn materialize_draft_writes_recipe_under_output_dir() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("recorded/demo");
        std::fs::create_dir_all(output.join("source")).unwrap();

        let recipe_path = materialize_draft_recipe(DraftMaterialization {
            output_dir: output.clone(),
            package_name: "demo".to_string(),
            package_version: "0.1.0-recorded".to_string(),
            command: vec!["make".to_string(), "install".to_string()],
            recording_destdir: temp.path().join("destdir").to_string_lossy().to_string(),
            installed_files: vec![InstalledFileEvidence {
                path: "usr/bin/demo".to_string(),
                file_type: "file".to_string(),
                executable: true,
                size: 12,
                link_target: None,
            }],
            network_likely: false,
        })
        .unwrap();

        assert_eq!(recipe_path, output.join("recipe.toml"));
        let text = std::fs::read_to_string(recipe_path).unwrap();
        assert!(text.contains("path = \"source\""));
    }
}
