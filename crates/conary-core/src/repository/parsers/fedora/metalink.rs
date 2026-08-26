// conary-core/src/repository/parsers/fedora/metalink.rs

//! Fedora metalink authority for the rpm-md root object.

use crate::error::{Error, Result};
use quick_xml::encoding::Decoder;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;

const METALINK_NAMESPACE: &[u8] = b"http://www.metalinker.org/";
const MIRRORMANAGER_NAMESPACE: &[u8] = b"http://fedorahosted.org/mirrormanager";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ElementScope {
    Metalink,
    Files,
    RepomdFile,
    CurrentSize,
    CurrentVerification,
    CurrentSha256,
    Alternates,
    Alternate,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MetalinkRepomdIdentity {
    sha256: String,
    size: u64,
}

impl MetalinkRepomdIdentity {
    pub(super) fn verify(&self, repomd: &[u8]) -> Result<()> {
        if repomd.len() as u64 != self.size {
            return Err(Error::TrustError(format!(
                "RPM metalink authenticates repomd.xml as {} bytes but the repository served {} \
                 bytes",
                self.size,
                repomd.len()
            )));
        }
        let actual = crate::hash::sha256(repomd);
        if actual != self.sha256 {
            return Err(Error::TrustError(format!(
                "RPM metalink repomd.xml SHA-256 mismatch: expected {}, got {}",
                self.sha256, actual
            )));
        }
        Ok(())
    }
}

pub(super) fn parse_metalink_repomd_identity(xml: &[u8]) -> Result<MetalinkRepomdIdentity> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut scopes = Vec::new();
    let mut repomd_files = 0usize;
    let mut sha256_seen = false;
    let mut size_seen = false;
    let mut sha256_text = None;
    let mut size_text = None;

    loop {
        let decoder = reader.decoder();
        match reader.read_resolved_event_into(&mut buffer) {
            Ok((namespace, Event::Start(event))) => {
                let scope = classify_element(
                    decoder,
                    &namespace,
                    &event,
                    scopes.last().copied(),
                    &mut repomd_files,
                    &mut size_seen,
                    &mut sha256_seen,
                )?;
                scopes.push(scope);
            }
            Ok((namespace, Event::Empty(event))) => {
                classify_element(
                    decoder,
                    &namespace,
                    &event,
                    scopes.last().copied(),
                    &mut repomd_files,
                    &mut size_seen,
                    &mut sha256_seen,
                )?;
            }
            Ok((_, Event::Text(text))) => {
                let value = text
                    .xml_content(quick_xml::XmlVersion::Implicit1_0)
                    .map_err(|error| {
                        Error::ParseError(format!(
                            "failed to decode RPM metalink identity: {error}"
                        ))
                    })?
                    .trim()
                    .to_string();
                match scopes.last() {
                    Some(ElementScope::CurrentSize) => {
                        capture_identity_text(&mut size_text, value, "size")?;
                    }
                    Some(ElementScope::CurrentSha256) => {
                        capture_identity_text(&mut sha256_text, value, "SHA-256")?;
                    }
                    _ => {}
                }
            }
            Ok((_, Event::End(_))) => {
                scopes.pop();
            }
            Ok((_, Event::Eof)) => break,
            Err(error) => {
                return Err(Error::ParseError(format!(
                    "failed to parse RPM metalink: {error}"
                )));
            }
            _ => {}
        }
        buffer.clear();
    }

    if repomd_files != 1 {
        return Err(Error::ParseError(
            "RPM metalink has no exact repomd.xml file identity".to_string(),
        ));
    }
    let sha256 = sha256_text
        .ok_or_else(|| Error::ParseError("RPM metalink repomd.xml has no SHA-256".to_string()))?;
    validate_sha256(&sha256)?;
    let size = size_text
        .ok_or_else(|| Error::ParseError("RPM metalink repomd.xml has no size".to_string()))?
        .parse::<u64>()
        .map_err(|error| {
            Error::ParseError(format!("RPM metalink repomd.xml size is invalid: {error}"))
        })?;
    Ok(MetalinkRepomdIdentity { sha256, size })
}

