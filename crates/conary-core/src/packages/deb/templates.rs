// conary-core/src/packages/deb/templates.rs

//! Exact parser and package-level native authority for Debian debconf templates.

use crate::error::{Error, Result};
use crate::packages::native_abi::{
    DEBCONF_TEMPLATES_ENTRY_ID, DEBCONF_TEMPLATES_NATIVE_SLOT, DebconfTemplateField,
    DebconfTemplateRecord, DebconfTemplatesMetadata, NativeInvocationContract, NativeLifecyclePath,
    NativeRootExpectation, NativeScriptletBody, NativeScriptletBodyEncoding, NativeScriptletEntry,
    NativeScriptletFormat, NativeScriptletKind, NativeScriptletMetadata, NativeScriptletSupport,
    NativeStdinContract, NativeTransactionOrder, NativeTransactionPosition,
};
use std::collections::BTreeSet;

/// Maximum byte length accepted for a debconf templates file.
///
/// The runtime `X_LOADTEMPLATEFILE` loader and package control-archive parser
/// share this bound so a file cannot become valid merely by changing how it
/// entered the package lifecycle.
pub const MAX_DEBCONF_TEMPLATES_SIZE: u64 = super::MAX_DEB_CONTROL_MEMBER_SIZE;

pub(crate) fn native_abi_entry(body: Vec<u8>) -> Result<Option<NativeScriptletEntry>> {
    let metadata = parse(&body)?;
    if metadata.records.is_empty() {
        return Ok(None);
    }
    Ok(Some(NativeScriptletEntry {
        id: DEBCONF_TEMPLATES_ENTRY_ID.to_string(),
        format: NativeScriptletFormat::Deb,
        kind: NativeScriptletKind::ControlArtifact,
        native_slot: DEBCONF_TEMPLATES_NATIVE_SLOT.to_string(),
        primary_lifecycle: NativeLifecyclePath::PackageControl,
        lifecycle_paths: vec![NativeLifecyclePath::PackageControl],
        interpreter: None,
        interpreter_args: Vec::new(),
        body: NativeScriptletBody::from_bytes(body),
        invocation: NativeInvocationContract::none(),
        order: NativeTransactionOrder::new(NativeTransactionPosition::ControlArtifact),
        support: NativeScriptletSupport::Parsed,
        metadata: NativeScriptletMetadata::DebconfTemplates(metadata),
    }))
}

/// Parse an exact Debian debconf templates file into typed records.
pub fn parse(body: &[u8]) -> Result<DebconfTemplatesMetadata> {
    if body.contains(&0) {
        return invalid("contains a NUL byte");
    }
    let text = std::str::from_utf8(body)
        .map_err(|error| invalid_error(format!("is not valid UTF-8: {error}")))?;
    let lines = source_lines(text)?;
    let stanzas = stanza_lines(&lines);
    let mut records = Vec::with_capacity(stanzas.len());
    let mut seen_templates = BTreeSet::new();
    for (stanza_index, stanza) in stanzas.into_iter().enumerate() {
        let record = parse_stanza(stanza_index + 1, &stanza)?;
        if !seen_templates.insert(record.template.clone()) {
            return invalid(format!(
                "stanza {} duplicates template authority '{}'",
                stanza_index + 1,
                record.template
            ));
        }
        records.push(record);
    }

    Ok(DebconfTemplatesMetadata {
        raw_sha256: crate::hash::sha256_prefixed(body),
        records,
    })
}

