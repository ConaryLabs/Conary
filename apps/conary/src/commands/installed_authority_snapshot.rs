// apps/conary/src/commands/installed_authority_snapshot.rs

//! Exact installed-state authority retained by rollback metadata.

mod capture;

use anyhow::{Context, Result, bail};
use conary_core::capability::CapabilityDeclaration;
use conary_core::ccs::manifest::FileCapability;
use conary_core::ccs::native_transaction::NativePackageIdentity;
use conary_core::db::models::{
    ComponentDepType, ConfigSource, ConfigStatus, DirectoryClaimAnchorPolicy, InstallReason,
    InstallSource, TroveType,
};
use conary_core::packages::InstalledPackageIdentity;
use conary_core::payload::PayloadNodeKind;
use conary_core::payload::{PayloadContentAuthority, ResolvedPayloadNode};
use conary_core::repository::dependency_model::{
    DebianMultiArch, ProvideVersionRelation, RepositoryRequirementGroup,
};
use conary_core::repository::versioning::VersionScheme;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub(crate) use capture::{capture_materialized_directory_snapshots, capture_trove_snapshot};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TroveSnapshot {
    pub name: String,
    pub version: String,
    pub package_release: Option<String>,
    pub trove_type: TroveType,
    pub architecture: Option<String>,
    pub debian_multi_arch: Option<DebianMultiArch>,
    pub description: Option<String>,
    pub installed_at: String,
    pub install_source: InstallSource,
    pub install_reason: InstallReason,
    pub flavor_spec: Option<String>,
    pub pinned: bool,
    pub selection_reason: Option<String>,
    pub label_id: Option<i64>,
    pub orphan_since: Option<String>,
    pub source_profile: Option<String>,
    pub version_scheme: VersionScheme,
    pub native_package_identity: Option<InstalledPackageIdentity>,
    pub installed_from_repository_id: Option<i64>,
    pub flavors: Vec<FlavorSnapshot>,
    pub provenance: Option<ProvenanceSnapshot>,
    pub installed_conversions: Vec<InstalledConversionSnapshot>,
    pub components: Vec<ComponentSnapshot>,
    pub collection_members: Vec<CollectionMemberSnapshot>,
    pub requirements: Vec<InstalledRequirementSnapshot>,
    pub provides: Vec<ProvideSnapshot>,
    pub config_files: Vec<ConfigFileSnapshot>,
    pub package_capabilities: Option<PackageCapabilitySnapshot>,
    pub installed_file_capabilities: Vec<InstalledFileCapabilitySnapshot>,
    pub directory_claims: Vec<DirectoryClaimSnapshot>,
    pub native_lifecycle: Option<NativeLifecycleSnapshot>,
    pub ccs_remove_hook: Option<CcsRemoveHookSnapshot>,
    pub derived_package_references: Vec<DerivedPackageReferenceSnapshot>,
    pub files: Vec<FileSnapshot>,
}

