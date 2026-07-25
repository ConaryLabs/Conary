// conary-core/src/repository/parsers/fedora/metalink.rs

//! Fedora metalink authority for the rpm-md root object.

use crate::error::{Error, Result};
use quick_xml::Reader;
use quick_xml::events::Event;

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
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut buffer = Vec::new();
    let mut in_repomd_file = false;
    let mut repomd_files = 0usize;
    let mut current_field = None;
    let mut sha256 = None;
    let mut size = None;

    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(event)) if event.local_name().as_ref() == b"file" => {
                let mut name = None;
                for attribute in event.attributes() {
                    let attribute = attribute.map_err(|error| {
                        Error::ParseError(format!(
                            "failed to parse RPM metalink file attribute: {error}"
                        ))
                    })?;
                    if attribute.key.local_name().as_ref() == b"name" {
                        name = Some(
                            attribute
                                .decoded_and_normalized_value(
                                    quick_xml::XmlVersion::Implicit1_0,
                                    reader.decoder(),
                                )
                                .map_err(|error| {
                                    Error::ParseError(format!(
                                        "failed to decode RPM metalink file name: {error}"
                                    ))
                                })?
                                .into_owned(),
                        );
                    }
                }
                in_repomd_file = name.as_deref() == Some("repomd.xml");
                if in_repomd_file {
                    repomd_files += 1;
                    if repomd_files > 1 {
                        return Err(Error::ParseError(
                            "RPM metalink repeats repomd.xml identity".to_string(),
                        ));
                    }
                }
            }
            Ok(Event::Start(event)) if in_repomd_file && event.local_name().as_ref() == b"size" => {
                if size.is_some() {
                    return Err(Error::ParseError(
                        "RPM metalink repeats repomd.xml size".to_string(),
                    ));
                }
                current_field = Some("size");
            }
            Ok(Event::Start(event)) if in_repomd_file && event.local_name().as_ref() == b"hash" => {
                let mut hash_type = None;
                for attribute in event.attributes() {
                    let attribute = attribute.map_err(|error| {
                        Error::ParseError(format!(
                            "failed to parse RPM metalink hash attribute: {error}"
                        ))
                    })?;
                    if attribute.key.local_name().as_ref() == b"type" {
                        hash_type = Some(
                            attribute
                                .decoded_and_normalized_value(
                                    quick_xml::XmlVersion::Implicit1_0,
                                    reader.decoder(),
                                )
                                .map_err(|error| {
                                    Error::ParseError(format!(
                                        "failed to decode RPM metalink hash type: {error}"
                                    ))
                                })?
                                .into_owned(),
                        );
                    }
                }
                current_field = (hash_type.as_deref() == Some("sha256")).then_some("sha256");
                if current_field.is_some() && sha256.is_some() {
                    return Err(Error::ParseError(
                        "RPM metalink repeats repomd.xml SHA-256".to_string(),
                    ));
                }
            }
            Ok(Event::Text(text)) if in_repomd_file => {
                let value = text
                    .xml_content(quick_xml::XmlVersion::Implicit1_0)
                    .map_err(|error| {
                        Error::ParseError(format!(
                            "failed to decode RPM metalink identity: {error}"
                        ))
                    })?
                    .trim()
                    .to_string();
                match current_field {
                    Some("size") => {
                        size = Some(value.parse::<u64>().map_err(|error| {
                            Error::ParseError(format!(
                                "RPM metalink repomd.xml size is invalid: {error}"
                            ))
                        })?);
                    }
                    Some("sha256") => {
                        validate_sha256(&value)?;
                        sha256 = Some(value);
                    }
                    _ => {}
                }
            }
            Ok(Event::End(event)) if matches!(event.local_name().as_ref(), b"size" | b"hash") => {
                current_field = None;
            }
            Ok(Event::End(event)) if event.local_name().as_ref() == b"file" => {
                in_repomd_file = false;
                current_field = None;
            }
            Ok(Event::Eof) => break,
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
    Ok(MetalinkRepomdIdentity {
        sha256: sha256.ok_or_else(|| {
            Error::ParseError("RPM metalink repomd.xml has no SHA-256".to_string())
        })?,
        size: size
            .ok_or_else(|| Error::ParseError("RPM metalink repomd.xml has no size".to_string()))?,
    })
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

    fn metalink(data: &[u8]) -> String {
        format!(
            r#"<metalink><files><file name="repomd.xml"><size>{}</size><verification><hash type="sha256">{}</hash></verification></file></files></metalink>"#,
            data.len(),
            crate::hash::sha256(data)
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
            "<metalink><files>{0}{0}</files></metalink>",
            r#"<file name="repomd.xml"><size>1</size><hash type="sha256">aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa</hash></file>"#
        );
        assert!(parse_metalink_repomd_identity(xml.as_bytes()).is_err());
    }
}
