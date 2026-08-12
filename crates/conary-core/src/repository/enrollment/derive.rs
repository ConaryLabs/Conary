// conary-core/src/repository/enrollment/derive.rs

//! Exact pre-mutation repository-intent derivation from package payloads.

use super::{
    LastOwnerDisposition, PACKAGE_REPOSITORY_ENROLLMENT_SCHEMA, PackageProjectionReference,
    PackageProjectionRole, PackageRepositoryEnrollmentIntent, RepositoryDefinition,
    RepositoryPolicyScopeInput, RepositorySourceStreamInput, RepositoryUpdatePolicyInput,
};
use crate::error::{Error, Result};
use crate::packages::payload::PackagePayloadFile;
use crate::repository::declarations::dnf::{
    DnfEndpointKind, DnfOptionName, DnfRepoDocument, DnfRepository,
};
use crate::repository::{
    OpenPgpTrustRoot, RepositoryParserConfig, RepositoryTrustPolicy, RpmMetadataAuthority,
};
use base64::Engine;
use openpgp::cert::CertParser;
use openpgp::parse::Parse;
use sequoia_openpgp as openpgp;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Component, Path};
use url::Url;

const MAX_DECLARATION_BYTES: u64 = 1024 * 1024;
const MAX_TRUST_BYTES: u64 = 4 * 1024 * 1024;

/// Derive every RPM repository declaration placed in DNF's exact package
/// payload ABI. Placement selects the consumer grammar; only a successful,
/// closed DNF parse supplies repository authority.
pub fn derive_rpm_repository_enrollments(
    payloads: &[PackagePayloadFile],
    architecture: &str,
    source_profile: Option<&str>,
) -> Result<Vec<PackageRepositoryEnrollmentIntent>> {
    if architecture.is_empty() || architecture.trim() != architecture {
        return Err(Error::ConfigError(
            "RPM repository enrollment requires an exact architecture".to_string(),
        ));
    }
    let by_path = payload_index(payloads)?;
    let mut intents = Vec::new();
    for (path, payload) in &by_path {
        if !is_dnf_repository_path(path) {
            continue;
        }
        let bytes = read_verified(payload, MAX_DECLARATION_BYTES, "DNF declaration")?;
        let source = std::str::from_utf8(&bytes).map_err(|error| {
            Error::ParseError(format!("DNF declaration '{path}' is not UTF-8: {error}"))
        })?;
        let document = DnfRepoDocument::parse(path, source).map_err(|error| {
            Error::ParseError(format!("invalid package DNF declaration '{path}': {error}"))
        })?;
        if document.repositories.is_empty() {
            return Err(Error::ConfigError(format!(
                "package DNF declaration '{path}' contains no repository sections"
            )));
        }
        intents.push(intent_from_dnf_document(
            path,
            payload,
            &document,
            &by_path,
            architecture,
            source_profile,
        )?);
    }
    intents.sort_by(|left, right| left.intent_id.cmp(&right.intent_id));
    Ok(intents)
}

fn intent_from_dnf_document(
    path: &str,
    declaration: &PackagePayloadFile,
    document: &DnfRepoDocument,
    payloads: &BTreeMap<String, &PackagePayloadFile>,
    architecture: &str,
    source_profile: Option<&str>,
) -> Result<PackageRepositoryEnrollmentIntent> {
    let mut repositories = Vec::new();
    let mut key_paths = BTreeSet::new();
    for repository in &document.repositories {
        let (definition, referenced_keys) =
            definition_from_dnf(repository, payloads, architecture, source_profile)?;
        repositories.push(definition);
        key_paths.extend(referenced_keys);
    }
    repositories.sort_by(|left, right| left.name.cmp(&right.name));
    let mut projections = vec![projection(
        declaration,
        path,
        PackageProjectionRole::Declaration,
    )?];
    for key_path in key_paths {
        let key = payloads.get(&key_path).ok_or_else(|| {
            Error::ConfigError(format!(
                "DNF declaration '{path}' references absent package trust object '{key_path}'"
            ))
        })?;
        projections.push(projection(
            key,
            &key_path,
            PackageProjectionRole::TrustRoot,
        )?);
    }
    let path_identity = crate::hash::sha256(path.as_bytes());
    let intent = PackageRepositoryEnrollmentIntent {
        schema: PACKAGE_REPOSITORY_ENROLLMENT_SCHEMA,
        intent_id: format!("rpm-dnf-{path_identity}"),
        repositories,
        projections,
        last_owner: LastOwnerDisposition::RemoveWhenUnowned,
    };
    intent.validate()?;
    Ok(intent)
}

