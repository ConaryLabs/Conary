// apps/conary/src/commands/install/acquire.rs

use super::conversion::{
    CcsArtifactInstallOptions, ConversionResult, install_ccs_artifact, try_convert_to_ccs,
};
use super::prepare::parse_package;
use super::resolve::{
    PackageResolutionRequest, PolicyOptions, ResolutionOutcome, ResolvedPackage,
    ResolvedSourceType, resolve_package_path_with_policy,
};
use super::{
    CcsEnvelopeAuthority, InstallIntent, InstallPhase, InstallProgress, PackageFormatType,
    RepositoryInstallProvenance, bind_transaction_source_identity, detect_package_format,
    effective_source_profile,
};
use anyhow::{Context, Result};
use conary_core::packages::PackageFormat;
use conary_core::repository::resolution_policy::ResolutionPolicy;
use conary_core::scriptlet::SandboxMode;
use std::collections::HashMap;
use tracing::info;

/// Parameters for CCS direct-install that are forwarded from `InstallOptions`.
pub(super) struct CcsInstallParams<'a> {
    pub(super) db_path: &'a str,
    pub(super) root: &'a str,
    pub(super) dry_run: bool,
    pub(super) sandbox_mode: SandboxMode,
    pub(super) no_deps: bool,
    pub(super) allow_downgrade: bool,
    pub(super) intent: InstallIntent,
    pub(super) yes: bool,
    pub(super) repository_provenance: Option<RepositoryInstallProvenance>,
    pub(super) requested_source_identity: Option<&'a str>,
}

/// Resolve a package path, detect its format, and parse it.
///
/// Handles early returns for CCS packages (from Remi, by extension, or via
/// conversion).  Returns `None` if the package was already installed as CCS
/// (no further processing needed), or `Some(...)` with the parsed native-format
/// package and its format type.
#[allow(clippy::too_many_arguments)]
pub(super) async fn resolve_and_parse_package(
    conn: &rusqlite::Connection,
    package_name: &str,
    package: &str,
    db_path: &str,
    version: Option<&str>,
    package_release: Option<&str>,
    repo: Option<&str>,
    architecture: Option<&str>,
    convert_to_ccs: bool,
    policy: &ResolutionPolicy,
    requested_source_profile: Option<&str>,
    ccs_opts: &CcsInstallParams<'_>,
) -> Result<
    Option<(
        Box<dyn PackageFormat>,
        PackageFormatType,
        Option<RepositoryInstallProvenance>,
    )>,
