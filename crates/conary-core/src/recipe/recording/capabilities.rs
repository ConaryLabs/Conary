// conary-core/src/recipe/recording/capabilities.rs

use super::{CapabilitySuggestion, InstalledFileEvidence};

pub fn suggest_capabilities_from_evidence(
    files: &[InstalledFileEvidence],
) -> Vec<CapabilitySuggestion> {
    let mut suggestions = Vec::new();
    if files
        .iter()
        .any(|file| file.executable && file.path.starts_with("usr/bin/"))
    {
        suggestions.push(CapabilitySuggestion {
            capability: "runtime.executable".to_string(),
            confidence: "medium".to_string(),
            rationale: "recorded install created executable files under usr/bin".to_string(),
        });
    }
    if files.iter().any(|file| file.path.ends_with(".service")) {
        suggestions.push(CapabilitySuggestion {
            capability: "service.systemd".to_string(),
            confidence: "low".to_string(),
            rationale: "recorded install created a systemd unit file".to_string(),
        });
    }
    suggestions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_runtime_executable_from_usr_bin_files() {
        let suggestions = suggest_capabilities_from_evidence(&[InstalledFileEvidence {
            path: "usr/bin/demo".to_string(),
            file_type: "file".to_string(),
            executable: true,
            size: 12,
            link_target: None,
        }]);

        assert_eq!(suggestions[0].capability, "runtime.executable");
    }
}
