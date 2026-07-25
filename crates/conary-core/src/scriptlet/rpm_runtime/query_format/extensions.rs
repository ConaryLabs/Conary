// conary-core/src/scriptlet/rpm_runtime/query_format/extensions.rs
//! Exact RPM computed-header extensions derived from persisted typed facts.
//! Source authority: RPM `lib/tagexts.cc` at
//! `a8f0192aee1c08bd1454ed2ac6ebaf506004b55c`.

use crate::ccs::native_lifecycle::{
    RpmHeaderContext, RpmHeaderFact, RpmHeaderFactSource, RpmHeaderValue,
};
use anyhow::{Result, bail};
use base64::Engine as _;
use std::collections::BTreeMap;

pub(super) enum ExtensionFact {
    NotExtension,
    Missing,
    Value(RpmHeaderFact),
}

pub(super) fn resolve(header: &RpmHeaderContext, tag: rpm::IndexTag) -> Result<ExtensionFact> {
    use rpm::IndexTag::*;

    let value = match tag {
        RPMTAG_FILECLASS => file_types(header, FileTypeKind::Class)?,
        RPMTAG_FILEPROVIDE => file_dependencies(header, DependencyKind::Provide)?,
        RPMTAG_FILEREQUIRE => file_dependencies(header, DependencyKind::Require)?,
        RPMTAG_TRIGGERCONDS => trigger_conditions(header, TriggerKind::Package)?,
        RPMTAG_FILETRIGGERCONDS => trigger_conditions(header, TriggerKind::File)?,
        RPMTAG_TRANSFILETRIGGERCONDS => trigger_conditions(header, TriggerKind::TransactionFile)?,
        RPMTAG_TRIGGERTYPE => trigger_types(header, TriggerKind::Package)?,
        RPMTAG_FILETRIGGERTYPE => trigger_types(header, TriggerKind::File)?,
        RPMTAG_TRANSFILETRIGGERTYPE => trigger_types(header, TriggerKind::TransactionFile)?,
        RPMTAG_LONGFILESIZES => long_integer(header, RPMTAG_LONGFILESIZES, RPMTAG_FILESIZES)?,
        RPMTAG_LONGARCHIVESIZE => long_integer(header, RPMTAG_LONGARCHIVESIZE, RPMTAG_ARCHIVESIZE)?,
        RPMTAG_LONGSIZE => long_integer(header, RPMTAG_LONGSIZE, RPMTAG_SIZE)?,
        RPMTAG_LONGSIGSIZE => long_integer(header, RPMTAG_LONGSIGSIZE, RPMTAG_SIGSIZE)?,
        RPMTAG_DBINSTANCE => Some(RpmHeaderValue::Integer(vec![u64::from(
            header.database_instance,
        )])),
        RPMTAG_HEADERCOLOR => Some(RpmHeaderValue::Integer(vec![header_color(header)?])),
        RPMTAG_VERBOSE => header
            .format_environment
            .verbose
            .then_some(RpmHeaderValue::Integer(vec![1])),
        RPMTAG_REQUIRENEVRS => dependency_nevrs(header, DependencySet::Require)?,
        RPMTAG_RECOMMENDNEVRS => dependency_nevrs(header, DependencySet::Recommend)?,
        RPMTAG_SUGGESTNEVRS => dependency_nevrs(header, DependencySet::Suggest)?,
        RPMTAG_SUPPLEMENTNEVRS => dependency_nevrs(header, DependencySet::Supplement)?,
        RPMTAG_ENHANCENEVRS => dependency_nevrs(header, DependencySet::Enhance)?,
        RPMTAG_PROVIDENEVRS => dependency_nevrs(header, DependencySet::Provide)?,
        RPMTAG_OBSOLETENEVRS => dependency_nevrs(header, DependencySet::Obsolete)?,
        RPMTAG_CONFLICTNEVRS => dependency_nevrs(header, DependencySet::Conflict)?,
        RPMTAG_FILENLINKS => file_link_counts(header)?,
        RPMTAG_SYSUSERS => sysusers(header)?,
        RPMTAG_FILEMIMES => file_types(header, FileTypeKind::Mime)?,
        RPMTAG_OPENPGP => openpgp(header)?,
        RPMTAG_RPMFORMAT => Some(rpm_format(header)?),
        _ => return Ok(ExtensionFact::NotExtension),
    };

    Ok(value.map_or(ExtensionFact::Missing, |value| {
        ExtensionFact::Value(RpmHeaderFact {
            tag: tag as u32,
            name: tag.to_string().strip_prefix("RPMTAG_").map(str::to_string),
            value,
            source: RpmHeaderFactSource::Header,
        })
    }))
}