> {
    // Create progress tracker for single package installation
    let progress = InstallProgress::single("Installing");
    progress.set_phase(package_name, InstallPhase::Downloading);

    // Build policy options for resolution.  The root request carries the
    // full policy; transitive deps will inherit the mixing policy but not
    // the request scope (that is handled inside the resolver).
    let policy_opts = PolicyOptions {
        policy: Some(policy.clone()),
        is_root: true,
    };

    // Resolve package path (download if needed).
    // Checksum verification and temp-file cleanup on failure are handled
    // inside conary_core::repository::download (fix 1.4).
    // TODO(round2): Surface partial-download byte counts in error messages
    // so users can diagnose connection issues vs corrupt mirrors.
    let resolved = match resolve_package_path_with_policy(
        PackageResolutionRequest {
            package: package_name,
            db_path,
            version,
            package_release,
            repository: repo,
            architecture,
        },
        &progress,
        &policy_opts,
    )
    .await
    {
        Err(e) => {
            print_package_suggestions(conn, package_name);
            return Err(e);
        }
        Ok(ResolutionOutcome::AlreadyInstalled { name, version }) => {
            crate::ui::note(&format!("{name} {version} is already installed"));
            return Ok(None);
        }
        Ok(ResolutionOutcome::Resolved(pkg)) => pkg,
    };
    let repository_provenance = install_provenance_from_resolved(&resolved)
        .or_else(|| ccs_opts.repository_provenance.clone());
    let resolution_policy =
        bind_transaction_source_identity(policy.clone(), repository_provenance.as_ref())?;

    // Repository-resolved CCS artifacts are already in installable form.
    if resolved.source_type == ResolvedSourceType::Ccs {
        info!("Repository supplied a CCS artifact; installing directly");
        let ccs_path = resolved
            .path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid CCS path (non-UTF8)"))?;
        install_ccs_artifact(CcsArtifactInstallOptions {
            ccs_path,
            db_path: ccs_opts.db_path,
            root: ccs_opts.root,
            dry_run: ccs_opts.dry_run,
            sandbox_mode: ccs_opts.sandbox_mode,
            no_deps: ccs_opts.no_deps,
            allow_downgrade: ccs_opts.allow_downgrade,
            intent: ccs_opts.intent,
            yes: ccs_opts.yes,
            envelope_authority: direct_ccs_envelope_authority(
                resolved.source_type,
                repository_provenance.as_ref(),
            )?,
            repository_provenance,
            requested_source_identity: ccs_opts.requested_source_identity,
            resolution_policy,
        })
        .await?;
        return Ok(None);
    }

    let path_str = resolved
        .path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid package path (non-UTF8)"))?;

    if conary_core::ccs::archive_reader::has_current_ccs_archive_contract(&resolved.path)
        .context("Failed to inspect package archive contract")?
    {
        info!("Detected CCS package archive contract, installing directly");
        install_ccs_artifact(CcsArtifactInstallOptions {
            ccs_path: path_str,
            db_path: ccs_opts.db_path,
            root: ccs_opts.root,
            dry_run: ccs_opts.dry_run,
            sandbox_mode: ccs_opts.sandbox_mode,
            no_deps: ccs_opts.no_deps,
            allow_downgrade: ccs_opts.allow_downgrade,
            intent: ccs_opts.intent,
            yes: ccs_opts.yes,
            envelope_authority: direct_ccs_envelope_authority(
                resolved.source_type,
                repository_provenance.as_ref(),
            )?,
            repository_provenance,
            requested_source_identity: ccs_opts.requested_source_identity,
            resolution_policy,
        })
        .await?;
        return Ok(None);
    }

    // Detect and parse native package formats.
    let format = detect_package_format(path_str)
        .with_context(|| format!("Failed to detect package format for '{}'", path_str))?;
    info!("Detected package format: {:?}", format);

    progress.set_phase(package, InstallPhase::Parsing);
    let pkg = parse_package(&resolved.path, format)?;

    // Convert to CCS format if requested.
    if convert_to_ccs {
        progress.set_status(&format!("Converting {} to CCS format...", pkg.name()));

        match try_convert_to_ccs(
            pkg.as_ref(),
            &resolved.path,
            format,
            db_path,
            effective_source_profile(repository_provenance.as_ref(), requested_source_profile),
        )
        .await?
        {
            ConversionResult::Converted {
                ccs_path,
                temp_dir: _temp_dir,
                pending_record,
                signing_public_key,
            } => {
                // Install via CCS path (temp_dir kept alive until install completes)
                let installed_trove_id = install_ccs_artifact(CcsArtifactInstallOptions {
                    ccs_path: &ccs_path,
                    db_path: ccs_opts.db_path,
                    root: ccs_opts.root,
                    dry_run: ccs_opts.dry_run,
                    sandbox_mode: ccs_opts.sandbox_mode,
                    no_deps: ccs_opts.no_deps,
                    allow_downgrade: ccs_opts.allow_downgrade,
                    intent: ccs_opts.intent,
                    yes: ccs_opts.yes,
                    envelope_authority: CcsEnvelopeAuthority::ExactKey(signing_public_key),
                    repository_provenance,
                    requested_source_identity: ccs_opts.requested_source_identity,
                    resolution_policy,
                })
                .await?;
                if let Some(trove_id) = installed_trove_id {
                    pending_record.persist(db_path, trove_id)?;
                }
                return Ok(None);
            }
            ConversionResult::Skipped => {
                // Already converted - fall through to regular install path
            }
        }
    }

    Ok(Some((pkg, format, repository_provenance)))
}

fn direct_ccs_envelope_authority(
    source_type: ResolvedSourceType,
    repository_provenance: Option<&RepositoryInstallProvenance>,
) -> Result<CcsEnvelopeAuthority> {
    if repository_provenance.is_some() {
        return Ok(CcsEnvelopeAuthority::Repository);
    }
    if source_type == ResolvedSourceType::LocalFile {
        return Ok(CcsEnvelopeAuthority::LocalDev);
    }
    anyhow::bail!("Repository-resolved CCS package is missing exact repository provenance")
}

