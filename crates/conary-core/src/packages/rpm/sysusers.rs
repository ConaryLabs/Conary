// conary-core/src/packages/rpm/sysusers.rs
//! RPM 4.19+ `%sysusers` dependency decoding.

use super::*;
use base64::Engine as _;

const FILE_DEPENDENCY_KIND_PROVIDE: u32 = b'P' as u32;
const FILE_DEPENDENCY_INDEX_MASK: u32 = 0x00ff_ffff;

#[derive(Debug)]
struct DecodedSysuser {
    provide_index: usize,
    line: String,
    directive: RpmSysusersDirective,
}

impl RpmPackage {
    pub(super) fn extract_rpm_sysusers(pkg: &Package) -> Result<Vec<RpmSysusersMetadata>> {
        let provides = match pkg.metadata.get_provides() {
            Ok(provides) => provides,
            Err(rpm::Error::TagNotFound(_)) => return Ok(Vec::new()),
            Err(error) => {
                return Err(Error::ParseError(format!(
                    "Failed to read RPM provides for %sysusers: {error}"
                )));
            }
        };

        let mut decoded = Vec::new();
        for (provide_index, provide) in provides.iter().enumerate() {
            let Some(identity) = SysuserProvideIdentity::parse(&provide.name) else {
                continue;
            };
            if !provide.flags.contains(DependencyFlags::EQUAL) {
                continue;
            }
            if provide.version.is_empty() {
                return Err(Error::ParseError(format!(
                    "RPM %sysusers provide '{}' has an empty encoded declaration",
                    provide.name
                )));
            }

            let line = decode_sysusers_line(&provide.version, &provide.name)?;
            let directive = parse_sysusers_directive(&line, &identity)?;
            decoded.push(DecodedSysuser {
                provide_index,
                line,
                directive,
            });
        }
        if decoded.is_empty() {
            return Ok(Vec::new());
        }

        let provider_files = rpm_provider_files(&pkg.metadata, provides.len())?;
        let mut groups = Vec::<RpmSysusersMetadata>::new();
        for decoded in decoded {
            let source_path = provider_files
                .get(decoded.provide_index)
                .and_then(Clone::clone);
            let group_index = groups
                .iter()
                .position(|group| group.source_path == source_path)
                .unwrap_or_else(|| {
                    groups.push(RpmSysusersMetadata {
                        source_path: source_path.clone(),
                        lines: Vec::new(),
                        directives: Vec::new(),
                    });
                    groups.len() - 1
                });
            groups[group_index].lines.push(decoded.line);
            groups[group_index].directives.push(decoded.directive);
        }
        Ok(groups)
    }
}

#[derive(Debug)]
enum SysuserProvideIdentity {
    User(String),
    Group(String),
    Member { user: String, group: String },
}

impl SysuserProvideIdentity {
    fn parse(name: &str) -> Option<Self> {
        if let Some(value) = exact_call_argument(name, "user") {
            return Some(Self::User(value.to_string()));
        }
        if let Some(value) = exact_call_argument(name, "group") {
            return Some(Self::Group(value.to_string()));
        }
        let value = exact_call_argument(name, "groupmember")?;
        let (user, group) = value.split_once('/')?;
        if user.is_empty() || group.is_empty() || group.contains('/') {
            return None;
        }
        Some(Self::Member {
            user: user.to_string(),
            group: group.to_string(),
        })
    }
}

fn exact_call_argument<'a>(value: &'a str, function: &str) -> Option<&'a str> {
    let argument = value.strip_prefix(function)?.strip_prefix('(')?;
    let argument = argument.strip_suffix(')')?;
    (!argument.is_empty() && !argument.contains(['(', ')'])).then_some(argument)
}

fn decode_sysusers_line(encoded: &str, provide_name: &str) -> Result<String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| {
            Error::ParseError(format!(
                "RPM %sysusers provide '{provide_name}' has invalid base64: {error}"
            ))
        })?;
    let content_len = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    if bytes[content_len..].iter().any(|byte| *byte != 0) {
        return Err(Error::ParseError(format!(
            "RPM %sysusers provide '{provide_name}' has non-NUL data after its encoded declaration"
        )));
    }
    let line = std::str::from_utf8(&bytes[..content_len]).map_err(|error| {
        Error::ParseError(format!(
            "RPM %sysusers provide '{provide_name}' is not UTF-8: {error}"
        ))
    })?;
    if line.is_empty() || line.contains(['\n', '\r', '\0']) {
        return Err(Error::ParseError(format!(
            "RPM %sysusers provide '{provide_name}' does not encode one declaration"
        )));
    }
    Ok(line.to_string())
}