fn raw(header: &RpmHeaderContext, tag: rpm::IndexTag) -> Option<&RpmHeaderFact> {
    header.facts.iter().find(|fact| fact.tag == tag as u32)
}

fn integer_array(header: &RpmHeaderContext, tag: rpm::IndexTag) -> Result<Option<&[u64]>> {
    match raw(header, tag).map(|fact| &fact.value) {
        None | Some(RpmHeaderValue::Null) => Ok(None),
        Some(RpmHeaderValue::Integer(values)) => Ok(Some(values)),
        Some(_) => bail!(
            "RpmQueryFormatHeaderError: {} must be an integer array",
            tag_name(tag)
        ),
    }
}

fn string_array(header: &RpmHeaderContext, tag: rpm::IndexTag) -> Result<Option<&[String]>> {
    match raw(header, tag).map(|fact| &fact.value) {
        None | Some(RpmHeaderValue::Null) => Ok(None),
        Some(RpmHeaderValue::StringArray(values)) | Some(RpmHeaderValue::I18nString(values)) => {
            Ok(Some(values))
        }
        Some(_) => bail!(
            "RpmQueryFormatHeaderError: {} must be a string array",
            tag_name(tag)
        ),
    }
}

fn tag_name(tag: rpm::IndexTag) -> String {
    tag.to_string()
        .strip_prefix("RPMTAG_")
        .unwrap_or("unnamed")
        .to_string()
}

fn file_count(header: &RpmHeaderContext) -> Result<usize> {
    Ok(string_array(header, rpm::IndexTag::RPMTAG_BASENAMES)?.map_or(0, <[String]>::len))
}

#[derive(Clone, Copy)]
enum FileTypeKind {
    Class,
    Mime,
}

fn file_types(header: &RpmHeaderContext, kind: FileTypeKind) -> Result<Option<RpmHeaderValue>> {
    use rpm::IndexTag::*;

    let count = file_count(header)?;
    if count == 0 {
        return Ok(None);
    }
    let (index_tag, dictionary_tag) = match kind {
        FileTypeKind::Class => (RPMTAG_FILECLASS, RPMTAG_CLASSDICT),
        FileTypeKind::Mime => (RPMTAG_FILEMIMES, RPMTAG_MIMEDICT),
    };
    let indexes = integer_array(header, index_tag)?.unwrap_or_default();
    let dictionary = string_array(header, dictionary_tag)?.unwrap_or_default();
    let modes = integer_array(header, RPMTAG_FILEMODES)?.unwrap_or_default();
    let links = string_array(header, RPMTAG_FILELINKTOS)?.unwrap_or_default();

    let values = (0..count)
        .map(|index| {
            let declared = indexes
                .get(index)
                .and_then(|dictionary_index| usize::try_from(*dictionary_index).ok())
                .and_then(|dictionary_index| dictionary.get(dictionary_index))
                .filter(|value| !value.is_empty());
            declared.cloned().unwrap_or_else(|| {
                fallback_file_type(
                    kind,
                    modes.get(index).copied().unwrap_or_default(),
                    links.get(index).map(String::as_str).unwrap_or_default(),
                )
            })
        })
        .collect();
    Ok(Some(RpmHeaderValue::StringArray(values)))
}

fn fallback_file_type(kind: FileTypeKind, mode: u64, link: &str) -> String {
    let file_type = mode & 0o170000;
    match (kind, file_type) {
        (FileTypeKind::Class, 0o060000) => "block special".to_string(),
        (FileTypeKind::Class, 0o020000) => "character special".to_string(),
        (FileTypeKind::Class, 0o040000) => "directory".to_string(),
        (FileTypeKind::Class, 0o010000) => "fifo (named pipe)".to_string(),
        (FileTypeKind::Class, 0o140000) => "socket".to_string(),
        (FileTypeKind::Class, 0o120000) => format!("symbolic link to `{link}'"),
        (FileTypeKind::Mime, 0o060000) => "inode/blockdevice".to_string(),
        (FileTypeKind::Mime, 0o020000) => "inode/chardevice".to_string(),
        (FileTypeKind::Mime, 0o040000) => "inode/directory".to_string(),
        (FileTypeKind::Mime, 0o010000) => "inode/fifo".to_string(),
        (FileTypeKind::Mime, 0o140000) => "inode/socket".to_string(),
        (FileTypeKind::Mime, 0o120000) => "inode/symlink".to_string(),
        _ => String::new(),
    }
}