pub(super) fn install_provenance_from_resolved(
    resolved: &ResolvedPackage,
) -> Option<RepositoryInstallProvenance> {
    resolved
        .repository_provenance
        .as_ref()
        .map(|provenance| RepositoryInstallProvenance {
            repository_id: provenance.repository_id,
            source_identity: provenance.source_identity.clone(),
            source_profile: provenance.source_profile.clone(),
            version_scheme: provenance.version_scheme,
            source_kind: provenance.source_kind.clone(),
        })
}

/// Search canonical packages and repository packages for names similar to the
/// given query. Returns up to 5 `(name, distros)` pairs suitable for "did you
/// mean?" suggestions.
fn find_package_suggestions(
    conn: &rusqlite::Connection,
    name: &str,
) -> std::result::Result<Vec<(String, String)>, rusqlite::Error> {
    let mut hits: HashMap<String, Vec<String>> = HashMap::new();

    // 1. Search canonical_packages by substring
    {
        let mut stmt = conn.prepare(
            "SELECT cp.name, pi.distro
             FROM canonical_packages cp
             LEFT JOIN package_implementations pi ON pi.canonical_id = cp.id
             WHERE cp.name LIKE '%' || ?1 || '%'
             LIMIT 10",
        )?;
        let rows = stmt.query_map([name], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        for row in rows {
            let (pkg_name, distro) = row?;
            let entry = hits.entry(pkg_name).or_default();
            if let Some(d) = distro
                && !entry.contains(&d)
            {
                entry.push(d);
            }
        }
    }

    // 2. Search repository_packages by prefix
    {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT rp.name, r.name
             FROM repository_packages rp
             JOIN repositories r ON r.id = rp.repository_id
             WHERE rp.name LIKE ?1 || '%'
             LIMIT 10",
        )?;
        let rows = stmt.query_map([name], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (pkg_name, repo_name) = row?;
            let entry = hits.entry(pkg_name).or_default();
            if !entry.contains(&repo_name) {
                entry.push(repo_name);
            }
        }
    }

    // Remove exact match (the user already tried it)
    hits.remove(name);

    // Collect, sort by name, and take top 5
    let mut results: Vec<(String, String)> = hits
        .into_iter()
        .map(|(pkg, distros)| {
            let info = if distros.is_empty() {
                String::new()
            } else {
                distros.join(", ")
            };
            (pkg, info)
        })
        .collect();
    results.sort_by(|a, b| a.0.cmp(&b.0));
    results.truncate(5);
    Ok(results)
}

/// Print "did you mean?" suggestions when a package is not found.
///
/// Silently does nothing if the DB query fails (don't make a bad error worse).
fn print_package_suggestions(conn: &rusqlite::Connection, package_name: &str) {
    if let Ok(suggestions) = find_package_suggestions(conn, package_name)
        && !suggestions.is_empty()
    {
        eprintln!("\nDid you mean:");
        for (name, distros) in suggestions.iter().take(5) {
            if distros.is_empty() {
                eprintln!("  {name}");
            } else {
                eprintln!("  {name:<20} ({distros})");
            }
        }
        let stem = package_name.split('-').next().unwrap_or(package_name);
        eprintln!("\nUse 'conary canonical search {}' for more options.", stem);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_core::repository::{RepositorySourceKind, RepositorySourceMetadata};
    use std::path::PathBuf;

    #[test]
    fn install_provenance_from_resolved_copies_source_kind() {
        let resolved = ResolvedPackage {
            path: PathBuf::from("/tmp/example.ccs"),
            _temp_dir: None,
            source_type: ResolvedSourceType::Ccs,
            repository_provenance: Some(RepositorySourceMetadata {
                repository_id: 77,
                source_identity: Some("fedora-44".to_string()),
                source_profile: Some("fedora-44".to_string()),
                version_scheme: conary_core::repository::versioning::VersionScheme::Rpm,
                source_kind: RepositorySourceKind::Static,
            }),
        };

        let provenance = install_provenance_from_resolved(&resolved)
            .expect("repository provenance should be copied");

        assert_eq!(provenance.repository_id, 77);
        assert_eq!(provenance.source_profile.as_deref(), Some("fedora-44"));
        assert_eq!(
            provenance.version_scheme,
            conary_core::repository::versioning::VersionScheme::Rpm
        );
        assert_eq!(provenance.source_kind, RepositorySourceKind::Static);
    }
}
