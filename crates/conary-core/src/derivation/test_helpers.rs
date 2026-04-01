// conary-core/src/derivation/test_helpers.rs

//! Shared test utilities for the derivation module.

#[cfg(test)]
pub(crate) mod helpers {
    use std::collections::HashMap;
    use std::path::Path;

    use crate::filesystem::CasStore;
    use crate::recipe::{BuildSection, PackageSection, Recipe, SourceSection};

    /// Create a minimal test recipe with the given name and dependencies.
    pub fn make_recipe(name: &str, requires: &[&str], makedepends: &[&str]) -> Recipe {
        Recipe {
            package: PackageSection {
                name: name.to_string(),
                version: "1.0.0".to_string(),
                release: "1".to_string(),
                summary: None,
                description: None,
                license: None,
                homepage: None,
            },
            source: SourceSection {
                archive: format!("https://example.com/{name}-1.0.tar.gz"),
                checksum: "sha256:abc".to_string(),
                signature: None,
                additional: Vec::new(),
                extract_dir: None,
            },
            build: BuildSection {
                requires: requires.iter().map(|s| s.to_string()).collect(),
                makedepends: makedepends.iter().map(|s| s.to_string()).collect(),
                configure: None,
                make: None,
                install: None,
                check: None,
                setup: None,
                post_install: None,
                environment: HashMap::new(),
                workdir: None,
                script_file: None,
                jobs: None,
                stage: None,
            },
            cross: None,
            patches: None,
            components: None,
            variables: HashMap::new(),
        }
    }

    /// Create a CAS store in a temp directory.
    pub fn test_cas(dir: &Path) -> CasStore {
        let cas_dir = dir.join("cas");
        CasStore::new(&cas_dir).expect("CAS creation must succeed")
    }
}
