// crates/conary-core/src/repository/catalog/record/debian.rs

//! Debian source-pocket provenance carried by canonical catalog records.

use serde::{Deserialize, Serialize};

use super::CatalogPackageRecordV1;
use crate::error::{Error, Result};
use crate::repository::registry::validate_source_identifier;
use crate::repository::versioning::VersionScheme;

/// Authenticated Debian repository placement carried by one package record.
///
/// A distribution suite or pocket and component identify where an exact
/// package artifact was published. They remain inspectable provenance but do
/// not change the package's source-independent semantics during profile
/// composition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DebianSourcePocketV1 {
    pub distribution: String,
    pub component: String,
}

impl CatalogPackageRecordV1 {
    /// Return typed Debian source-pocket provenance when parser metadata owns it.
    pub fn debian_source_pocket(&self) -> Result<Option<DebianSourcePocketV1>> {
        self.profile_semantic_metadata()
            .map(|(source_pocket, _)| source_pocket)
    }

    pub(super) fn profile_semantic_metadata(
        &self,
    ) -> Result<(Option<DebianSourcePocketV1>, Option<serde_json::Value>)> {
        let Some(metadata) = &self.metadata else {
            return Ok((None, None));
        };
        let mut semantic =
            serde_json::from_str::<serde_json::Value>(metadata).map_err(|error| {
                Error::ParseError(format!(
                    "catalog package '{}' has invalid metadata JSON: {error}",
                    self.name
                ))
            })?;
        let Some(fields) = semantic.as_object_mut() else {
            return Ok((None, Some(semantic)));
        };
        if fields.get("format").and_then(serde_json::Value::as_str) != Some("deb") {
            return Ok((None, Some(semantic)));
        }
        if self.version_scheme != VersionScheme::Debian {
            return Err(Error::ConfigError(format!(
                "catalog package '{}' carries Debian source-pocket provenance under the {} version scheme",
                self.name,
                self.version_scheme.as_str()
            )));
        }
        let distribution = source_pocket_field(fields, "distribution", &self.name)?;
        let component = source_pocket_field(fields, "component", &self.name)?;
        validate_source_identifier(&distribution, "catalog Debian distribution provenance")?;
        validate_source_identifier(&component, "catalog Debian component provenance")?;
        fields.remove("distribution");
        fields.remove("component");
        Ok((
            Some(DebianSourcePocketV1 {
                distribution,
                component,
            }),
            Some(semantic),
        ))
    }
}

fn source_pocket_field(
    fields: &serde_json::Map<String, serde_json::Value>,
    field: &str,
    package_name: &str,
) -> Result<String> {
    fields
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| {
            Error::ConfigError(format!(
                "catalog Debian package '{package_name}' must carry string source-pocket field '{field}'"
            ))
        })
}
