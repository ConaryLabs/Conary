// conary-core/src/packages/eopkg/xml.rs

//! Bounded XML admission for eopkg `metadata.xml` and `files.xml`.

use crate::error::{Error, Result};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use serde::Serialize;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RelationRecord {
    pub name: String,
    pub version: Option<String>,
    pub version_from: Option<String>,
    pub version_to: Option<String>,
    pub release: Option<u64>,
    pub release_from: Option<u64>,
    pub release_to: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Metadata {
    pub name: String,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub version: String,
    pub release: u64,
    pub distribution: String,
    pub distribution_release: String,
    pub architecture: String,
    pub package_format: String,
    pub package_size: Option<u64>,
    pub package_hash: Option<String>,
    pub package_uri: Option<String>,
    pub dependencies: Vec<Vec<RelationRecord>>,
    pub conflicts: Vec<RelationRecord>,
    pub replaces: Vec<RelationRecord>,
    pub pkg_config: Vec<String>,
    pub pkg_config_32: Vec<String>,
    pub comar: Vec<String>,
    pub deltas: Vec<DeltaRecord>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct DeltaRecord {
    pub release_from: Option<u64>,
    pub build_from: Option<String>,
    pub package_uri: String,
    pub package_size: u64,
    pub package_hash: String,
}

#[derive(Debug, Clone, Default)]
pub(super) struct FileRecord {
    pub path: String,
    pub file_type: String,
    pub size: Option<u64>,
    pub uid: Option<u64>,
    pub gid: Option<u64>,
    pub mode: Option<u32>,
    pub sha1: Option<String>,
    pub permanent: bool,
    pub xattrs: Vec<(String, Vec<u8>)>,
}

fn attribute<R>(
    element: &BytesStart<'_>,
    reader: &Reader<R>,
    name: &[u8],
) -> Result<Option<String>> {
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|error| Error::ParseError(error.to_string()))?;
        if attribute.key.as_ref() == name {
            return Ok(Some(
                attribute
                    .decoded_and_normalized_value(
                        quick_xml::XmlVersion::Implicit1_0,
                        reader.decoder(),
                    )
                    .map_err(|error| Error::ParseError(error.to_string()))?
                    .into_owned(),
            ));
        }
    }
    Ok(None)
}

fn parse_optional_u64(value: Option<String>, field: &str) -> Result<Option<u64>> {
    value
        .map(|value| {
            value.parse::<u64>().map_err(|error| {
                Error::ParseError(format!("eopkg {field} is not an unsigned integer: {error}"))
            })
        })
        .transpose()
}

fn relation<R>(element: &BytesStart<'_>, reader: &Reader<R>) -> Result<RelationRecord> {
    Ok(RelationRecord {
        version: attribute(element, reader, b"version")?,
        version_from: attribute(element, reader, b"versionFrom")?,
        version_to: attribute(element, reader, b"versionTo")?,
        release: parse_optional_u64(attribute(element, reader, b"release")?, "release")?,
        release_from: parse_optional_u64(
            attribute(element, reader, b"releaseFrom")?,
            "releaseFrom",
        )?,
        release_to: parse_optional_u64(attribute(element, reader, b"releaseTo")?, "releaseTo")?,
        ..RelationRecord::default()
    })
}

fn path_ends(path: &[String], suffix: &[&str]) -> bool {
    path.len() >= suffix.len()
        && path[path.len() - suffix.len()..]
            .iter()
            .map(String::as_str)
            .eq(suffix.iter().copied())
}

fn reference_value(reference: &quick_xml::events::BytesRef<'_>, document: &str) -> Result<String> {
    match reference.resolve_char_ref().map_err(|error| {
        Error::ParseError(format!(
            "invalid eopkg {document} character reference: {error}"
        ))
    })? {
        Some(character) => Ok(character.to_string()),
        None => {
            let entity = reference
                .decode()
                .map_err(|error| Error::ParseError(error.to_string()))?;
            quick_xml::escape::resolve_xml_entity(&entity)
                .map(str::to_string)
                .ok_or_else(|| {
                    Error::ParseError(format!(
                        "eopkg {document} uses undeclared entity '&{entity};'"
                    ))
                })
        }
    }
}