fn parse_sysusers_directive(
    line: &str,
    identity: &SysuserProvideIdentity,
) -> Result<RpmSysusersDirective> {
    let fields = shlex::split(line).ok_or_else(|| {
        Error::ParseError(format!(
            "RPM %sysusers declaration has invalid quoting: {line}"
        ))
    })?;
    let kind = fields.first().map(String::as_str).unwrap_or_default();
    match (kind, identity) {
        ("u" | "u!", SysuserProvideIdentity::User(expected)) => {
            if !(2..=6).contains(&fields.len()) {
                return Err(invalid_sysusers_shape(
                    line,
                    "u/u! requires 2 through 6 fields",
                ));
            }
            ensure_identity("user", &fields[1], expected, line)?;
            Ok(RpmSysusersDirective::User {
                name: fields[1].clone(),
                id: optional_field(fields.get(2)),
                description: optional_field(fields.get(3)),
                home: optional_field(fields.get(4)),
                shell: optional_field(fields.get(5)),
                locked: kind == "u!",
            })
        }
        ("g", SysuserProvideIdentity::Group(expected)) => {
            if !(2..=6).contains(&fields.len())
                || fields
                    .get(3..)
                    .is_some_and(|extra| extra.iter().any(|field| field != "-"))
            {
                return Err(invalid_sysusers_shape(
                    line,
                    "g requires a name, optional ID, and only '-' in unused fields",
                ));
            }
            ensure_identity("group", &fields[1], expected, line)?;
            Ok(RpmSysusersDirective::Group {
                name: fields[1].clone(),
                id: optional_field(fields.get(2)),
            })
        }
        ("m", SysuserProvideIdentity::Member { user, group }) => {
            if fields.len() != 3 {
                return Err(invalid_sysusers_shape(
                    line,
                    "m requires exactly a user and group",
                ));
            }
            ensure_identity("group member user", &fields[1], user, line)?;
            ensure_identity("group member group", &fields[2], group, line)?;
            Ok(RpmSysusersDirective::Member {
                user: fields[1].clone(),
                group: fields[2].clone(),
            })
        }
        ("u" | "u!" | "g" | "m", _) => Err(Error::ParseError(format!(
            "RPM %sysusers provide identity does not match declaration: {line}"
        ))),
        _ => Err(Error::ParseError(format!(
            "RPM encoded %sysusers declaration uses unsupported directive '{kind}': {line}"
        ))),
    }
}

fn optional_field(value: Option<&String>) -> Option<String> {
    value.filter(|value| value.as_str() != "-").cloned()
}

fn ensure_identity(kind: &str, actual: &str, expected: &str, line: &str) -> Result<()> {
    if actual != expected {
        return Err(Error::ParseError(format!(
            "RPM %sysusers {kind} '{actual}' does not match provide identity '{expected}': {line}"
        )));
    }
    Ok(())
}

fn invalid_sysusers_shape(line: &str, expectation: &str) -> Error {
    Error::ParseError(format!(
        "RPM %sysusers declaration has an invalid field contract ({expectation}): {line}"
    ))
}

