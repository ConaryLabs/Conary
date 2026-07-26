// conary-core/src/packages/rpm/scriptlets/runtime_context.rs
//! Typed RPM query-format and macro context captured from package headers.
//! Signature translation mirrors RPM `lib/package.cc` at
//! `a8f0192aee1c08bd1454ed2ac6ebaf506004b55c`.

use super::super::*;
use num_traits::FromPrimitive as _;

#[derive(Debug, Clone)]
pub(super) struct ParsedRpmRuntimeContext {
    pub(super) macros: RpmMacroContextMetadata,
    pub(super) header: RpmHeaderContextMetadata,
    pub(super) package_rpm_version: Option<String>,
}

impl RpmPackage {
    pub(super) fn rpm_runtime_context(
        metadata: &rpm::PackageMetadata,
    ) -> Result<ParsedRpmRuntimeContext> {
        let required = |label: &str, value: std::result::Result<&str, rpm::Error>| {
            value.map(str::to_string).map_err(|error| {
                Error::ParseError(format!(
                    "Failed to read RPM {label} for runtime context: {error}"
                ))
            })
        };
        let optional = |label: &str, value: std::result::Result<&str, rpm::Error>| match value {
            Ok(value) => Ok(Some(value.to_string())),
            Err(rpm::Error::TagNotFound(_)) => Ok(None),
            Err(error) => Err(Error::ParseError(format!(
                "Failed to read optional RPM {label} for runtime context: {error}"
            ))),
        };
        let name = required("name", metadata.get_name())?;
        let version = required("version", metadata.get_version())?;
        let release = required("release", metadata.get_release())?;
        let arch = required("architecture", metadata.get_arch())?;
        let epoch = if metadata
            .header
            .entry_is_present(rpm::IndexTag::RPMTAG_EPOCH)
        {
            Some(metadata.get_epoch().map_err(|error| {
                Error::ParseError(format!(
                    "Failed to read RPM epoch for runtime context: {error}"
                ))
            })?)
        } else {
            None
        };

        let mut facts = metadata
            .header
            .get_all_entries()
            .map_err(|error| {
                Error::ParseError(format!(
                    "Failed to preserve typed RPM header context: {error}"
                ))
            })?
            .into_iter()
            .map(|(tag, value)| {
                Ok(RpmHeaderFactMetadata {
                    tag,
                    name: rpm_header_tag_name(tag)?,
                    value: rpm_header_value(value),
                    source: RpmHeaderFactSource::Header,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        merge_signature_header_facts(metadata, &mut facts)?;

        let installed_filenames = metadata
            .get_file_entries()
            .map_err(|error| {
                Error::ParseError(format!(
                    "Failed to derive RPM installed filenames for query-format context: {error}"
                ))
            })?
            .into_iter()
            .map(|entry| entry.path().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        for (tag, name, source) in [
            (
                rpm::IndexTag::RPMTAG_FILENAMES as u32,
                "FILENAMES",
                RpmHeaderFactSource::Header,
            ),
            (
                rpm::IndexTag::RPMTAG_ORIGFILENAMES as u32,
                "ORIGFILENAMES",
                RpmHeaderFactSource::Header,
            ),
            (
                rpm::IndexTag::RPMTAG_INSTFILENAMES as u32,
                "INSTFILENAMES",
                RpmHeaderFactSource::TransactionDerived,
            ),
        ] {
            upsert_rpm_header_fact(
                &mut facts,
                RpmHeaderFactMetadata {
                    tag,
                    name: Some(name.to_string()),
                    value: RpmHeaderValueMetadata::StringArray(installed_filenames.clone()),
                    source,
                },
            )?;
        }
        let epoch_prefix = epoch.map_or_else(String::new, |epoch| format!("{epoch}:"));
        for (tag, name, value) in [
            (
                rpm::IndexTag::RPMTAG_EVR as u32,
                "EVR",
                format!("{epoch_prefix}{version}-{release}"),
            ),
            (
                rpm::IndexTag::RPMTAG_NVR as u32,
                "NVR",
                format!("{name}-{version}-{release}"),
            ),
            (
                rpm::IndexTag::RPMTAG_NEVR as u32,
                "NEVR",
                format!("{name}-{epoch_prefix}{version}-{release}"),
            ),
            (
                rpm::IndexTag::RPMTAG_NVRA as u32,
                "NVRA",
                format!("{name}-{version}-{release}.{arch}"),
            ),
            (
                rpm::IndexTag::RPMTAG_NEVRA as u32,
                "NEVRA",
                format!("{name}-{epoch_prefix}{version}-{release}.{arch}"),
            ),
            (
                rpm::IndexTag::RPMTAG_ARCHSUFFIX as u32,
                "ARCHSUFFIX",
                format!(".{arch}"),
            ),
        ] {
            upsert_rpm_header_fact(
                &mut facts,
                RpmHeaderFactMetadata {
                    tag,
                    name: Some(name.to_string()),
                    value: RpmHeaderValueMetadata::String(value),
                    source: RpmHeaderFactSource::Header,
                },
            )?;
        }
        upsert_rpm_header_fact(
            &mut facts,
            RpmHeaderFactMetadata {
                tag: rpm::IndexTag::RPMTAG_EPOCHNUM as u32,
                name: Some("EPOCHNUM".to_string()),
                value: RpmHeaderValueMetadata::Integer(vec![u64::from(epoch.unwrap_or(0))]),
                source: RpmHeaderFactSource::Header,
            },
        )?;
        facts.sort_by_key(|fact| fact.tag);

        let mut definitions = Vec::new();
        for (name, value) in [
            ("name", Some(name)),
            ("version", Some(version)),
            ("release", Some(release)),
            ("epoch", epoch.map(|value| value.to_string())),
            ("arch", Some(arch)),
            (
                "os",
                optional(
                    "OS",
                    metadata
                        .header
                        .get_entry_data_as_string(rpm::IndexTag::RPMTAG_OS),
                )?,
            ),
        ] {
            if let Some(body) = value {
                definitions.push(RpmMacroDefinitionMetadata {
                    name: name.to_string(),
                    body,
                    source: RpmMacroDefinitionSource::PackageHeader,
                });
            }
        }

        let package_rpm_version = optional(
            "RPM version",
            metadata
                .header
                .get_entry_data_as_string(rpm::IndexTag::RPMTAG_RPMVERSION),
        )?;

        Ok(ParsedRpmRuntimeContext {
            macros: RpmMacroContextMetadata { definitions },
            header: RpmHeaderContextMetadata { facts },
            package_rpm_version,
        })
    }
}

fn merge_signature_header_facts(
    metadata: &rpm::PackageMetadata,
    facts: &mut Vec<RpmHeaderFactMetadata>,
) -> Result<()> {
    let signature_facts = metadata.signature.get_all_entries().map_err(|error| {
        Error::ParseError(format!(
            "Failed to preserve typed RPM signature header context: {error}"
        ))
    })?;
    for (signature_tag, value) in signature_facts {
        let Some(tag) = signature_runtime_tag(signature_tag) else {
            continue;
        };
        if facts.iter().any(|fact| fact.tag == tag as u32) {
            return Err(Error::ParseError(format!(
                "RPM package repeats signature projection {} in its main and signature headers",
                rpm_header_tag_name(tag as u32)?.unwrap_or_else(|| (tag as u32).to_string())
            )));
        }
        facts.push(RpmHeaderFactMetadata {
            tag: tag as u32,
            name: rpm_header_tag_name(tag as u32)?,
            value: rpm_header_value(value),
            source: RpmHeaderFactSource::Header,
        });
    }
    Ok(())
}

fn signature_runtime_tag(tag: u32) -> Option<rpm::IndexTag> {
    use rpm::{IndexSignatureTag as Sig, IndexTag as Tag};

    match tag {
        value if value == Sig::RPMSIGTAG_SIZE as u32 => Some(Tag::RPMTAG_SIGSIZE),
        value if value == Sig::RPMSIGTAG_PGP as u32 => Some(Tag::RPMTAG_SIGPGP),
        value if value == Sig::RPMSIGTAG_MD5 as u32 => Some(Tag::RPMTAG_SIGMD5),
        value if value == Sig::RPMSIGTAG_GPG as u32 => Some(Tag::RPMTAG_SIGGPG),
        value if value == Sig::RPMSIGTAG_PAYLOADSIZE as u32 => Some(Tag::RPMTAG_ARCHIVESIZE),
        value if value == Sig::RPMSIGTAG_FILESIGNATURES as u32 => Some(Tag::RPMTAG_FILESIGNATURES),
        value if value == Sig::RPMSIGTAG_FILESIGNATURE_LENGTH as u32 => {
            Some(Tag::RPMTAG_FILESIGNATURELENGTH)
        }
        value if value == Sig::RPMSIGTAG_VERITYSIGNATURES as u32 => {
            Some(Tag::RPMTAG_VERITYSIGNATURES)
        }
        value if value == Sig::RPMSIGTAG_VERITYSIGNATUREALGO as u32 => {
            Some(Tag::RPMTAG_VERITYSIGNATUREALGO)
        }
        value if value == Sig::RPMSIGTAG_SHA1 as u32 => Some(Tag::RPMTAG_SHA1HEADER),
        value if value == Sig::RPMSIGTAG_SHA256 as u32 => Some(Tag::RPMTAG_SHA256HEADER),
        value if value == Sig::RPMSIGTAG_SHA3_256 as u32 => Some(Tag::RPMTAG_SHA3_256_HEADER),
        value if value == Sig::RPMSIGTAG_DSA as u32 => Some(Tag::RPMTAG_DSAHEADER),
        value if value == Sig::RPMSIGTAG_RSA as u32 => Some(Tag::RPMTAG_RSAHEADER),
        value if value == Sig::RPMSIGTAG_LONGSIZE as u32 => Some(Tag::RPMTAG_LONGSIGSIZE),
        value if value == Sig::RPMSIGTAG_LONGARCHIVESIZE as u32 => {
            Some(Tag::RPMTAG_LONGARCHIVESIZE)
        }
        value if value == Sig::RPMSIGTAG_OPENPGP as u32 => Some(Tag::RPMTAG_OPENPGP),
        _ => None,
    }
}

fn rpm_header_tag_name(tag: u32) -> Result<Option<String>> {
    let Some(known) = rpm::IndexTag::from_u32(tag) else {
        return Ok(None);
    };
    let typed_name = format!("{known:?}");
    for prefix in ["RPMTAG_", "RPMSIGTAG_"] {
        if let Some(name) = typed_name.strip_prefix(prefix) {
            return Ok(Some(name.to_string()));
        }
    }
    Err(Error::ParseError(format!(
        "RPM header tag {tag} has typed identifier '{typed_name}' outside the supported RPM tag grammar"
    )))
}

fn rpm_header_value(value: rpm::IndexData) -> RpmHeaderValueMetadata {
    match value {
        rpm::IndexData::Null => RpmHeaderValueMetadata::Null,
        rpm::IndexData::Char(value) => {
            RpmHeaderValueMetadata::Integer(value.into_iter().map(u64::from).collect())
        }
        rpm::IndexData::Bin(value) => RpmHeaderValueMetadata::Binary(value),
        rpm::IndexData::Int8(value) => {
            RpmHeaderValueMetadata::Integer(value.into_iter().map(u64::from).collect())
        }
        rpm::IndexData::Int16(value) => {
            RpmHeaderValueMetadata::Integer(value.into_iter().map(u64::from).collect())
        }
        rpm::IndexData::Int32(value) => {
            RpmHeaderValueMetadata::Integer(value.into_iter().map(u64::from).collect())
        }
        rpm::IndexData::Int64(value) => RpmHeaderValueMetadata::Integer(value),
        rpm::IndexData::StringTag(value) => RpmHeaderValueMetadata::String(value),
        rpm::IndexData::StringArray(value) => RpmHeaderValueMetadata::StringArray(value),
        rpm::IndexData::I18NString(value) => RpmHeaderValueMetadata::I18nString(value),
    }
}

fn upsert_rpm_header_fact(
    facts: &mut Vec<RpmHeaderFactMetadata>,
    fact: RpmHeaderFactMetadata,
) -> Result<()> {
    if let Some(existing) = facts.iter_mut().find(|existing| existing.tag == fact.tag) {
        *existing = fact;
        return Ok(());
    }
    if fact.name.as_ref().is_some_and(|name| {
        facts.iter().any(|existing| {
            existing
                .name
                .as_ref()
                .is_some_and(|existing| existing.eq_ignore_ascii_case(name))
        })
    }) {
        return Err(Error::ParseError(format!(
            "RPM header query-format context repeats derived tag name '{}'",
            fact.name.as_deref().unwrap_or_default()
        )));
    }
    facts.push(fact);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{rpm_header_value, signature_runtime_tag};
    use crate::packages::native_abi::RpmHeaderValueMetadata;
    use rpm::{IndexSignatureTag as Sig, IndexTag as Tag};

    #[test]
    fn rpm_char_values_preserve_rpms_numeric_type_class() {
        assert_eq!(
            rpm_header_value(rpm::IndexData::Char(vec![1, 2, u8::MAX])),
            RpmHeaderValueMetadata::Integer(vec![1, 2, u64::from(u8::MAX)])
        );
        assert_eq!(
            rpm_header_value(rpm::IndexData::Bin(vec![1, 2, u8::MAX])),
            RpmHeaderValueMetadata::Binary(vec![1, 2, u8::MAX])
        );
    }

    #[test]
    fn signature_header_projection_matches_rpm_translation_table() {
        for (signature, runtime) in [
            (Sig::RPMSIGTAG_SIZE, Tag::RPMTAG_SIGSIZE),
            (Sig::RPMSIGTAG_PGP, Tag::RPMTAG_SIGPGP),
            (Sig::RPMSIGTAG_MD5, Tag::RPMTAG_SIGMD5),
            (Sig::RPMSIGTAG_GPG, Tag::RPMTAG_SIGGPG),
            (Sig::RPMSIGTAG_PAYLOADSIZE, Tag::RPMTAG_ARCHIVESIZE),
            (Sig::RPMSIGTAG_FILESIGNATURES, Tag::RPMTAG_FILESIGNATURES),
            (
                Sig::RPMSIGTAG_FILESIGNATURE_LENGTH,
                Tag::RPMTAG_FILESIGNATURELENGTH,
            ),
            (
                Sig::RPMSIGTAG_VERITYSIGNATURES,
                Tag::RPMTAG_VERITYSIGNATURES,
            ),
            (
                Sig::RPMSIGTAG_VERITYSIGNATUREALGO,
                Tag::RPMTAG_VERITYSIGNATUREALGO,
            ),
            (Sig::RPMSIGTAG_SHA1, Tag::RPMTAG_SHA1HEADER),
            (Sig::RPMSIGTAG_SHA256, Tag::RPMTAG_SHA256HEADER),
            (Sig::RPMSIGTAG_SHA3_256, Tag::RPMTAG_SHA3_256_HEADER),
            (Sig::RPMSIGTAG_DSA, Tag::RPMTAG_DSAHEADER),
            (Sig::RPMSIGTAG_RSA, Tag::RPMTAG_RSAHEADER),
            (Sig::RPMSIGTAG_LONGSIZE, Tag::RPMTAG_LONGSIGSIZE),
            (Sig::RPMSIGTAG_LONGARCHIVESIZE, Tag::RPMTAG_LONGARCHIVESIZE),
            (Sig::RPMSIGTAG_OPENPGP, Tag::RPMTAG_OPENPGP),
        ] {
            assert_eq!(signature_runtime_tag(signature as u32), Some(runtime));
        }
        assert_eq!(signature_runtime_tag(Sig::RPMSIGTAG_RESERVED as u32), None);
    }
}