pub(crate) fn parse_metadata(bytes: &[u8]) -> Result<Metadata> {
    let mut reader = Reader::from_reader(bytes);
    // XML references split one logical scalar into multiple reader events.
    // Preserve whitespace between those events and trim only after the entire
    // element has been accumulated.
    reader.config_mut().trim_text(false);
    let mut metadata = Metadata::default();
    let mut path = Vec::new();
    let mut current_relation: Option<(RelationRecord, RelationDestination)> = None;
    let mut history_depth = 0_u32;
    let mut saw_current_update = false;
    let mut any_dependency_group: Option<Vec<RelationRecord>> = None;
    let mut current_delta: Option<DeltaRecord> = None;
    let mut element_value = String::new();
    loop {
        match reader
            .read_event()
            .map_err(|error| Error::ParseError(error.to_string()))?
        {
            Event::Start(start) => {
                let tag = String::from_utf8_lossy(start.local_name().as_ref()).into_owned();
                path.push(tag.clone());
                element_value.clear();
                if tag == "History" {
                    history_depth += 1;
                }
                if path_ends(&path, &["RuntimeDependencies", "AnyDependency"]) {
                    any_dependency_group = Some(Vec::new());
                }
                let destination = if path_ends(&path, &["RuntimeDependencies", "Dependency"]) {
                    Some(RelationDestination::Dependency)
                } else if path_ends(
                    &path,
                    &["RuntimeDependencies", "AnyDependency", "Dependency"],
                ) {
                    Some(RelationDestination::AnyDependency)
                } else if path_ends(&path, &["Conflicts", "Package"]) {
                    Some(RelationDestination::Conflict)
                } else if path_ends(&path, &["Replaces", "Package"]) {
                    Some(RelationDestination::Replace)
                } else {
                    None
                };
                if let Some(destination) = destination {
                    current_relation = Some((relation(&start, &reader)?, destination));
                }
                if path_ends(&path, &["Package", "History", "Update"]) && !saw_current_update {
                    metadata.release = attribute(&start, &reader, b"release")?
                        .ok_or_else(|| {
                            Error::ParseError("eopkg current Update has no release".to_string())
                        })?
                        .parse::<u64>()
                        .map_err(|error| {
                            Error::ParseError(format!("invalid eopkg release: {error}"))
                        })?;
                    saw_current_update = true;
                }
                if path_ends(&path, &["Package", "DeltaPackages", "Delta"]) {
                    current_delta = Some(DeltaRecord {
                        release_from: parse_optional_u64(
                            attribute(&start, &reader, b"releaseFrom")?,
                            "delta releaseFrom",
                        )?,
                        build_from: attribute(&start, &reader, b"buildFrom")?,
                        ..DeltaRecord::default()
                    });
                }
            }
            Event::Text(text) => {
                let value = text
                    .xml_content(quick_xml::XmlVersion::Implicit1_0)
                    .map_err(|error| Error::ParseError(error.to_string()))?
                    .to_string();
                element_value.push_str(&value);
            }
            Event::CData(data) => {
                element_value.push_str(
                    &data
                        .xml_content(quick_xml::XmlVersion::Implicit1_0)
                        .map_err(|error| Error::ParseError(error.to_string()))?,
                );
            }
            Event::GeneralRef(reference) => {
                element_value.push_str(&reference_value(&reference, "metadata.xml")?);
            }
            Event::End(end) => {
                let tag = String::from_utf8_lossy(end.local_name().as_ref()).into_owned();
                apply_metadata_value(
                    &path,
                    element_value.trim(),
                    &mut metadata,
                    &mut current_relation,
                    &mut current_delta,
                )?;
                let closes_relation = match current_relation
                    .as_ref()
                    .map(|(_, destination)| *destination)
                {
                    Some(RelationDestination::Dependency | RelationDestination::AnyDependency) => {
                        tag == "Dependency"
                    }
                    Some(RelationDestination::Conflict | RelationDestination::Replace) => {
                        tag == "Package"
                    }
                    None => false,
                };
                if closes_relation && let Some((record, destination)) = current_relation.take() {
                    if record.name.is_empty() {
                        return Err(Error::ParseError(
                            "eopkg dependency name is empty".to_string(),
                        ));
                    }
                    match destination {
                        RelationDestination::Dependency => metadata.dependencies.push(vec![record]),
                        RelationDestination::AnyDependency => {
                            any_dependency_group
                                .as_mut()
                                .expect("AnyDependency start owns group")
                                .push(record);
                        }
                        RelationDestination::Conflict => metadata.conflicts.push(record),
                        RelationDestination::Replace => metadata.replaces.push(record),
                    }
                }
                if tag == "History" {
                    history_depth = history_depth.saturating_sub(1);
                }
                if tag == "AnyDependency" {
                    let group = any_dependency_group.take().ok_or_else(|| {
                        Error::ParseError("unmatched eopkg AnyDependency".to_string())
                    })?;
                    if group.is_empty() {
                        return Err(Error::ParseError(
                            "eopkg AnyDependency has no Dependency alternatives".to_string(),
                        ));
                    }
                    metadata.dependencies.push(group);
                }
                if tag == "Delta" {
                    let delta = current_delta
                        .take()
                        .ok_or_else(|| Error::ParseError("unmatched eopkg Delta".to_string()))?;
                    if delta.release_from.is_none()
                        || delta.package_uri.is_empty()
                        || delta.package_size == 0
                        || delta.package_hash.is_empty()
                    {
                        return Err(Error::ParseError(
                            "eopkg Delta lacks releaseFrom, PackageURI, PackageSize, or PackageHash"
                                .to_string(),
                        ));
                    }
                    metadata.deltas.push(delta);
                }
                path.pop();
                element_value.clear();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !saw_current_update || metadata.name.is_empty() || metadata.version.is_empty() {
        return Err(Error::ParseError(
            "eopkg metadata lacks exact current identity".to_string(),
        ));
    }
    if metadata.package_format != "1.2" {
        return Err(Error::ParseError(format!(
            "unsupported eopkg PackageFormat {:?}; current source ABI is 1.2",
            metadata.package_format
        )));
    }
    if history_depth != 0 {
        return Err(Error::ParseError(
            "unclosed eopkg History element".to_string(),
        ));
    }
    if current_delta.is_some() {
        return Err(Error::ParseError("unclosed eopkg Delta record".to_string()));
    }
    Ok(metadata)
}

fn apply_metadata_value(
    path: &[String],
    value: &str,
    metadata: &mut Metadata,
    current_relation: &mut Option<(RelationRecord, RelationDestination)>,
    current_delta: &mut Option<DeltaRecord>,
) -> Result<()> {
    if value.is_empty() {
        return Ok(());
    }
    if let Some((relation, _)) = current_relation.as_mut() {
        relation.name.push_str(value);
    } else if path_ends(path, &["PISI", "Package", "Name"]) {
        metadata.name = value.to_string();
    } else if path_ends(path, &["PISI", "Package", "Summary"]) && metadata.summary.is_none() {
        metadata.summary = Some(value.to_string());
    } else if path_ends(path, &["PISI", "Package", "Description"]) && metadata.description.is_none()
    {
        metadata.description = Some(value.to_string());
    } else if path_ends(path, &["Package", "History", "Update", "Version"])
        && metadata.version.is_empty()
    {
        metadata.version = value.to_string();
    } else if path_ends(path, &["PISI", "Package", "Distribution"]) {
        metadata.distribution = value.to_string();
    } else if path_ends(path, &["PISI", "Package", "DistributionRelease"]) {
        metadata.distribution_release = value.to_string();
    } else if path_ends(path, &["PISI", "Package", "Architecture"]) {
        metadata.architecture = value.to_string();
    } else if path_ends(path, &["PISI", "Package", "PackageFormat"]) {
        metadata.package_format = value.to_string();
    } else if path_ends(path, &["PISI", "Package", "PackageSize"]) {
        metadata.package_size =
            Some(value.parse().map_err(|error| {
                Error::ParseError(format!("invalid eopkg PackageSize: {error}"))
            })?);
    } else if path_ends(path, &["PISI", "Package", "PackageHash"]) {
        metadata.package_hash = Some(value.to_string());
    } else if path_ends(path, &["PISI", "Package", "PackageURI"]) {
        metadata.package_uri = Some(value.to_string());
    } else if path_ends(path, &["Package", "DeltaPackages", "Delta", "PackageURI"]) {
        current_delta
            .as_mut()
            .expect("Delta start owns record")
            .package_uri = value.to_string();
    } else if path_ends(path, &["Package", "DeltaPackages", "Delta", "PackageSize"]) {
        current_delta
            .as_mut()
            .expect("Delta start owns record")
            .package_size = value.parse().map_err(|error| {
            Error::ParseError(format!("invalid eopkg delta PackageSize: {error}"))
        })?;
    } else if path_ends(path, &["Package", "DeltaPackages", "Delta", "PackageHash"])
        || path_ends(path, &["Package", "DeltaPackages", "Delta", "SHA1Sum"])
    {
        current_delta
            .as_mut()
            .expect("Delta start owns record")
            .package_hash = value.to_string();
    } else if path_ends(path, &["Package", "Provides", "PkgConfig"]) {
        metadata.pkg_config.push(value.to_string());
    } else if path_ends(path, &["Package", "Provides", "PkgConfig32"]) {
        metadata.pkg_config_32.push(value.to_string());
    } else if path_ends(path, &["Package", "Provides", "COMAR"]) {
        metadata.comar.push(value.to_string());
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum RelationDestination {
    Dependency,
    AnyDependency,
    Conflict,
    Replace,
}

pub(super) fn parse_files(bytes: &[u8]) -> Result<Vec<FileRecord>> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(false);
    let mut files = Vec::new();
    let mut current: Option<FileRecord> = None;
    let mut field = String::new();
    let mut field_value = String::new();
    let mut xattr_label = None;
    loop {
        match reader
            .read_event()
            .map_err(|error| Error::ParseError(error.to_string()))?
        {
            Event::Start(start) => {
                field = String::from_utf8_lossy(start.local_name().as_ref()).into_owned();
                field_value.clear();
                if field == "File" {
                    if current.replace(FileRecord::default()).is_some() {
                        return Err(Error::ParseError("nested eopkg File record".to_string()));
                    }
                } else if field == "ExtendedAttribute" {
                    xattr_label = attribute(&start, &reader, b"label")?;
                }
            }
            Event::Text(text) => {
                let value = text
                    .xml_content(quick_xml::XmlVersion::Implicit1_0)
                    .map_err(|error| Error::ParseError(error.to_string()))?
                    .to_string();
                if current.is_some() {
                    field_value.push_str(&value);
                }
            }
            Event::CData(data) if current.is_some() => {
                field_value.push_str(
                    &data
                        .xml_content(quick_xml::XmlVersion::Implicit1_0)
                        .map_err(|error| Error::ParseError(error.to_string()))?,
                );
            }
            Event::GeneralRef(reference) if current.is_some() => {
                field_value.push_str(&reference_value(&reference, "files.xml")?);
            }
            Event::End(end) if end.local_name().as_ref() == b"File" => {
                let record = current.take().expect("File start creates current record");
                if record.path.is_empty() || record.file_type.is_empty() {
                    return Err(Error::ParseError(
                        "eopkg File lacks Path or Type".to_string(),
                    ));
                }
                files.push(record);
                field.clear();
                field_value.clear();
            }
            Event::End(_) => {
                if let Some(record) = current.as_mut() {
                    apply_file_field(record, &field, field_value.trim(), &mut xattr_label)?;
                }
                field.clear();
                field_value.clear();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if current.is_some() {
        return Err(Error::ParseError("unclosed eopkg File record".to_string()));
    }
    Ok(files)
}

fn apply_file_field(
    record: &mut FileRecord,
    field: &str,
    value: &str,
    xattr_label: &mut Option<String>,
) -> Result<()> {
    match field {
        "Path" => record.path = value.to_string(),
        "Type" => record.file_type = value.to_string(),
        "Size" => {
            record.size =
                Some(value.parse().map_err(|error| {
                    Error::ParseError(format!("invalid eopkg file size: {error}"))
                })?)
        }
        "Uid" => {
            record.uid = Some(
                value
                    .parse()
                    .map_err(|error| Error::ParseError(format!("invalid eopkg uid: {error}")))?,
            )
        }
        "Gid" => {
            record.gid = Some(
                value
                    .parse()
                    .map_err(|error| Error::ParseError(format!("invalid eopkg gid: {error}")))?,
            )
        }
        "Mode" => {
            record.mode = Some(
                u32::from_str_radix(value, 8)
                    .map_err(|error| Error::ParseError(format!("invalid eopkg mode: {error}")))?,
            )
        }
        "Hash" | "SHA1Sum" => record.sha1 = Some(value.to_string()),
        "Permanent" => record.permanent = value == "true",
        "ExtendedAttribute" => {
            let label = xattr_label
                .take()
                .ok_or_else(|| Error::ParseError("eopkg xattr has no label".to_string()))?;
            use base64::Engine as _;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(value)
                .map_err(|error| {
                    Error::ParseError(format!("invalid eopkg xattr value: {error}"))
                })?;
            record.xattrs.push((label, bytes));
        }
        _ => {}
    }
    Ok(())
}