pub(crate) fn validate_native_entry(entry: &NativeScriptletEntry) -> Result<()> {
    let NativeScriptletMetadata::DebconfTemplates(metadata) = &entry.metadata else {
        return invalid("native entry does not carry debconf templates metadata");
    };
    if metadata.records.is_empty() {
        return invalid("native entry has no template records");
    }
    if entry.id != DEBCONF_TEMPLATES_ENTRY_ID
        || entry.format != NativeScriptletFormat::Deb
        || entry.kind != NativeScriptletKind::ControlArtifact
        || entry.native_slot != DEBCONF_TEMPLATES_NATIVE_SLOT
        || entry.primary_lifecycle != NativeLifecyclePath::PackageControl
        || entry.lifecycle_paths != [NativeLifecyclePath::PackageControl]
    {
        return invalid("native entry does not use the exact package-level templates contract");
    }
    if entry.interpreter.is_some() || !entry.interpreter_args.is_empty() {
        return invalid("control artifact declares an executable interpreter");
    }
    if !entry.invocation.args.is_empty()
        || !entry.invocation.environment.is_empty()
        || entry.invocation.stdin != NativeStdinContract::None
        || entry.invocation.root != NativeRootExpectation::PackageManagerDefault
    {
        return invalid("control artifact declares an executable invocation");
    }
    if entry.order.position != NativeTransactionPosition::ControlArtifact
        || entry.order.relative_to.is_some()
    {
        return invalid("control artifact declares transaction execution ordering");
    }
    if entry.body.encoding != NativeScriptletBodyEncoding::Utf8 {
        return invalid("body is not UTF-8");
    }
    let body_text = std::str::from_utf8(&entry.body.bytes)
        .map_err(|error| invalid_error(format!("is not valid UTF-8: {error}")))?;
    if entry.body.text.as_deref() != Some(body_text) {
        return invalid("native body text does not match its raw bytes");
    }
    let reparsed = parse(&entry.body.bytes)?;
    if entry.body.sha256 != reparsed.raw_sha256 {
        return invalid("native body SHA-256 does not match its raw bytes");
    }
    if metadata != &reparsed {
        return invalid("typed metadata does not match the exact raw templates member");
    }
    Ok(())
}

fn source_lines(text: &str) -> Result<Vec<(usize, &str)>> {
    text.split('\n')
        .enumerate()
        .map(|(index, line)| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.contains('\r') {
                return invalid(format!(
                    "line {} contains a lone carriage return",
                    index + 1
                ));
            }
            Ok((index + 1, line))
        })
        .collect()
}

fn stanza_lines<'a>(lines: &'a [(usize, &'a str)]) -> Vec<Vec<(usize, &'a str)>> {
    let mut stanzas = Vec::new();
    let mut current = Vec::new();
    for &(line_number, line) in lines {
        if line.is_empty() {
            if !current.is_empty() {
                stanzas.push(std::mem::take(&mut current));
            }
        } else {
            current.push((line_number, line));
        }
    }
    if !current.is_empty() {
        stanzas.push(current);
    }
    stanzas
}

fn parse_stanza(stanza_number: usize, lines: &[(usize, &str)]) -> Result<DebconfTemplateRecord> {
    let mut fields: Vec<DebconfTemplateField> = Vec::new();
    let mut seen_fields = BTreeSet::new();

    for &(line_number, line) in lines {
        if line.starts_with([' ', '\t']) {
            let Some(field) = fields.last_mut() else {
                return invalid(format!(
                    "stanza {stanza_number} line {line_number} has a continuation before any field"
                ));
            };
            field.continuation_lines.push(line.to_string());
            continue;
        }

        let Some((name, raw_value)) = line.split_once(':') else {
            return invalid(format!(
                "stanza {stanza_number} line {line_number} is not an RFC822 field or continuation"
            ));
        };
        if name.is_empty()
            || !name.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'@' | b'.')
            })
        {
            return invalid(format!(
                "stanza {stanza_number} line {line_number} has invalid field name '{name}'"
            ));
        }

        let folded_name = name.to_ascii_lowercase();
        if !seen_fields.insert(folded_name) {
            return invalid(format!(
                "stanza {stanza_number} line {line_number} duplicates field '{name}'"
            ));
        }
        let value = raw_value
            .strip_prefix([' ', '\t'])
            .unwrap_or(raw_value)
            .to_string();
        fields.push(DebconfTemplateField {
            name: name.to_string(),
            value,
            continuation_lines: Vec::new(),
        });
    }

    let template = required_field_value(stanza_number, &fields, "template")?;
    if !valid_template_name(&template) {
        return invalid(format!(
            "stanza {stanza_number} has invalid Template value '{template}'"
        ));
    }
    let template_type = required_field_value(stanza_number, &fields, "type")?;

    Ok(DebconfTemplateRecord {
        template,
        template_type,
        fields,
    })
}

