// src/commands/model/snapshot.rs

use super::super::open_db;
use anyhow::Result;
use conary_core::model::{capture_current_state, snapshot_to_model};

/// Create a model file from current system state
pub async fn cmd_model_snapshot(
    output_path: &str,
    db_path: &str,
    description: Option<&str>,
) -> Result<()> {
    // Open database and capture current state
    let conn = open_db(db_path)?;
    let state = capture_current_state(&conn)?;

    // Create model from state
    let model = snapshot_to_model(&state);

    // Generate TOML
    let mut toml_content = String::new();

    // Add header comment
    toml_content.push_str("# Conary System Model\n");
    toml_content.push_str("# Generated from current system state\n");
    if let Some(desc) = description {
        toml_content.push_str(&format!("# Description: {}\n", desc));
    }
    toml_content.push_str(&format!(
        "# Generated at: {}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    ));
    toml_content.push_str("#\n");
    toml_content.push_str("# Edit this file to define your desired system state.\n");
    toml_content.push_str("# Then run 'conary model-apply' to sync the system.\n");
    toml_content.push('\n');

    // Add model content
    toml_content.push_str(&model.to_toml()?);

    // Write to file
    std::fs::write(output_path, &toml_content)?;

    println!("Model snapshot written to: {}", output_path);
    println!();
    println!("Captured:");
    println!("  - {} explicit package(s)", model.config.install.len());
    println!("  - {} pinned package(s)", model.pin.len());
    println!();
    println!("Edit the file to customize, then run:");
    println!("  conary model-diff -m {}   # Preview changes", output_path);
    println!("  conary model-apply -m {}  # Apply changes", output_path);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_helpers::create_test_db;
    use conary_core::db::models::DistroPin;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_model_snapshot_writes_effective_source_policy() {
        let (_temp_file, db_path) = create_test_db();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        DistroPin::set(&conn, "arch", "strict").unwrap();
        drop(conn);

        let temp_dir = tempdir().unwrap();
        let output_path = temp_dir.path().join("system.toml");

        cmd_model_snapshot(
            output_path.to_str().unwrap(),
            &db_path,
            Some("snapshot test"),
        )
        .await
        .unwrap();

        let content = std::fs::read_to_string(&output_path).unwrap();
        assert!(content.contains("[system]"));
        assert!(content.contains("profile = \"balanced/latest-anywhere\""));
        assert!(content.contains("[system.pin]"));
        assert!(content.contains("distro = \"arch\""));
        assert!(content.contains("strength = \"strict\""));
    }
}