fn definition_from_dnf(
    repository: &DnfRepository,
    payloads: &BTreeMap<String, &PackagePayloadFile>,
    architecture: &str,
    source_profile: Option<&str>,
) -> Result<(RepositoryDefinition, BTreeSet<String>)> {
    if let Some(source_profile) = source_profile {
        let profile = crate::repository::supported_profiles::profile_by_public_id(source_profile)
            .ok_or_else(|| {
            Error::ConfigError(format!(
                "DNF repository '{}' declares unknown feed projection '{source_profile}'",
                repository.id
            ))
        })?;
        if profile.package_format()
            != crate::repository::supported_profiles::ProfilePackageFormat::Rpm
        {
            return Err(Error::ConfigError(format!(
                "DNF repository '{}' feed projection '{source_profile}' does not own RPM semantics",
                repository.id
            )));
        }
    }
    let (endpoint_kind, endpoint_template) = repository.effective_endpoint().ok_or_else(|| {
        Error::ConfigError(format!(
            "DNF repository '{}' has no effective metadata endpoint",
            repository.id
        ))
    })?;
    let endpoint = expand_rpm_endpoint(endpoint_template, architecture)?;
    let enabled = repository
        .explicit_enabled()
        .map_err(|error| Error::ParseError(error.to_string()))?
        .unwrap_or(true);
    let package_check = required_bool(
        repository,
        &[DnfOptionName::PkgGpgcheck, DnfOptionName::Gpgcheck],
        "package signatures",
    )?;
    if !package_check {
        return Err(Error::ConfigError(format!(
            "DNF repository '{}' disables required package signature verification",
            repository.id
        )));
    }
    let (roots, key_paths) = roots_from_dnf(repository, payloads)?;
    let metadata = match endpoint_kind {
        DnfEndpointKind::Metalink => RpmMetadataAuthority::Metalink {
            url: endpoint.clone(),
        },
        DnfEndpointKind::Baseurl | DnfEndpointKind::Mirrorlist => {
            if !required_bool(
                repository,
                &[DnfOptionName::RepoGpgcheck],
                "repository metadata",
            )? {
                return Err(Error::ConfigError(format!(
                    "DNF repository '{}' disables required repository metadata signature verification",
                    repository.id
                )));
            }
            RpmMetadataAuthority::OpenPgp {
                keys: roots.clone(),
            }
        }
    };
    let priority = last_i32(repository, DnfOptionName::Priority)?.unwrap_or(0);
    let metadata_expire = last_i32(repository, DnfOptionName::MetadataExpire)?.unwrap_or(3600);
    let endpoint_identity = crate::hash::sha256(endpoint.as_bytes());
    let source_identity = format!("rpm-source-{endpoint_identity}");
    let repository_identity = repository.id.clone();
    Ok((
        RepositoryDefinition {
            name: repository.id.clone(),
            source_identity,
            repository_identity: repository_identity.clone(),
            scope: RepositoryPolicyScopeInput::Repository {
                identity: repository_identity,
            },
            stream: RepositorySourceStreamInput::Rolling {
                identity: endpoint_identity,
            },
            update: RepositoryUpdatePolicyInput::Follow,
            metadata_url: endpoint,
            content_url: None,
            parser: RepositoryParserConfig::Rpm {
                architecture: architecture.to_string(),
            },
            trust: RepositoryTrustPolicy::Rpm {
                metadata,
                package_keys: roots,
            },
            enabled,
            priority,
            metadata_expire,
            source_profile: source_profile.map(str::to_string),
        },
        key_paths,
    ))
}