#[derive(Clone, Copy)]
enum DependencyKind {
    Provide,
    Require,
}

fn file_dependencies(
    header: &RpmHeaderContext,
    kind: DependencyKind,
) -> Result<Option<RpmHeaderValue>> {
    use rpm::IndexTag::*;

    let count = file_count(header)?;
    if count == 0 {
        return Ok(None);
    }
    let starts = integer_array(header, RPMTAG_FILEDEPENDSX)?.unwrap_or_default();
    let lengths = integer_array(header, RPMTAG_FILEDEPENDSN)?.unwrap_or_default();
    let dictionary = integer_array(header, RPMTAG_DEPENDSDICT)?.unwrap_or_default();
    let marker = match kind {
        DependencyKind::Provide => b'P',
        DependencyKind::Require => b'R',
    };
    let dependency_set = match kind {
        DependencyKind::Provide => DependencySet::Provide,
        DependencyKind::Require => DependencySet::Require,
    };
    let arrays = dependency_arrays(header, dependency_set)?;

    let mut values = Vec::with_capacity(count);
    for file_index in 0..count {
        let start = starts
            .get(file_index)
            .copied()
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or_default();
        let length = lengths
            .get(file_index)
            .copied()
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or_default();
        let mut dependencies = Vec::new();
        let entries = start
            .checked_add(length)
            .and_then(|end| dictionary.get(start..end))
            .unwrap_or_default();
        for packed in entries {
            if ((packed >> 24) & 0xff) as u8 != marker {
                continue;
            }
            let dependency_index = (packed & 0x00ff_ffff) as usize;
            if let Some(dependency) = arrays.render(dependency_index) {
                dependencies.push(dependency);
            }
        }
        values.push(dependencies.join(" "));
    }
    Ok(Some(RpmHeaderValue::StringArray(values)))
}

#[derive(Clone, Copy)]
enum TriggerKind {
    Package,
    File,
    TransactionFile,
}

struct TriggerTags {
    names: rpm::IndexTag,
    indexes: rpm::IndexTag,
    flags: rpm::IndexTag,
    versions: rpm::IndexTag,
    scripts: rpm::IndexTag,
}

fn trigger_tags(kind: TriggerKind) -> TriggerTags {
    use rpm::IndexTag::*;
    match kind {
        TriggerKind::Package => TriggerTags {
            names: RPMTAG_TRIGGERNAME,
            indexes: RPMTAG_TRIGGERINDEX,
            flags: RPMTAG_TRIGGERFLAGS,
            versions: RPMTAG_TRIGGERVERSION,
            scripts: RPMTAG_TRIGGERSCRIPTS,
        },
        TriggerKind::File => TriggerTags {
            names: RPMTAG_FILETRIGGERNAME,
            indexes: RPMTAG_FILETRIGGERINDEX,
            flags: RPMTAG_FILETRIGGERFLAGS,
            versions: RPMTAG_FILETRIGGERVERSION,
            scripts: RPMTAG_FILETRIGGERSCRIPTS,
        },
        TriggerKind::TransactionFile => TriggerTags {
            names: RPMTAG_TRANSFILETRIGGERNAME,
            indexes: RPMTAG_TRANSFILETRIGGERINDEX,
            flags: RPMTAG_TRANSFILETRIGGERFLAGS,
            versions: RPMTAG_TRANSFILETRIGGERVERSION,
            scripts: RPMTAG_TRANSFILETRIGGERSCRIPTS,
        },
    }
}