fn classify_element(
    decoder: Decoder,
    namespace: &ResolveResult<'_>,
    event: &BytesStart<'_>,
    parent: Option<ElementScope>,
    repomd_files: &mut usize,
    size_seen: &mut bool,
    sha256_seen: &mut bool,
) -> Result<ElementScope> {
    let local_name = event.local_name();
    let local_name = local_name.as_ref();
    let in_metalink_namespace = namespace_matches(namespace, METALINK_NAMESPACE);
    let in_mirrormanager_namespace = namespace_matches(namespace, MIRRORMANAGER_NAMESPACE);

    if parent.is_none() && in_metalink_namespace && local_name == b"metalink" {
        return Ok(ElementScope::Metalink);
    }
    if parent == Some(ElementScope::Metalink) && in_metalink_namespace && local_name == b"files" {
        return Ok(ElementScope::Files);
    }
    if parent == Some(ElementScope::Files) && in_metalink_namespace && local_name == b"file" {
        let name = exact_attribute(decoder, event, b"name", "file name")?;
        if name.as_deref() == Some("repomd.xml") {
            *repomd_files += 1;
            if *repomd_files > 1 {
                return Err(Error::ParseError(
                    "RPM metalink repeats repomd.xml identity".to_string(),
                ));
            }
            return Ok(ElementScope::RepomdFile);
        }
        return Ok(ElementScope::Other);
    }
    if parent == Some(ElementScope::RepomdFile) && in_metalink_namespace && local_name == b"size" {
        if *size_seen {
            return Err(Error::ParseError(
                "RPM metalink repeats repomd.xml size".to_string(),
            ));
        }
        *size_seen = true;
        return Ok(ElementScope::CurrentSize);
    }
    if parent == Some(ElementScope::RepomdFile)
        && in_metalink_namespace
        && local_name == b"verification"
    {
        return Ok(ElementScope::CurrentVerification);
    }
    if parent == Some(ElementScope::CurrentVerification)
        && in_metalink_namespace
        && local_name == b"hash"
    {
        let hash_type = exact_attribute(decoder, event, b"type", "hash type")?;
        if hash_type.as_deref() == Some("sha256") {
            if *sha256_seen {
                return Err(Error::ParseError(
                    "RPM metalink repeats repomd.xml SHA-256".to_string(),
                ));
            }
            *sha256_seen = true;
            return Ok(ElementScope::CurrentSha256);
        }
        return Ok(ElementScope::Other);
    }
    if parent == Some(ElementScope::RepomdFile)
        && in_mirrormanager_namespace
        && local_name == b"alternates"
    {
        return Ok(ElementScope::Alternates);
    }
    if parent == Some(ElementScope::Alternates)
        && in_mirrormanager_namespace
        && local_name == b"alternate"
    {
        return Ok(ElementScope::Alternate);
    }
    Ok(ElementScope::Other)
}

fn exact_attribute(
    decoder: Decoder,
    event: &BytesStart<'_>,
    name: &[u8],
    label: &str,
) -> Result<Option<String>> {
    let mut value = None;
    for attribute in event.attributes() {
        let attribute = attribute.map_err(|error| {
            Error::ParseError(format!(
                "failed to parse RPM metalink {label} attribute: {error}"
            ))
        })?;
        if attribute.key.as_ref() != name {
            continue;
        }
        if value.is_some() {
            return Err(Error::ParseError(format!(
                "RPM metalink repeats {label} attribute"
            )));
        }
        value = Some(
            attribute
                .decoded_and_normalized_value(quick_xml::XmlVersion::Implicit1_0, decoder)
                .map_err(|error| {
                    Error::ParseError(format!("failed to decode RPM metalink {label}: {error}"))
                })?
                .into_owned(),
        );
    }
    Ok(value)
}

fn namespace_matches(namespace: &ResolveResult<'_>, expected: &[u8]) -> bool {
    matches!(namespace, ResolveResult::Bound(namespace) if namespace.as_ref() == expected)
}