fn expand_rpm_endpoint(template: &str, architecture: &str) -> Result<String> {
    let mut expanded = String::with_capacity(template.len());
    let mut remaining = template;
    while let Some(index) = remaining.find('$') {
        expanded.push_str(&remaining[..index]);
        remaining = &remaining[index + 1..];
        let (name, rest) = if let Some(braced) = remaining.strip_prefix('{') {
            let end = braced.find('}').ok_or_else(|| {
                Error::ConfigError(format!(
                    "RPM repository endpoint '{template}' has an unterminated variable"
                ))
            })?;
            (&braced[..end], &braced[end + 1..])
        } else {
            let end = remaining
                .find(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
                .unwrap_or(remaining.len());
            (&remaining[..end], &remaining[end..])
        };
        match name {
            "arch" | "basearch" => expanded.push_str(architecture),
            _ => {
                return Err(Error::ConfigError(format!(
                    "RPM repository endpoint '{template}' requires unavailable variable '${name}' authority"
                )));
            }
        }
        remaining = rest;
    }
    expanded.push_str(remaining);
    let parsed = Url::parse(&expanded).map_err(|error| {
        Error::ConfigError(format!(
            "expanded RPM repository endpoint '{expanded}' is invalid: {error}"
        ))
    })?;
    if parsed.scheme() != "https" || parsed.host_str().is_none() {
        return Err(Error::ConfigError(format!(
            "RPM repository endpoint '{expanded}' must be absolute HTTPS"
        )));
    }
    Ok(expanded)
}

fn required_bool(repository: &DnfRepository, names: &[DnfOptionName], label: &str) -> Result<bool> {
    let value = names.iter().find_map(|name| {
        repository
            .options
            .iter()
            .rev()
            .find(|option| &option.name == name)
    });
    let Some(value) = value else {
        return Err(Error::ConfigError(format!(
            "DNF repository '{}' does not declare exact {label} verification authority",
            repository.id
        )));
    };
    match value.raw_value.trim().to_ascii_lowercase().as_str() {
        "1" | "yes" | "true" | "on" => Ok(true),
        "0" | "no" | "false" | "off" => Ok(false),
        _ => Err(Error::ConfigError(format!(
            "DNF repository '{}' has invalid {label} value '{}'",
            repository.id, value.raw_value
        ))),
    }
}

fn last_i32(repository: &DnfRepository, name: DnfOptionName) -> Result<Option<i32>> {
    repository
        .options
        .iter()
        .rev()
        .find(|option| option.name == name)
        .map(|option| {
            option.raw_value.trim().parse::<i32>().map_err(|_| {
                Error::ConfigError(format!(
                    "DNF repository '{}' has non-integral {:?} value '{}'",
                    repository.id, name, option.raw_value
                ))
            })
        })
        .transpose()
}

fn roots_from_dnf(
    repository: &DnfRepository,
    payloads: &BTreeMap<String, &PackagePayloadFile>,
) -> Result<(Vec<OpenPgpTrustRoot>, BTreeSet<String>)> {
    let values = repository
        .options
        .iter()
        .filter(|option| option.name == DnfOptionName::Gpgkey)
        .flat_map(|option| option.raw_value.split_whitespace());
    let mut roots = Vec::new();
    let mut paths = BTreeSet::new();
    for value in values {
        let path = local_key_path(value)?;
        let payload = payloads.get(&path).ok_or_else(|| {
            Error::ConfigError(format!(
                "DNF repository '{}' references trust object '{path}' outside its package payload",
                repository.id
            ))
        })?;
        let bytes = read_verified(payload, MAX_TRUST_BYTES, "OpenPGP trust object")?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let url = format!("data:application/pgp-keys;base64,{encoded}");
        let parser = CertParser::from_bytes(&bytes).map_err(|error| {
            Error::ParseError(format!(
                "failed to parse OpenPGP trust object '{path}': {error}"
            ))
        })?;
        let mut count = 0_usize;
        for certificate in parser {
            let certificate = certificate.map_err(|error| {
                Error::ParseError(format!("malformed OpenPGP trust object '{path}': {error}"))
            })?;
            roots.push(OpenPgpTrustRoot::new(
                url.clone(),
                certificate.fingerprint().to_hex(),
            )?);
            count += 1;
        }
        if count == 0 {
            return Err(Error::ConfigError(format!(
                "OpenPGP trust object '{path}' contains no certificates"
            )));
        }
        paths.insert(path);
    }
    roots.sort_by(|left, right| left.fingerprint.cmp(&right.fingerprint));
    roots.dedup_by(|left, right| left.fingerprint == right.fingerprint);
    if roots.is_empty() {
        return Err(Error::ConfigError(format!(
            "DNF repository '{}' has no package-carried OpenPGP trust roots",
            repository.id
        )));
    }
    Ok((roots, paths))
}

fn local_key_path(value: &str) -> Result<String> {
    if value.starts_with('/') {
        return normalize_guest_path(value);
    }
    let url = Url::parse(value).map_err(|_| {
        Error::ConfigError(format!(
            "DNF gpgkey '{value}' is not an absolute package path or file URL"
        ))
    })?;
    if url.scheme() != "file" {
        return Err(Error::ConfigError(format!(
            "DNF gpgkey '{value}' is remote or uses an unsupported scheme; package enrollment requires exact carried trust"
        )));
    }
    let path = url.to_file_path().map_err(|_| {
        Error::ConfigError(format!("DNF gpgkey '{value}' is not a local file path"))
    })?;
    normalize_guest_path(path.to_string_lossy().as_ref())
}

fn payload_index(payloads: &[PackagePayloadFile]) -> Result<BTreeMap<String, &PackagePayloadFile>> {
    let mut by_path = BTreeMap::new();
    for payload in payloads {
        let path = normalize_guest_path(&payload.path)?;
        if by_path.insert(path.clone(), payload).is_some() {
            return Err(Error::ConfigError(format!(
                "package payload repeats normalized path '{path}'"
            )));
        }
    }
    Ok(by_path)
}

fn is_dnf_repository_path(path: &str) -> bool {
    let Some(name) = Path::new(path).file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    Path::new(path)
        .parent()
        .is_some_and(|parent| parent == Path::new("/etc/yum.repos.d"))
        && name.ends_with(".repo")
        && name.len() > ".repo".len()
}

fn normalize_guest_path(path: &str) -> Result<String> {
    let absolute = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    if Path::new(&absolute).components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::CurDir | Component::Prefix(_)
        )
    }) {
        return Err(Error::ConfigError(format!(
            "package repository path '{path}' is not normalized"
        )));
    }
    Ok(absolute)
}

