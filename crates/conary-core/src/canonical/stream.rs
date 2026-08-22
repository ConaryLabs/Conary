// conary-core/src/canonical/stream.rs

//! Bounded streaming parser for canonical-map universe objects.

use std::fmt;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use serde::Deserialize;
use serde::de::{DeserializeSeed, Error as _, MapAccess, SeqAccess, Visitor};

use super::exchange::{CanonicalMapEntry, validate_entry};
use crate::error::{Error, Result};

pub(crate) fn for_each_entry(
    path: &Path,
    expected_revision: u64,
    expected_count: u64,
    callback: &mut impl FnMut(CanonicalMapEntry) -> Result<()>,
) -> Result<()> {
    let mut callback_error = None;
    let reader = BufReader::new(File::open(path)?);
    let mut deserializer = serde_json::Deserializer::from_reader(reader);
    let parsed = SnapshotSeed {
        callback,
        callback_error: &mut callback_error,
    }
    .deserialize(&mut deserializer);
    if let Some(error) = callback_error {
        return Err(error);
    }
    let (revision, count) = parsed
        .map_err(|error| Error::ParseError(format!("invalid canonical map JSON: {error}")))?;
    deserializer
        .end()
        .map_err(|error| Error::ParseError(format!("invalid canonical map trailer: {error}")))?;
    if revision != expected_revision || count != expected_count {
        return Err(Error::ConflictError(format!(
            "canonical map facts ({revision}, {count}) disagree with universe manifest ({expected_revision}, {expected_count})"
        )));
    }
    Ok(())
}

struct SnapshotSeed<'a, F> {
    callback: &'a mut F,
    callback_error: &'a mut Option<Error>,
}

impl<'de, F> DeserializeSeed<'de> for SnapshotSeed<'_, F>
where
    F: FnMut(CanonicalMapEntry) -> Result<()>,
{
    type Value = (u64, u64);

    fn deserialize<D>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_map(SnapshotVisitor {
            callback: self.callback,
            callback_error: self.callback_error,
        })
    }
}

struct SnapshotVisitor<'a, F> {
    callback: &'a mut F,
    callback_error: &'a mut Option<Error>,
}

#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "snake_case")]
enum Field {
    SchemaVersion,
    Revision,
    GeneratedAt,
    Entries,
}

impl<'de, F> Visitor<'de> for SnapshotVisitor<'_, F>
where
    F: FnMut(CanonicalMapEntry) -> Result<()>,
{
    type Value = (u64, u64);

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a strict canonical-map object")
    }

    fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut schema_version: Option<u32> = None;
        let mut revision: Option<u64> = None;
        let mut generated_at: Option<Option<String>> = None;
        let mut entry_count: Option<u64> = None;
        while let Some(field) = map.next_key::<Field>()? {
            match field {
                Field::SchemaVersion => {
                    if schema_version.replace(map.next_value()?).is_some() {
                        return Err(A::Error::duplicate_field("schema_version"));
                    }
                }
                Field::Revision => {
                    if revision.replace(map.next_value()?).is_some() {
                        return Err(A::Error::duplicate_field("revision"));
                    }
                }
                Field::GeneratedAt => {
                    if generated_at.replace(map.next_value()?).is_some() {
                        return Err(A::Error::duplicate_field("generated_at"));
                    }
                }
                Field::Entries => {
                    if entry_count.is_some() {
                        return Err(A::Error::duplicate_field("entries"));
                    }
                    entry_count = Some(map.next_value_seed(EntriesSeed {
                        callback: self.callback,
                        callback_error: self.callback_error,
                    })?);
                }
            }
        }
        let schema_version =
            schema_version.ok_or_else(|| A::Error::missing_field("schema_version"))?;
        if schema_version != super::CANONICAL_MAP_SCHEMA_VERSION {
            return Err(A::Error::custom(format!(
                "unsupported canonical map schema version {schema_version}"
            )));
        }
        let revision = revision.ok_or_else(|| A::Error::missing_field("revision"))?;
        let generated_at = generated_at.ok_or_else(|| A::Error::missing_field("generated_at"))?;
        let entry_count = entry_count.ok_or_else(|| A::Error::missing_field("entries"))?;
        match (revision, generated_at.as_deref(), entry_count) {
            (0, None, 0) => {}
            (0, _, _) => {
                return Err(A::Error::custom(
                    "canonical map revision zero must be empty and have no generation timestamp",
                ));
            }
            (_, Some(timestamp), _) => {
                chrono::DateTime::parse_from_rfc3339(timestamp).map_err(A::Error::custom)?;
            }
            (_, None, _) => {
                return Err(A::Error::custom(
                    "positive canonical map revision requires generated_at",
                ));
            }
        }
        Ok((revision, entry_count))
    }
}

struct EntriesSeed<'a, F> {
    callback: &'a mut F,
    callback_error: &'a mut Option<Error>,
}

impl<'de, F> DeserializeSeed<'de> for EntriesSeed<'_, F>
where
    F: FnMut(CanonicalMapEntry) -> Result<()>,
{
    type Value = u64;

    fn deserialize<D>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_seq(EntriesVisitor {
            callback: self.callback,
            callback_error: self.callback_error,
        })
    }
}

struct EntriesVisitor<'a, F> {
    callback: &'a mut F,
    callback_error: &'a mut Option<Error>,
}

impl<'de, F> Visitor<'de> for EntriesVisitor<'_, F>
where
    F: FnMut(CanonicalMapEntry) -> Result<()>,
{
    type Value = u64;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a canonical entry array")
    }

    fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut count = 0_u64;
        let mut previous = None;
        while let Some(entry) = sequence.next_element::<CanonicalMapEntry>()? {
            validate_entry(&entry).map_err(A::Error::custom)?;
            if previous
                .as_deref()
                .is_some_and(|name| name >= entry.canonical.as_str())
            {
                return Err(A::Error::custom(
                    "canonical map entries must be strictly ordered by canonical name",
                ));
            }
            previous = Some(entry.canonical.clone());
            count = count
                .checked_add(1)
                .ok_or_else(|| A::Error::custom("canonical map entry count overflow"))?;
            if let Err(error) = (self.callback)(entry) {
                *self.callback_error = Some(error);
                return Err(A::Error::custom("canonical map callback failed"));
            }
        }
        Ok(count)
    }
}