fn capture_identity_text(target: &mut Option<String>, value: String, label: &str) -> Result<()> {
    if target.replace(value).is_some() {
        return Err(Error::ParseError(format!(
            "RPM metalink repomd.xml {label} contains nested or repeated text"
        )));
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<()> {
    if value.len() != 64
        || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
        || value.bytes().any(|byte| byte.is_ascii_uppercase())
    {
        return Err(Error::ParseError(
            "RPM metalink repomd.xml SHA256 must be exactly 64 lowercase hexadecimal digits"
                .to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRODUCTION_SHAPED_METALINK: &[u8] =
        include_bytes!("../../../../tests/fixtures/rpm/fedora-44-updates-metalink.xml");

    fn metalink(data: &[u8]) -> String {
        metalink_with_body(&format!(
            r#"<size>{}</size><verification><hash type="sha256">{}</hash></verification>"#,
            data.len(),
            crate::hash::sha256(data)
        ))
    }

    fn metalink_with_body(body: &str) -> String {
        format!(
            r#"<metalink xmlns="http://www.metalinker.org/" xmlns:mm0="http://fedorahosted.org/mirrormanager" xmlns:evil="https://example.test/evil"><files><file name="repomd.xml">{body}</file></files></metalink>"#
        )
    }

    #[test]
    fn exact_metalink_identity_accepts_only_matching_repomd() {
        let repomd = b"<repomd/>";
        let identity = parse_metalink_repomd_identity(metalink(repomd).as_bytes()).unwrap();
        identity.verify(repomd).unwrap();
        assert!(identity.verify(b"<repomd tampered='true'/>").is_err());
    }

    #[test]
    fn duplicate_repomd_identity_is_rejected() {
        let xml = format!(
            "<metalink xmlns=\"http://www.metalinker.org/\"><files>{0}{0}</files></metalink>",
            r#"<file name="repomd.xml"><size>1</size><hash type="sha256">aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa</hash></file>"#
        );
        assert!(parse_metalink_repomd_identity(xml.as_bytes()).is_err());
    }

    #[test]
    fn production_shaped_mirrormanager_alternates_do_not_override_current_identity() {
        let identity = parse_metalink_repomd_identity(PRODUCTION_SHAPED_METALINK).unwrap();
        identity.verify(b"<repomd/>").unwrap();
        assert!(identity.verify(b"<alternate/>").is_err());
    }

    #[test]
    fn alternates_cannot_supply_a_missing_current_identity() {
        let xml = metalink_with_body(
            r#"<mm0:alternates><mm0:alternate><size>9</size><verification><hash type="sha256">50095c0dd3ea786b68ccdfc6eaf4a30f893ab83aa88d29bdd787e957b888cb48</hash></verification></mm0:alternate></mm0:alternates>"#,
        );
        assert!(parse_metalink_repomd_identity(xml.as_bytes()).is_err());
    }

    #[test]
    fn duplicate_current_size_and_sha256_are_rejected() {
        let duplicate_size = metalink_with_body(
            r#"<size>9</size><size>9</size><verification><hash type="sha256">50095c0dd3ea786b68ccdfc6eaf4a30f893ab83aa88d29bdd787e957b888cb48</hash></verification>"#,
        );
        assert!(parse_metalink_repomd_identity(duplicate_size.as_bytes()).is_err());

        let duplicate_sha256 = metalink_with_body(
            r#"<size>9</size><verification><hash type="sha256">50095c0dd3ea786b68ccdfc6eaf4a30f893ab83aa88d29bdd787e957b888cb48</hash><hash type="sha256">aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa</hash></verification>"#,
        );
        assert!(parse_metalink_repomd_identity(duplicate_sha256.as_bytes()).is_err());
    }

    #[test]
    fn wrong_namespace_and_misleading_nesting_cannot_establish_identity() {
        let invalid_bodies = [
            r#"<evil:size>9</evil:size><evil:verification><evil:hash type="sha256">50095c0dd3ea786b68ccdfc6eaf4a30f893ab83aa88d29bdd787e957b888cb48</evil:hash></evil:verification>"#,
            r#"<wrapper><size>9</size><verification><hash type="sha256">50095c0dd3ea786b68ccdfc6eaf4a30f893ab83aa88d29bdd787e957b888cb48</hash></verification></wrapper>"#,
            r#"<evil:alternates><size>9</size><verification><hash type="sha256">50095c0dd3ea786b68ccdfc6eaf4a30f893ab83aa88d29bdd787e957b888cb48</hash></verification></evil:alternates>"#,
        ];
        for body in invalid_bodies {
            let xml = metalink_with_body(body);
            assert!(parse_metalink_repomd_identity(xml.as_bytes()).is_err());
        }

        let wrong_root_namespace = r#"<metalink xmlns="https://example.test/evil"><files><file name="repomd.xml"><size>9</size><verification><hash type="sha256">50095c0dd3ea786b68ccdfc6eaf4a30f893ab83aa88d29bdd787e957b888cb48</hash></verification></file></files></metalink>"#;
        assert!(parse_metalink_repomd_identity(wrong_root_namespace.as_bytes()).is_err());
    }

    #[test]
    fn namespaced_attributes_cannot_select_current_authority() {
        let wrong_file_name = r#"<metalink xmlns="http://www.metalinker.org/" xmlns:evil="https://example.test/evil"><files><file evil:name="repomd.xml"><size>9</size><verification><hash type="sha256">50095c0dd3ea786b68ccdfc6eaf4a30f893ab83aa88d29bdd787e957b888cb48</hash></verification></file></files></metalink>"#;
        assert!(parse_metalink_repomd_identity(wrong_file_name.as_bytes()).is_err());

        let wrong_hash_type = metalink_with_body(
            r#"<size>9</size><verification><hash evil:type="sha256">50095c0dd3ea786b68ccdfc6eaf4a30f893ab83aa88d29bdd787e957b888cb48</hash></verification>"#,
        );
        assert!(parse_metalink_repomd_identity(wrong_hash_type.as_bytes()).is_err());
    }
}
