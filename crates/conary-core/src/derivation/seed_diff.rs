// conary-core/src/derivation/seed_diff.rs

use std::collections::BTreeSet;
use std::path::Path;

use toml::Value;

use crate::derivation::compose::erofs_image_hash;
use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedDiffReport {
    pub metadata_differences: Vec<String>,
    pub artifact_differences: Vec<String>,
    pub erofs_hash_a: Option<String>,
    pub erofs_hash_b: Option<String>,
}

impl SeedDiffReport {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.metadata_differences.is_empty()
            && self.artifact_differences.is_empty()
            && self.erofs_hash_a == self.erofs_hash_b
    }
}

pub fn diff_seed_dirs(path_a: &Path, path_b: &Path) -> Result<SeedDiffReport> {
    let mut metadata_differences = Vec::new();
    let mut artifact_differences = Vec::new();

    for artifact in ["seed.toml", "seed.erofs", "cas"] {
        let exists_a = path_a.join(artifact).exists();
        let exists_b = path_b.join(artifact).exists();
        if exists_a != exists_b {
            let location = if exists_a { "A only" } else { "B only" };
            artifact_differences.push(format!("{artifact}: present in {location}"));
        }
    }

    let seed_toml_a = path_a.join("seed.toml");
    let seed_toml_b = path_b.join("seed.toml");
    if seed_toml_a.exists() && seed_toml_b.exists() {
        let value_a = load_seed_metadata_value(&seed_toml_a)?;
        let value_b = load_seed_metadata_value(&seed_toml_b)?;
        metadata_differences.extend(diff_top_level_metadata(&value_a, &value_b));
    }

    let seed_erofs_a = path_a.join("seed.erofs");
    let seed_erofs_b = path_b.join("seed.erofs");
    let erofs_hash_a = if seed_erofs_a.exists() {
        Some(hash_seed_erofs(&seed_erofs_a)?)
    } else {
        None
    };
    let erofs_hash_b = if seed_erofs_b.exists() {
        Some(hash_seed_erofs(&seed_erofs_b)?)
    } else {
        None
    };
    if erofs_hash_a.is_some() && erofs_hash_b.is_some() && erofs_hash_a != erofs_hash_b {
        artifact_differences.push("seed.erofs: content hash differs".to_string());
    }

    Ok(SeedDiffReport {
        metadata_differences,
        artifact_differences,
        erofs_hash_a,
        erofs_hash_b,
    })
}

fn load_seed_metadata_value(path: &Path) -> Result<Value> {
    let content = std::fs::read_to_string(path)?;
    toml::from_str(&content)
        .map_err(|error| Error::ParseError(format!("failed to parse {}: {error}", path.display())))
}

fn hash_seed_erofs(path: &Path) -> Result<String> {
    erofs_image_hash(path)
        .map_err(|error| Error::ParseError(format!("failed to hash {}: {error}", path.display())))
}

fn diff_top_level_metadata(value_a: &Value, value_b: &Value) -> Vec<String> {
    let table_a = value_a.as_table();
    let table_b = value_b.as_table();
    let mut keys = BTreeSet::new();

    if let Some(table) = table_a {
        keys.extend(table.keys().cloned());
    }
    if let Some(table) = table_b {
        keys.extend(table.keys().cloned());
    }

    keys.into_iter()
        .filter_map(|key| {
            let left = table_a.and_then(|table| table.get(&key));
            let right = table_b.and_then(|table| table.get(&key));
            if left == right {
                None
            } else {
                Some(format!(
                    "{key}: {} != {}",
                    format_value(left),
                    format_value(right)
                ))
            }
        })
        .collect()
}

fn format_value(value: Option<&Value>) -> String {
    value
        .map(Value::to_string)
        .unwrap_or_else(|| "<missing>".to_string())
}

#[cfg(test)]
mod tests {
    use super::diff_seed_dirs;
    use crate::derivation::compose::erofs_image_hash;

    fn write_seed_dir(
        path: &std::path::Path,
        erofs_contents: &[u8],
        metadata: &str,
        create_cas: bool,
    ) {
        std::fs::create_dir_all(path).unwrap();
        std::fs::write(path.join("seed.erofs"), erofs_contents).unwrap();
        std::fs::write(path.join("seed.toml"), metadata).unwrap();
        if create_cas {
            std::fs::create_dir_all(path.join("cas")).unwrap();
        }
    }

    #[test]
    fn test_seed_diff_reports_metadata_hash_and_missing_artifacts() {
        let temp = tempfile::tempdir().unwrap();
        let path_a = temp.path().join("seed-a");
        let path_b = temp.path().join("seed-b");
        let tmp_a = path_a.join("tmp.erofs");
        let tmp_b = path_b.join("tmp.erofs");
        std::fs::create_dir_all(&path_a).unwrap();
        std::fs::create_dir_all(&path_b).unwrap();
        std::fs::write(&tmp_a, b"seed-a").unwrap();
        std::fs::write(&tmp_b, b"seed-b").unwrap();
        let hash_a = erofs_image_hash(&tmp_a).unwrap();
        let hash_b = erofs_image_hash(&tmp_b).unwrap();

        write_seed_dir(
            &path_a,
            b"seed-a",
            &format!(
                "seed_id = \"{hash_a}\"\nsource = \"adopted\"\norigin_distro = \"fedora\"\npackages = []\ntarget_triple = \"x86_64\"\nverified_by = []\n"
            ),
            true,
        );
        write_seed_dir(
            &path_b,
            b"seed-b",
            &format!(
                "seed_id = \"{hash_b}\"\nsource = \"adopted\"\norigin_distro = \"arch\"\npackages = []\ntarget_triple = \"x86_64\"\nverified_by = []\n"
            ),
            false,
        );

        let report = diff_seed_dirs(&path_a, &path_b).expect("diff seeds");
        assert!(
            report
                .metadata_differences
                .iter()
                .any(|line| line.contains("origin_distro"))
        );
        assert!(
            report
                .artifact_differences
                .iter()
                .any(|line| line.contains("cas"))
        );
        assert_ne!(report.erofs_hash_a, report.erofs_hash_b);
    }
}