fn trigger_conditions(
    header: &RpmHeaderContext,
    kind: TriggerKind,
) -> Result<Option<RpmHeaderValue>> {
    let tags = trigger_tags(kind);
    let Some(names) = string_array(header, tags.names)? else {
        return Ok(None);
    };
    let script_count = value_len(raw(header, tags.scripts).map(|fact| &fact.value));
    let indexes = integer_array(header, tags.indexes)?.unwrap_or_default();
    let flags = integer_array(header, tags.flags)?.unwrap_or_default();
    let versions = string_array(header, tags.versions)?.unwrap_or_default();
    ensure_parallel(names.len(), indexes.len(), tags.indexes)?;
    ensure_parallel(names.len(), flags.len(), tags.flags)?;
    ensure_parallel(names.len(), versions.len(), tags.versions)?;

    let mut values = Vec::with_capacity(script_count);
    for script_index in 0..script_count {
        let conditions = names
            .iter()
            .enumerate()
            .filter(|(index, _)| indexes[*index] as usize == script_index)
            .map(|(index, name)| dependency_nevr(name, flags[index], &versions[index]))
            .collect::<Vec<_>>();
        values.push(conditions.join(", "));
    }
    Ok((script_count > 0).then_some(RpmHeaderValue::StringArray(values)))
}

fn trigger_types(header: &RpmHeaderContext, kind: TriggerKind) -> Result<Option<RpmHeaderValue>> {
    let tags = trigger_tags(kind);
    let Some(indexes) = integer_array(header, tags.indexes)? else {
        return Ok(None);
    };
    let script_count = value_len(raw(header, tags.scripts).map(|fact| &fact.value));
    let flags = integer_array(header, tags.flags)?.unwrap_or_default();
    ensure_parallel(indexes.len(), flags.len(), tags.flags)?;
    let values = (0..script_count)
        .map(|script_index| {
            indexes
                .iter()
                .position(|index| *index as usize == script_index)
                .and_then(|index| flags.get(index))
                .map_or_else(String::new, |flags| trigger_type(*flags))
        })
        .collect();
    Ok((script_count > 0).then_some(RpmHeaderValue::StringArray(values)))
}

fn ensure_parallel(expected: usize, actual: usize, tag: rpm::IndexTag) -> Result<()> {
    if expected != actual {
        bail!(
            "RpmQueryFormatHeaderError: {} cardinality {actual} does not match {expected}",
            tag_name(tag)
        );
    }
    Ok(())
}

fn value_len(value: Option<&RpmHeaderValue>) -> usize {
    match value {
        None | Some(RpmHeaderValue::Null) => 0,
        Some(RpmHeaderValue::Integer(values)) => values.len(),
        Some(RpmHeaderValue::StringArray(values)) | Some(RpmHeaderValue::I18nString(values)) => {
            values.len()
        }
        Some(RpmHeaderValue::Binary(_)) | Some(RpmHeaderValue::String(_)) => 1,
    }
}

fn trigger_type(flags: u64) -> String {
    for (bit, value) in [
        (1 << 25, "prein"),
        (1 << 16, "in"),
        (1 << 17, "un"),
        (1 << 18, "postun"),
    ] {
        if flags & bit != 0 {
            return value.to_string();
        }
    }
    String::new()
}

fn long_integer(
    header: &RpmHeaderContext,
    long_tag: rpm::IndexTag,
    legacy_tag: rpm::IndexTag,
) -> Result<Option<RpmHeaderValue>> {
    if let Some(values) = integer_array(header, long_tag)? {
        return Ok(Some(RpmHeaderValue::Integer(values.to_vec())));
    }
    Ok(integer_array(header, legacy_tag)?.map(|values| RpmHeaderValue::Integer(values.to_vec())))
}

fn header_color(header: &RpmHeaderContext) -> Result<u64> {
    Ok(integer_array(header, rpm::IndexTag::RPMTAG_FILECOLORS)?
        .unwrap_or_default()
        .iter()
        .fold(0, |color, file_color| color | file_color)
        & 0x0f)
}

#[derive(Clone, Copy)]
enum DependencySet {
    Require,
    Recommend,
    Suggest,
    Supplement,
    Enhance,
    Provide,
    Obsolete,
    Conflict,
    OldSuggest,
    OldEnhance,
}

struct DependencyTags {
    names: rpm::IndexTag,
    versions: rpm::IndexTag,
    flags: rpm::IndexTag,
}