fn required_field_value(
    stanza_number: usize,
    fields: &[DebconfTemplateField],
    required_name: &str,
) -> Result<String> {
    let field = fields
        .iter()
        .find(|field| field.name.eq_ignore_ascii_case(required_name))
        .ok_or_else(|| {
            invalid_error(format!(
                "stanza {stanza_number} does not contain a '{required_name}:' field"
            ))
        })?;
    let value = field
        .value
        .trim_end_matches(|character: char| character.is_ascii_whitespace());
    if value.is_empty() {
        return invalid(format!(
            "stanza {stanza_number} has an empty '{required_name}:' field"
        ));
    }
    Ok(value.to_string())
}

fn valid_template_name(name: &str) -> bool {
    name.split('/').all(|component| {
        !component.is_empty()
            && component.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.' | b'_')
            })
    })
}

fn invalid<T>(message: impl Into<String>) -> Result<T> {
    Err(invalid_error(message))
}

fn invalid_error(message: impl Into<String>) -> Error {
    Error::ParseError(format!("Invalid DEBIAN/templates: {}", message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packages::deb::DebPackage;
    use std::io::Cursor;

    const COMPLETE: &[u8] = b"Template: demo/server\nType: select\nChoices: alpha, beta\nChoices-pt_BR.UTF-8: alfa, beta\nDescription: Server choice  \n Extended description.\n .\n  * literal list item\nX-Vendor.Field: retained\n\nTemplate: shared/secret\nType: vendor-password\nDescription-fr: Secret partage\n";

    #[test]
    fn parses_and_preserves_fields_localizations_and_continuations() {
        let parsed = parse(COMPLETE).expect("parse templates");

        assert_eq!(parsed.raw_sha256, crate::hash::sha256_prefixed(COMPLETE));
        assert_eq!(parsed.records.len(), 2);
        assert_eq!(parsed.records[0].template, "demo/server");
        assert_eq!(parsed.records[0].template_type, "select");
        assert_eq!(
            parsed.records[0]
                .fields
                .iter()
                .map(|field| field.name.as_str())
                .collect::<Vec<_>>(),
            vec![
                "Template",
                "Type",
                "Choices",
                "Choices-pt_BR.UTF-8",
                "Description",
                "X-Vendor.Field",
            ]
        );
        let description = parsed.records[0]
            .fields
            .iter()
            .find(|field| field.name == "Description")
            .expect("description");
        assert_eq!(description.value, "Server choice  ");
        assert_eq!(
            description.continuation_lines,
            vec![
                " Extended description.".to_string(),
                " .".to_string(),
                "  * literal list item".to_string(),
            ]
        );
        assert_eq!(parsed.records[1].template_type, "vendor-password");
        assert!(
            parsed.records[1]
                .fields
                .iter()
                .any(|field| field.name == "Description-fr")
        );
    }

    #[test]
    fn accepts_crlf_without_changing_raw_authority() {
        let body = b"Template: demo/name\r\nType: string\r\nDescription: Demo\r\n";
        let parsed = parse(body).expect("parse CRLF templates");

        assert_eq!(parsed.raw_sha256, crate::hash::sha256_prefixed(body));
        assert_eq!(parsed.records[0].template, "demo/name");
    }

    #[test]
    fn rejects_duplicate_or_ambiguous_authority() {
        for body in [
            b"Template: demo/name\nType: string\nTYPE: boolean\n".as_slice(),
            b"Template: demo/name\nType: string\n\nTemplate: demo/name\nType: boolean\n".as_slice(),
        ] {
            assert!(parse(body).is_err(), "{body:?}");
        }
    }

    #[test]
    fn rejects_missing_required_fields_and_malformed_records() {
        for body in [
            b"Type: string\n".as_slice(),
            b"Template: demo/name\n".as_slice(),
            b"Template: demo//name\nType: string\n".as_slice(),
            b" Continuation first\nTemplate: demo/name\nType: string\n".as_slice(),
            b"Template demo/name\nType: string\n".as_slice(),
            b"Template: demo/name\nType: string\rbroken\n".as_slice(),
            b"Template: demo/name\nType: string\n\0".as_slice(),
            b"Template: demo/name\nType: \xff\n".as_slice(),
        ] {
            assert!(parse(body).is_err(), "{body:?}");
        }
    }

    #[test]
    fn empty_or_blank_member_is_a_valid_noop() {
        for body in [b"".as_slice(), b"\n\n".as_slice(), b"\r\n\r\n".as_slice()] {
            let parsed = parse(body).expect("empty member follows upstream no-op behavior");
            assert!(parsed.records.is_empty());
            assert_eq!(parsed.raw_sha256, crate::hash::sha256_prefixed(body));
            assert!(
                native_abi_entry(body.to_vec())
                    .expect("empty member")
                    .is_none()
            );
        }
    }

    #[test]
    fn native_entry_is_package_control_not_config_owned() {
        let entry = native_abi_entry(COMPLETE.to_vec())
            .expect("templates entry")
            .expect("non-empty templates authority");

        assert_eq!(entry.id, DEBCONF_TEMPLATES_ENTRY_ID);
        assert_eq!(entry.primary_lifecycle, NativeLifecyclePath::PackageControl);
        assert_eq!(entry.lifecycle_paths, [NativeLifecyclePath::PackageControl]);
        assert_eq!(entry.kind, NativeScriptletKind::ControlArtifact);
        assert!(entry.interpreter.is_none());
        validate_native_entry(&entry).expect("validate exact entry");
    }

    #[test]
    fn native_entry_validation_rejects_metadata_drift() {
        let mut entry = native_abi_entry(COMPLETE.to_vec())
            .expect("templates entry")
            .expect("non-empty templates authority");
        let NativeScriptletMetadata::DebconfTemplates(metadata) = &mut entry.metadata else {
            unreachable!("templates metadata")
        };
        metadata.records[0].template = "drifted/name".to_string();

        assert!(validate_native_entry(&entry).is_err());
    }

    #[test]
    fn control_archive_rejects_duplicate_templates_members() {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, body) in [
            (
                "control",
                b"Package: demo\nVersion: 1\nArchitecture: all\n".as_slice(),
            ),
            ("templates", COMPLETE),
            ("./templates", COMPLETE),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, path, Cursor::new(body))
                .expect("append control member");
        }
        let archive = builder.into_inner().expect("finish control archive");

        let error = match DebPackage::parse_control_tar_all(&archive) {
            Ok(_) => panic!("duplicate templates member must fail"),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains("duplicate control member"),
            "{error}"
        );
    }

    #[test]
    fn control_archive_requires_a_bounded_regular_templates_member() {
        let mut builder = tar::Builder::new(Vec::new());
        let control = b"Package: demo\nVersion: 1\nArchitecture: all\n";
        let mut control_header = tar::Header::new_gnu();
        control_header.set_size(control.len() as u64);
        control_header.set_mode(0o644);
        control_header.set_cksum();
        builder
            .append_data(&mut control_header, "control", Cursor::new(control))
            .expect("append control");
        let mut directory_header = tar::Header::new_gnu();
        directory_header.set_entry_type(tar::EntryType::Directory);
        directory_header.set_size(0);
        directory_header.set_mode(0o755);
        directory_header.set_cksum();
        builder
            .append_data(
                &mut directory_header,
                "templates",
                Cursor::new(Vec::<u8>::new()),
            )
            .expect("append directory");
        let directory_archive = builder.into_inner().expect("finish directory archive");
        let error = match DebPackage::parse_control_tar_all(&directory_archive) {
            Ok(_) => panic!("directory templates member must fail"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("not a regular file"), "{error}");

        let mut oversized_header = tar::Header::new_gnu();
        oversized_header
            .set_path("templates")
            .expect("set templates path");
        oversized_header.set_entry_type(tar::EntryType::Regular);
        oversized_header.set_size(crate::packages::deb::MAX_DEB_CONTROL_MEMBER_SIZE + 1);
        oversized_header.set_mode(0o644);
        oversized_header.set_cksum();
        let mut oversized_archive = oversized_header.as_bytes().to_vec();
        oversized_archive.extend_from_slice(&[0; 1024]);
        let error = match DebPackage::parse_control_tar_all(&oversized_archive) {
            Ok(_) => panic!("oversized templates member must fail"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("too large"), "{error}");
    }
}