fn rpm_provider_files(
    metadata: &rpm::PackageMetadata,
    provide_count: usize,
) -> Result<Vec<Option<String>>> {
    let file_starts = optional_u32_array(metadata, rpm::IndexTag::RPMTAG_FILEDEPENDSX)?;
    let file_counts = optional_u32_array(metadata, rpm::IndexTag::RPMTAG_FILEDEPENDSN)?;
    let dependency_dictionary = optional_u32_array(metadata, rpm::IndexTag::RPMTAG_DEPENDSDICT)?;
    if file_starts.is_empty() && file_counts.is_empty() && dependency_dictionary.is_empty() {
        return Ok(vec![None; provide_count]);
    }
    let paths = metadata.get_file_paths().map_err(|error| {
        Error::ParseError(format!(
            "Failed to read RPM file paths for %sysusers: {error}"
        ))
    })?;
    if file_starts.len() != paths.len() || file_counts.len() != paths.len() {
        return Err(Error::ParseError(format!(
            "RPM file dependency arrays do not match file count: starts={}, counts={}, files={}",
            file_starts.len(),
            file_counts.len(),
            paths.len()
        )));
    }

    let mut provider_files = vec![None; provide_count];
    for (file_index, path) in paths.iter().enumerate() {
        let start = usize::try_from(file_starts[file_index]).map_err(|_| {
            Error::ParseError("RPM file dependency start does not fit usize".to_string())
        })?;
        let count = usize::try_from(file_counts[file_index]).map_err(|_| {
            Error::ParseError("RPM file dependency count does not fit usize".to_string())
        })?;
        let end = start
            .checked_add(count)
            .ok_or_else(|| Error::ParseError("RPM file dependency range overflows".to_string()))?;
        let entries = dependency_dictionary.get(start..end).ok_or_else(|| {
            Error::ParseError(format!(
                "RPM file dependency range {start}..{end} is outside dictionary length {}",
                dependency_dictionary.len()
            ))
        })?;
        for packed in entries {
            if packed >> 24 != FILE_DEPENDENCY_KIND_PROVIDE {
                continue;
            }
            let provide_index = usize::try_from(packed & FILE_DEPENDENCY_INDEX_MASK)
                .expect("24-bit RPM dependency index fits usize");
            let target = provider_files.get_mut(provide_index).ok_or_else(|| {
                Error::ParseError(format!(
                    "RPM file dependency references missing provide index {provide_index}"
                ))
            })?;
            let normalized = normalize_rpm_header_path(path);
            match target {
                Some(existing) if existing != &normalized => {
                    return Err(Error::ParseError(format!(
                        "RPM provide index {provide_index} is attached to multiple files"
                    )));
                }
                Some(_) => {}
                None => *target = Some(normalized),
            }
        }
    }
    Ok(provider_files)
}

fn optional_u32_array(metadata: &rpm::PackageMetadata, tag: rpm::IndexTag) -> Result<Vec<u32>> {
    match metadata.header.get_entry_data_as_u32_array(tag) {
        Ok(values) => Ok(values),
        Err(rpm::Error::TagNotFound(_)) => Ok(Vec::new()),
        Err(error) => Err(Error::ParseError(format!(
            "Failed to read RPM header tag {tag:?}: {error}"
        ))),
    }
}

fn normalize_rpm_header_path(path: &std::path::Path) -> String {
    let value = path.to_string_lossy();
    if let Some(relative) = value.strip_prefix("./") {
        format!("/{relative}")
    } else if value.starts_with('/') {
        value.into_owned()
    } else {
        format!("/{value}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity_user() -> SysuserProvideIdentity {
        SysuserProvideIdentity::User("dnsmasq".to_string())
    }

    #[test]
    fn parses_quoted_user_fields_without_losing_the_source_line() {
        let line = r#"u! dnsmasq - "Dnsmasq DHCP and DNS server" /var/lib/dnsmasq"#;
        let directive = parse_sysusers_directive(line, &identity_user()).unwrap();

        assert_eq!(
            directive,
            RpmSysusersDirective::User {
                name: "dnsmasq".to_string(),
                id: None,
                description: Some("Dnsmasq DHCP and DNS server".to_string()),
                home: Some("/var/lib/dnsmasq".to_string()),
                shell: None,
                locked: true,
            }
        );
    }

    #[test]
    fn rejects_a_decoded_line_that_disagrees_with_the_typed_provide() {
        let error = parse_sysusers_directive(
            "u other -",
            &SysuserProvideIdentity::User("expected".to_string()),
        )
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("does not match provide identity")
        );
    }

    #[test]
    fn base64_decoder_accepts_only_trailing_nul_padding() {
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"g input -\0\0");
        assert_eq!(
            decode_sysusers_line(&encoded, "group(input)").unwrap(),
            "g input -"
        );

        let encoded = base64::engine::general_purpose::STANDARD.encode(b"g input\0x");
        assert!(decode_sysusers_line(&encoded, "group(input)").is_err());
    }
}