fn dependency_tags(set: DependencySet) -> DependencyTags {
    use rpm::IndexTag::*;
    match set {
        DependencySet::Require => DependencyTags {
            names: RPMTAG_REQUIRENAME,
            versions: RPMTAG_REQUIREVERSION,
            flags: RPMTAG_REQUIREFLAGS,
        },
        DependencySet::Recommend => DependencyTags {
            names: RPMTAG_RECOMMENDNAME,
            versions: RPMTAG_RECOMMENDVERSION,
            flags: RPMTAG_RECOMMENDFLAGS,
        },
        DependencySet::Suggest => DependencyTags {
            names: RPMTAG_SUGGESTNAME,
            versions: RPMTAG_SUGGESTVERSION,
            flags: RPMTAG_SUGGESTFLAGS,
        },
        DependencySet::Supplement => DependencyTags {
            names: RPMTAG_SUPPLEMENTNAME,
            versions: RPMTAG_SUPPLEMENTVERSION,
            flags: RPMTAG_SUPPLEMENTFLAGS,
        },
        DependencySet::Enhance => DependencyTags {
            names: RPMTAG_ENHANCENAME,
            versions: RPMTAG_ENHANCEVERSION,
            flags: RPMTAG_ENHANCEFLAGS,
        },
        DependencySet::Provide => DependencyTags {
            names: RPMTAG_PROVIDENAME,
            versions: RPMTAG_PROVIDEVERSION,
            flags: RPMTAG_PROVIDEFLAGS,
        },
        DependencySet::Obsolete => DependencyTags {
            names: RPMTAG_OBSOLETENAME,
            versions: RPMTAG_OBSOLETEVERSION,
            flags: RPMTAG_OBSOLETEFLAGS,
        },
        DependencySet::Conflict => DependencyTags {
            names: RPMTAG_CONFLICTNAME,
            versions: RPMTAG_CONFLICTVERSION,
            flags: RPMTAG_CONFLICTFLAGS,
        },
        DependencySet::OldSuggest => DependencyTags {
            names: RPMTAG_OLDSUGGESTSNAME,
            versions: RPMTAG_OLDSUGGESTSVERSION,
            flags: RPMTAG_OLDSUGGESTSFLAGS,
        },
        DependencySet::OldEnhance => DependencyTags {
            names: RPMTAG_OLDENHANCESNAME,
            versions: RPMTAG_OLDENHANCESVERSION,
            flags: RPMTAG_OLDENHANCESFLAGS,
        },
    }
}

struct DependencyArrays<'a> {
    names: &'a [String],
    versions: &'a [String],
    flags: &'a [u64],
}

impl DependencyArrays<'_> {
    fn render(&self, index: usize) -> Option<String> {
        self.names.get(index).map(|name| {
            dependency_nevr(
                name,
                self.flags.get(index).copied().unwrap_or_default(),
                self.versions
                    .get(index)
                    .map(String::as_str)
                    .unwrap_or_default(),
            )
        })
    }
}

fn dependency_arrays(
    header: &RpmHeaderContext,
    set: DependencySet,
) -> Result<DependencyArrays<'_>> {
    let tags = dependency_tags(set);
    Ok(DependencyArrays {
        names: string_array(header, tags.names)?.unwrap_or_default(),
        versions: string_array(header, tags.versions)?.unwrap_or_default(),
        flags: integer_array(header, tags.flags)?.unwrap_or_default(),
    })
}

fn dependency_nevrs(
    header: &RpmHeaderContext,
    set: DependencySet,
) -> Result<Option<RpmHeaderValue>> {
    let primary = dependency_arrays(header, set)?;
    if !primary.names.is_empty() {
        return Ok(Some(RpmHeaderValue::StringArray(
            (0..primary.names.len())
                .filter_map(|index| primary.render(index))
                .collect(),
        )));
    }

    let fallback = match set {
        DependencySet::Recommend => Some((DependencySet::OldSuggest, true)),
        DependencySet::Suggest => Some((DependencySet::OldSuggest, false)),
        DependencySet::Supplement => Some((DependencySet::OldEnhance, true)),
        DependencySet::Enhance => Some((DependencySet::OldEnhance, false)),
        _ => None,
    };
    let Some((fallback, strong)) = fallback else {
        return Ok(None);
    };
    let fallback = dependency_arrays(header, fallback)?;
    let values = (0..fallback.names.len())
        .filter(|index| {
            let is_strong = fallback
                .flags
                .get(*index)
                .is_some_and(|flags| flags & (1 << 27) != 0);
            is_strong == strong
        })
        .filter_map(|index| fallback.render(index))
        .collect::<Vec<_>>();
    Ok((!values.is_empty()).then_some(RpmHeaderValue::StringArray(values)))
}