fn read_verified(payload: &PackagePayloadFile, limit: u64, label: &str) -> Result<Vec<u8>> {
    let authority = payload.content_authority.as_ref().ok_or_else(|| {
        Error::ConfigError(format!(
            "{label} '{}' is not a regular payload object",
            payload.path
        ))
    })?;
    if authority.size > limit {
        return Err(Error::ConfigError(format!(
            "{label} '{}' is {} bytes; limit is {limit}",
            payload.path, authority.size
        )));
    }
    let mut bytes = Vec::with_capacity(authority.size as usize);
    payload
        .open_content()?
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != authority.size || crate::hash::sha256(&bytes) != authority.sha256 {
        return Err(Error::ConflictError(format!(
            "{label} '{}' content changed after package authentication",
            payload.path
        )));
    }
    Ok(bytes)
}

fn projection(
    payload: &PackagePayloadFile,
    path: &str,
    role: PackageProjectionRole,
) -> Result<PackageProjectionReference> {
    let authority = payload.content_authority.as_ref().ok_or_else(|| {
        Error::ConfigError(format!(
            "repository projection '{path}' is not a regular payload object"
        ))
    })?;
    Ok(PackageProjectionReference {
        path: path.to_string(),
        sha256: authority.sha256.clone(),
        mode: payload.node.mode & 0o7777,
        role,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packages::payload::ReopenablePayload;
    use crate::payload::{PayloadContentAuthority, PayloadNode};
    use openpgp::cert::prelude::CertBuilder;
    use openpgp::serialize::Serialize;

    fn payload(path: &str, bytes: Vec<u8>) -> PackagePayloadFile {
        PackagePayloadFile::new(
            path.to_string(),
            PayloadNode::regular(0o644),
            Some(PayloadContentAuthority {
                sha256: crate::hash::sha256(&bytes),
                size: bytes.len() as u64,
            }),
            Some(ReopenablePayload::from_in_memory_bytes(bytes)),
        )
        .unwrap()
    }

    fn certificate() -> Vec<u8> {
        let (certificate, _) = CertBuilder::new()
            .add_userid("repo@example.test")
            .generate()
            .unwrap();
        let mut bytes = Vec::new();
        certificate.serialize(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn derives_closed_chrome_shaped_rpm_repository_authority() {
        let declaration = b"[browser]\nname=Browser\nenabled=1\nbaseurl=https://repo.example/rpm/stable/$basearch\ngpgcheck=1\nrepo_gpgcheck=1\ngpgkey=file:///etc/pki/rpm-gpg/browser.gpg\npriority=20\n".to_vec();
        let payloads = vec![
            payload("etc/yum.repos.d/browser.repo", declaration),
            payload("etc/pki/rpm-gpg/browser.gpg", certificate()),
        ];

        let intents =
            derive_rpm_repository_enrollments(&payloads, "x86_64", Some("fedora-44")).unwrap();

        assert_eq!(intents.len(), 1);
        assert_eq!(intents[0].repositories[0].name, "browser");
        assert_eq!(intents[0].repositories[0].priority, 20);
        assert_eq!(
            intents[0].repositories[0].metadata_url,
            "https://repo.example/rpm/stable/x86_64"
        );
        assert_eq!(intents[0].projections.len(), 2);
        assert!(matches!(
            intents[0].repositories[0].trust,
            RepositoryTrustPolicy::Rpm {
                metadata: RpmMetadataAuthority::OpenPgp { .. },
                ..
            }
        ));
    }

    #[test]
    fn remote_or_missing_trust_blocks_before_intent_exists() {
        let declaration = b"[browser]\nbaseurl=https://repo.example/rpm\ngpgcheck=1\nrepo_gpgcheck=1\ngpgkey=https://repo.example/key.gpg\n".to_vec();
        let error = derive_rpm_repository_enrollments(
            &[payload("etc/yum.repos.d/browser.repo", declaration)],
            "x86_64",
            Some("fedora-44"),
        )
        .unwrap_err();

        assert!(error.to_string().contains("exact carried trust"));
    }

    #[test]
    fn repository_payload_without_named_feed_projection_derives_exact_source_identity() {
        let declaration = b"[browser]\nbaseurl=https://repo.example/rpm\ngpgcheck=1\nrepo_gpgcheck=1\ngpgkey=file:///etc/pki/rpm-gpg/browser.gpg\n".to_vec();
        let payloads = vec![
            payload("etc/yum.repos.d/browser.repo", declaration),
            payload("etc/pki/rpm-gpg/browser.gpg", certificate()),
        ];

        let intents = derive_rpm_repository_enrollments(&payloads, "x86_64", None).unwrap();

        assert_eq!(intents.len(), 1);
        let repository = &intents[0].repositories[0];
        assert_eq!(repository.source_profile, None);
        assert!(repository.source_identity.starts_with("rpm-source-"));
    }
}
