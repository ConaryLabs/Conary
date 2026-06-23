// apps/conary/src/commands/ccs/lint.rs

use anyhow::{Context, Result};
use conary_core::ccs::CcsManifest;
use conary_core::ccs::v2::authoring::{
    AuthoringFinding, AuthoringFindingSeverity, V2AuthoringTargetProfile,
    lint_manifest_for_v2_authoring,
};
use std::path::{Path, PathBuf};

pub async fn cmd_ccs_lint(
    path: &str,
    format: crate::cli::CcsOutputFormat,
    target_profile: Option<String>,
) -> Result<()> {
    let manifest_path = manifest_path(path)?;
    let manifest = CcsManifest::from_file(&manifest_path).context("Failed to parse ccs.toml")?;
    let profile = super::target_profile::resolve_target_profile(target_profile.as_deref())?;
    let target_profile = profile.map(|profile| V2AuthoringTargetProfile {
        public_id: profile.id(),
        query: profile,
    });
    let findings = lint_manifest_for_v2_authoring(&manifest, target_profile);

    match format {
        crate::cli::CcsOutputFormat::Text => print_text(&findings),
        crate::cli::CcsOutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&findings)?);
        }
    }

    if findings
        .iter()
        .any(|finding| finding.severity == AuthoringFindingSeverity::Error)
    {
        anyhow::bail!("ccs lint found blocking errors");
    }
    Ok(())
}

fn manifest_path(path: &str) -> Result<PathBuf> {
    let path = Path::new(path);
    if path.is_file() {
        Ok(path.to_path_buf())
    } else if path.is_dir() {
        Ok(path.join("ccs.toml"))
    } else {
        anyhow::bail!("Cannot find ccs.toml at {}", path.display())
    }
}

fn print_text(findings: &[AuthoringFinding]) {
    if findings.is_empty() {
        println!("ccs lint passed");
        return;
    }
    for finding in findings {
        println!(
            "{} {:?}: {}",
            finding.code, finding.severity, finding.message
        );
        println!("  fix: {}", finding.suggestion);
    }
}