fn dependency_nevr(name: &str, flags: u64, version: &str) -> String {
    let mut value = name.to_string();
    if flags & 0x0e != 0 {
        value.push(' ');
        if flags & (1 << 1) != 0 {
            value.push('<');
        }
        if flags & (1 << 2) != 0 {
            value.push('>');
        }
        if flags & (1 << 3) != 0 {
            value.push('=');
        }
    }
    if !version.is_empty() {
        value.push(' ');
        value.push_str(version);
    }
    value
}

fn file_link_counts(header: &RpmHeaderContext) -> Result<Option<RpmHeaderValue>> {
    use rpm::IndexTag::*;

    let count = file_count(header)?;
    if count == 0 {
        return Ok(None);
    }
    let modes = integer_array(header, RPMTAG_FILEMODES)?.unwrap_or_default();
    let flags = integer_array(header, RPMTAG_FILEFLAGS)?.unwrap_or_default();
    let inodes = integer_array(header, RPMTAG_FILEINODES)?.unwrap_or_default();
    let devices = integer_array(header, RPMTAG_FILEDEVICES)?.unwrap_or_default();
    let mut groups: BTreeMap<(u64, u64), usize> = BTreeMap::new();
    for index in 0..count {
        let mode = modes.get(index).copied().unwrap_or_default();
        let file_flags = flags.get(index).copied().unwrap_or_default();
        let inode = inodes.get(index).copied().unwrap_or_default();
        let device = devices.get(index).copied();
        if mode & 0o170000 == 0o100000
            && file_flags & (1 << 6) == 0
            && inode > 0
            && let Some(device) = device
        {
            *groups.entry((device, inode)).or_default() += 1;
        }
    }
    let values = (0..count)
        .map(|index| {
            let mode = modes.get(index).copied().unwrap_or_default();
            let file_flags = flags.get(index).copied().unwrap_or_default();
            let inode = inodes.get(index).copied().unwrap_or_default();
            let device = devices.get(index).copied();
            if mode & 0o170000 == 0o100000
                && file_flags & (1 << 6) == 0
                && inode > 0
                && let Some(device) = device
            {
                groups.get(&(device, inode)).copied().unwrap_or(1) as u64
            } else {
                1
            }
        })
        .collect();
    Ok(Some(RpmHeaderValue::Integer(values)))
}

fn sysusers(header: &RpmHeaderContext) -> Result<Option<RpmHeaderValue>> {
    let arrays = dependency_arrays(header, DependencySet::Provide)?;
    let mut values = Vec::new();
    for (index, name) in arrays.names.iter().enumerate() {
        let flags = arrays.flags.get(index).copied().unwrap_or_default();
        if flags & (1 << 3) == 0
            || !(name.starts_with("user(")
                || name.starts_with("group(")
                || name.starts_with("groupmember("))
        {
            continue;
        }
        let Some(encoded) = arrays.versions.get(index) else {
            continue;
        };
        let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
            continue;
        };
        if let Ok(line) = String::from_utf8(decoded) {
            values.push(line);
        }
    }
    Ok((!values.is_empty()).then_some(RpmHeaderValue::StringArray(values)))
}

fn openpgp(header: &RpmHeaderContext) -> Result<Option<RpmHeaderValue>> {
    use rpm::IndexTag::*;

    if let Some(value) = raw(header, RPMTAG_OPENPGP) {
        return Ok(Some(value.value.clone()));
    }
    let mut signatures = Vec::new();
    for tag in [
        RPMTAG_RSAHEADER,
        RPMTAG_DSAHEADER,
        RPMTAG_SIGPGP,
        RPMTAG_SIGGPG,
    ] {
        if let Some(RpmHeaderFact {
            value: RpmHeaderValue::Binary(value),
            ..
        }) = raw(header, tag)
        {
            signatures.push(value.clone());
        }
    }
    Ok((!signatures.is_empty()).then_some(RpmHeaderValue::StringArray(signatures)))
}

fn rpm_format(header: &RpmHeaderContext) -> Result<RpmHeaderValue> {
    use rpm::IndexTag::*;

    if let Some(values) = integer_array(header, RPMTAG_RPMFORMAT)? {
        return Ok(RpmHeaderValue::Integer(values.to_vec()));
    }
    Ok(RpmHeaderValue::Integer(vec![
        if raw(header, RPMTAG_HEADERIMMUTABLE).is_some() {
            4
        } else {
            3
        },
    ]))
}