impl TroveSnapshot {
    pub(crate) fn validate(&self) -> Result<()> {
        require_text("trove name", &self.name)?;
        require_text("trove version", &self.version)?;
        require_text("trove installed_at", &self.installed_at)?;
        if let Some(release) = self.package_release.as_deref() {
            conary_core::repository::versioning::validate_package_release(release)
                .context("rollback snapshot has an invalid signed CCS release")?;
        }
        match (self.version_scheme, self.debian_multi_arch) {
            (VersionScheme::Debian, Some(_))
            | (VersionScheme::Conary | VersionScheme::Rpm | VersionScheme::Arch, None) => {}
            (VersionScheme::Debian, None) => {
                bail!(
                    "rollback snapshot '{}' has Debian version authority without exact Multi-Arch metadata",
                    self.name
                );
            }
            (_, Some(_)) => {
                bail!(
                    "rollback snapshot '{}' carries Debian Multi-Arch metadata under the {} version scheme",
                    self.name,
                    self.version_scheme.as_str()
                );
            }
        }
        if self.architecture.as_deref() == Some("") {
            bail!(
                "rollback snapshot '{}' has an empty architecture",
                self.name
            );
        }

        if self.trove_type != TroveType::Collection && !self.collection_members.is_empty() {
            bail!(
                "rollback snapshot '{}' has collection members but is not a collection",
                self.name
            );
        }

        unique_text(
            "flavor key",
            self.flavors.iter().map(|flavor| flavor.key.as_str()),
        )?;
        unique_text(
            "component name",
            self.components
                .iter()
                .map(|component| component.name.as_str()),
        )?;
        unique_text(
            "collection member",
            self.collection_members
                .iter()
                .map(|member| member.member_name.as_str()),
        )?;
        let mut provided_identities = BTreeSet::new();
        for provide in &self.provides {
            require_text("provided capability", &provide.capability)?;
            if provide.version.is_some() != provide.version_relation.is_some() {
                bail!(
                    "rollback snapshot capability '{}' must pair its version boundary and relation",
                    provide.capability
                );
            }
            let identity = serde_json::to_string(provide)
                .context("failed to encode rollback provided-capability identity")?;
            if !provided_identities.insert(identity) {
                bail!(
                    "rollback snapshot repeats provided capability identity '{}'",
                    provide.capability
                );
            }
        }
        unique_text(
            "installed conversion checksum",
            self.installed_conversions
                .iter()
                .map(|conversion| conversion.original_checksum.as_str()),
        )?;
        unique_text(
            "config path",
            self.config_files.iter().map(|config| config.path.as_str()),
        )?;
        unique_text(
            "file capability path",
            self.installed_file_capabilities
                .iter()
                .map(|capability| capability.capability.path.as_str()),
        )?;
        unique_text(
            "payload path",
            self.files.iter().map(|file| file.path.as_str()),
        )?;
        unique_text(
            "directory claim path",
            self.directory_claims
                .iter()
                .map(|claim| claim.path.as_str()),
        )?;

        let component_names = self
            .components
            .iter()
            .map(|component| component.name.as_str())
            .collect::<BTreeSet<_>>();
        for component in &self.components {
            component.validate()?;
        }
        let file_paths = self
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<BTreeSet<_>>();
        let materialization_target_paths = self
            .directory_claims
            .iter()
            .filter_map(|claim| claim.materialization_target_path.as_deref())
            .collect::<BTreeSet<_>>();
        for file in &self.files {
            require_text("file installed_at", &file.installed_at)?;
            if matches!(file.node.source.kind, PayloadNodeKind::Directory)
                && !materialization_target_paths.contains(file.path.as_str())
            {
                bail!(
                    "rollback snapshot directory '{}' must use directory-claim or materialization-target authority",
                    file.path
                );
            }
            if let Some(component_name) = file.component.as_deref()
                && !component_names.contains(component_name)
            {
                bail!(
                    "rollback snapshot file '{}' references missing component '{}'",
                    file.path,
                    component_name
                );
            }
        }
        for claim in &self.directory_claims {
            require_text("directory claim claimed_at", &claim.claimed_at)?;
            if let Some(target_path) = claim.materialization_target_path.as_deref() {
                require_text("directory claim materialization target", target_path)?;
                if target_path == claim.path {
                    bail!(
                        "rollback snapshot directory claim '{}' cannot target itself",
                        claim.path
                    );
                }
            }
            claim
                .node
                .validate()
                .context("rollback snapshot directory claim node is invalid")?;
            if !matches!(claim.node.source.kind, PayloadNodeKind::Directory) {
                bail!(
                    "rollback snapshot directory claim '{}' is not a directory",
                    claim.path
                );
            }
            if let Some(component) = claim.component.as_deref()
                && !component_names.contains(component)
            {
                bail!(
                    "rollback snapshot directory claim '{}' references missing component '{}'",
                    claim.path,
                    component
                );
            }
            if let Some(anchor) = &claim.anchor {
                require_text("directory anchor installed_at", &anchor.installed_at)?;
                anchor
                    .materialized_node
                    .validate()
                    .context("rollback snapshot directory anchor node is invalid")?;
                if !claim
                    .anchor_policy
                    .accepts(&anchor.materialized_node.source.kind)
                {
                    bail!(
                        "rollback snapshot materialized directory anchor '{}' is not accepted by policy '{}'",
                        claim.path,
                        claim.anchor_policy.as_str()
                    );
                }
                if let Some(component) = anchor.component.as_deref()
                    && !component_names.contains(component)
                {
                    bail!(
                        "rollback snapshot directory anchor '{}' references missing component '{}'",
                        claim.path,
                        component
                    );
                }
            }
        }
        for config in &self.config_files {
            if config.file_backed && !file_paths.contains(config.path.as_str()) {
                bail!(
                    "rollback snapshot config '{}' references a missing payload file",
                    config.path
                );
            }
            for backup in &config.backups {
                require_text("config backup hash", &backup.backup_hash)?;
                require_text("config backup reason", &backup.reason)?;
                require_text("config backup created_at", &backup.created_at)?;
            }
        }
        for requirement in &self.requirements {
            conary_core::db::models::InstalledRequirementGroup::from_group(
                0,
                requirement.version_scheme,
                requirement.requirement.clone(),
            )
            .with_context(|| {
                format!(
                    "rollback snapshot '{}' has an invalid installed requirement",
                    self.name
                )
            })?;
        }
        if let Some(capabilities) = &self.package_capabilities {
            capabilities
                .declaration
                .validate()
                .context("rollback snapshot package capability declaration is invalid")?;
            if i64::from(capabilities.declaration.version) != capabilities.declaration_version {
                bail!(
                    "rollback snapshot package capability declaration version {} differs from persisted projection {}",
                    capabilities.declaration.version,
                    capabilities.declaration_version
                );
            }
            require_text("package capability declared_at", &capabilities.declared_at)?;
        }
        for capability in &self.installed_file_capabilities {
            capability
                .capability
                .validate()
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            require_text(
                "installed file capability created_at",
                &capability.created_at,
            )?;
            if !file_paths.contains(capability.capability.path.as_str()) {
                bail!(
                    "rollback snapshot file capability '{}' references a missing payload file",
                    capability.capability.path
                );
            }
        }

        let mut derived_ids = BTreeSet::new();
        for reference in &self.derived_package_references {
            if !reference.parent && !reference.built {
                bail!(
                    "rollback snapshot derived-package reference {} has no role",
                    reference.derived_package_id
                );
            }
            if !derived_ids.insert(reference.derived_package_id) {
                bail!(
                    "rollback snapshot repeats derived-package reference {}",
                    reference.derived_package_id
                );
            }
        }

        if let Some(identity) = &self.native_package_identity {
            identity
                .validate()
                .context("rollback snapshot native package identity is invalid")?;
        }
        if let Some(native) = &self.native_lifecycle {
            require_text("native lifecycle installed_at", &native.installed_at)?;
            let bundle: conary_core::ccs::native_lifecycle::NativeLifecycleBundle =
                toml::from_str(&native.bundle_toml)
                    .context("rollback snapshot native lifecycle TOML is invalid")?;
            bundle
                .validate()
                .context("rollback snapshot native lifecycle bundle is invalid")?;
            if native.source_format != bundle.source_format.as_str()
                || native.source_family != bundle.source_family
                || native.source_profile != bundle.source_profile
                || native.source_release != bundle.source_release
                || native.source_arch != bundle.source_arch
                || native.source_package != bundle.source_package
                || native.source_version != bundle.source_version
                || native.scriptlet_fidelity != bundle.scriptlet_fidelity.as_str()
                || native.evidence_digest != bundle.evidence_digest
            {
                bail!(
                    "rollback snapshot '{}' native lifecycle row projections differ from bundle authority",
                    self.name
                );
            }
            if bundle.source_package != self.name
                || bundle.source_version != self.version
                || bundle.source_arch != self.architecture
                || bundle.version_scheme.repository_scheme() != self.version_scheme
            {
                bail!(
                    "rollback snapshot '{}' native lifecycle identity differs from installed package identity",
                    self.name
                );
            }
            for awaited in &native.awaited_packages {
                require_text(
                    "native lifecycle awaited package name",
                    &awaited.package_name,
                )?;
                require_text(
                    "native lifecycle awaited package version",
                    &awaited.package_version,
                )?;
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn test_package(
        name: impl Into<String>,
        version: impl Into<String>,
        files: Vec<FileSnapshot>,
    ) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            package_release: None,
            trove_type: TroveType::Package,
            architecture: None,
            debian_multi_arch: None,
            description: None,
            installed_at: "2026-01-01T00:00:00Z".to_string(),
            install_source: InstallSource::File,
            install_reason: InstallReason::Explicit,
            flavor_spec: None,
            pinned: false,
            selection_reason: Some("Explicitly installed".to_string()),
            label_id: None,
            orphan_since: None,
            source_profile: None,
            version_scheme: VersionScheme::Conary,
            native_package_identity: None,
            installed_from_repository_id: None,
            flavors: Vec::new(),
            provenance: None,
            installed_conversions: Vec::new(),
            components: Vec::new(),
            collection_members: Vec::new(),
            requirements: Vec::new(),
            provides: Vec::new(),
            config_files: Vec::new(),
            package_capabilities: None,
            installed_file_capabilities: Vec::new(),
            directory_claims: Vec::new(),
            native_lifecycle: None,
            ccs_remove_hook: None,
            derived_package_references: Vec::new(),
            files,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FlavorSnapshot {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ComponentSnapshot {
    pub name: String,
    pub description: Option<String>,
    pub installed_at: String,
    pub is_installed: bool,
    pub dependencies: Vec<ComponentDependencySnapshot>,
    pub provides: Vec<ComponentProvideSnapshot>,
}

impl ComponentSnapshot {
    fn validate(&self) -> Result<()> {
        require_text("component name", &self.name)?;
        require_text("component installed_at", &self.installed_at)?;
        let mut dependencies = BTreeSet::new();
        for dependency in &self.dependencies {
            let key = (
                dependency.depends_on_package.as_deref(),
                dependency.depends_on_component.as_str(),
            );
            if !dependencies.insert(key) {
                bail!(
                    "rollback snapshot component '{}' repeats dependency {:?}",
                    self.name,
                    key
                );
            }
        }
        unique_text(
            "component provide",
            self.provides
                .iter()
                .map(|provide| provide.capability.as_str()),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ComponentDependencySnapshot {
    pub depends_on_component: String,
    pub depends_on_package: Option<String>,
    pub dependency_type: ComponentDepType,
    pub version_constraint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ComponentProvideSnapshot {
    pub capability: String,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CollectionMemberSnapshot {
    pub member_name: String,
    pub member_version: Option<String>,
    pub is_optional: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InstalledRequirementSnapshot {
    pub version_scheme: VersionScheme,
    pub requirement: RepositoryRequirementGroup,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProvideSnapshot {
    pub capability: String,
    pub version: Option<String>,
    pub version_relation: Option<ProvideVersionRelation>,
    pub kind: conary_core::repository::dependency_model::RepositoryCapabilityKind,
    pub version_scheme: VersionScheme,
    pub architecture_qualifier:
        conary_core::repository::dependency_model::ProvideArchitectureQualifier,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfigFileSnapshot {
    pub path: String,
    pub file_backed: bool,
    pub original_hash: Option<String>,
    pub original_md5: Option<String>,
    pub current_hash: Option<String>,
    pub noreplace: bool,
    pub ghost: bool,
    pub remove_on_upgrade: bool,
    pub status: ConfigStatus,
    pub modified_at: Option<String>,
    pub source: ConfigSource,
    pub backups: Vec<ConfigBackupSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ConfigBackupSnapshot {
    pub backup_hash: String,
    pub reason: String,
    pub changeset_id: Option<i64>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InstalledFileCapabilitySnapshot {
    pub capability: FileCapability,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DirectoryClaimSnapshot {
    pub path: String,
    pub node: ResolvedPayloadNode,
    pub component: Option<String>,
    pub anchor_policy: DirectoryClaimAnchorPolicy,
    pub materialization_target_path: Option<String>,
    pub claimed_at: String,
    pub anchor: Option<DirectoryAnchorSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DirectoryAnchorSnapshot {
    pub materialized_node: ResolvedPayloadNode,
    pub installed_at: String,
    pub component: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MaterializedDirectorySnapshot {
    pub path: String,
    pub materialized_node: ResolvedPayloadNode,
    pub anchor_policy: DirectoryClaimAnchorPolicy,
    pub installed_at: String,
    pub anchor_trove: StableTroveIdentitySnapshot,
    pub anchor_component: Option<String>,
}

impl MaterializedDirectorySnapshot {
    pub(crate) fn validate(&self) -> Result<()> {
        require_text("materialized directory path", &self.path)?;
        require_text("materialized directory installed_at", &self.installed_at)?;
        self.materialized_node
            .validate()
            .context("rollback materialized directory node is invalid")?;
        if !self
            .anchor_policy
            .accepts(&self.materialized_node.source.kind)
        {
            bail!(
                "rollback materialized directory '{}' is not accepted by policy '{}'",
                self.path,
                self.anchor_policy.as_str()
            );
        }
        self.anchor_trove.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StableTroveIdentitySnapshot {
    pub name: String,
    pub version: String,
    pub trove_type: TroveType,
    pub architecture: Option<String>,
    pub installed_at: String,
    pub version_scheme: VersionScheme,
}

impl StableTroveIdentitySnapshot {
    pub(crate) fn validate(&self) -> Result<()> {
        require_text("materialized directory anchor trove name", &self.name)?;
        require_text("materialized directory anchor trove version", &self.version)?;
        require_text(
            "materialized directory anchor trove installed_at",
            &self.installed_at,
        )?;
        if self.architecture.as_deref() == Some("") {
            bail!("materialized directory anchor trove architecture is empty");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PackageCapabilitySnapshot {
    pub declaration: CapabilityDeclaration,
    pub declaration_version: i64,
    pub declared_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeLifecycleSnapshot {
    pub source_format: String,
    pub source_family: String,
    pub source_profile: Option<String>,
    pub source_release: Option<String>,
    pub source_arch: Option<String>,
    pub source_package: String,
    pub source_version: String,
    pub scriptlet_fidelity: String,
    pub evidence_digest: Option<String>,
    pub bundle_toml: String,
    pub lifecycle_state: String,
    pub pending_triggers: Vec<String>,
    pub awaited_packages: Vec<NativePackageIdentity>,
    pub installed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CcsRemoveHookSnapshot {
    pub script: String,
    pub reversible: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileSnapshot {
    pub path: String,
    pub node: ResolvedPayloadNode,
    pub content: Option<PayloadContentAuthority>,
    pub installed_at: String,
    pub component: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DerivedPackageReferenceSnapshot {
    pub derived_package_id: i64,
    pub parent: bool,
    pub built: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InstalledConversionSnapshot {
    pub original_format: String,
    pub original_checksum: String,
    pub conversion_version: i32,
    pub converted_at: String,
    pub enhancement_version: i32,
    pub extracted_provenance_json: Option<String>,
    pub enhancement_status: String,
    pub enhancement_error: Option<String>,
    pub enhancement_attempted_at: Option<String>,
    pub enhancement_priority: Option<i32>,
    pub scriptlet_fidelity: String,
    pub evidence_digest: Option<String>,
    pub scriptlet_summary_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProvenanceSnapshot {
    pub source_url: Option<String>,
    pub source_branch: Option<String>,
    pub source_commit: Option<String>,
    pub build_host: Option<String>,
    pub build_time: Option<String>,
    pub builder: Option<String>,
    pub upstream_url: Option<String>,
    pub upstream_hash: Option<String>,
    pub git_repo: Option<String>,
    pub git_tag: Option<String>,
    pub fetch_timestamp: Option<String>,
    pub patches_json: Option<String>,
    pub recipe_hash: Option<String>,
    pub build_deps_json: Option<String>,
    pub host_arch: Option<String>,
    pub host_kernel: Option<String>,
    pub host_distro: Option<String>,
    pub build_start: Option<String>,
    pub build_end: Option<String>,
    pub build_log_hash: Option<String>,
    pub isolation_level: Option<String>,
    pub reproducibility_json: Option<String>,
    pub signatures_json: Option<String>,
    pub rekor_log_index: Option<i64>,
    pub sbom_spdx_hash: Option<String>,
    pub sbom_cyclonedx_hash: Option<String>,
    pub merkle_root: Option<String>,
    pub component_hashes_json: Option<String>,
    pub chunk_manifest_json: Option<String>,
    pub total_size: Option<i64>,
    pub file_count: Option<i64>,
    pub dna_hash: Option<String>,
}

fn require_text(context: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        bail!("rollback snapshot has an empty {context}");
    }
    Ok(())
}

fn unique_text<'a>(context: &str, values: impl IntoIterator<Item = &'a str>) -> Result<()> {
    let mut seen = BTreeSet::new();
    for value in values {
        require_text(context, value)?;
        if !seen.insert(value) {
            bail!("rollback snapshot repeats {context} '{value}'");
        }
    }
    Ok(())
}
