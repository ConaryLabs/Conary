// conary-core/src/ccs/native_lifecycle/rpm.rs
//! Persisted RPM lifecycle runtime contracts.

use super::required_string;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpmRuntimeMetadata {
    pub program: RpmProgram,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub body_transforms: Vec<RpmBodyTransform>,
    pub critical: bool,
    pub criticality: RpmCriticality,
    pub raw_flags: u32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub unknown_flags: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub install_prefixes: Vec<String>,
    #[serde(default, skip_serializing_if = "RpmMacroContext::is_empty")]
    pub macro_context: RpmMacroContext,
    #[serde(default, skip_serializing_if = "RpmHeaderContext::is_empty")]
    pub header_context: RpmHeaderContext,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_rpm_version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RpmProgram {
    External,
    EmbeddedLua,
    Sysusers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RpmBodyTransform {
    MacroExpand,
    HeaderQueryFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RpmCriticality {
    Header,
    SlotDefault,
    WarningOnly,
    ForcedWarningOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RpmMacroContext {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub definitions: Vec<RpmMacroDefinition>,
}

impl RpmMacroContext {
    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpmMacroDefinition {
    pub name: String,
    pub body: String,
    pub source: RpmMacroDefinitionSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RpmMacroDefinitionSource {
    PackageHeader,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RpmHeaderContext {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<RpmHeaderFact>,
    pub format_environment: RpmQueryFormatEnvironment,
    /// RPM database header instance, or zero for a package-file header.
    pub database_instance: u32,
}

impl RpmHeaderContext {
    pub fn is_empty(&self) -> bool {
        self.facts.is_empty() && self.format_environment.is_default() && self.database_instance == 0
    }
}

/// Exact locale and timezone inputs used by RPM query-format date renderers.
///
/// Conary stores these with the header contract so conversion and execution
/// never inherit the conversion host's process locale or timezone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RpmQueryFormatEnvironment {
    pub locale: RpmQueryFormatLocale,
    pub timezone: RpmQueryFormatTimezone,
    /// Whether RPM's process-wide verbose query state was enabled.
    pub verbose: bool,
}

impl RpmQueryFormatEnvironment {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RpmQueryFormatLocale {
    /// POSIX C locale, the deterministic locale used by Conary scriptlets.
    #[default]
    C,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RpmQueryFormatTimezone {
    #[default]
    Utc,
    FixedOffset {
        /// Seconds east of UTC, matching POSIX local-time arithmetic.
        seconds_east_of_utc: i32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpmHeaderFact {
    pub tag: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub value: RpmHeaderValue,
    pub source: RpmHeaderFactSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RpmHeaderFactSource {
    Header,
    TransactionDerived,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "kebab-case")]
pub enum RpmHeaderValue {
    Null,
    Binary(String),
    Integer(Vec<u64>),
    String(String),
    StringArray(Vec<String>),
    I18nString(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpmSysusersMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    pub lines: Vec<String>,
    pub directives: Vec<RpmSysusersDirective>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum RpmSysusersDirective {
    User {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        home: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shell: Option<String>,
        #[serde(default)]
        locked: bool,
    },
    Group {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    Member {
        user: String,
        group: String,
    },
}

impl RpmRuntimeMetadata {
    pub(super) fn validate(&self, entry_id: &str) -> Result<()> {
        let expected_critical = !matches!(
            self.criticality,
            RpmCriticality::WarningOnly | RpmCriticality::ForcedWarningOnly
        );
        if self.critical != expected_critical {
            bail!("entry '{entry_id}' RPM critical flag disagrees with its criticality authority");
        }
        if self.unknown_flags != 0 {
            bail!(
                "entry '{entry_id}' RPM scriptlet flags contain unknown bits {:#x}",
                self.unknown_flags
            );
        }
        let mut previous = None;
        for transform in &self.body_transforms {
            if previous == Some(*transform) {
                bail!("entry '{entry_id}' repeats RPM body transform '{transform:?}'");
            }
            if matches!(
                (previous, transform),
                (
                    Some(RpmBodyTransform::HeaderQueryFormat),
                    RpmBodyTransform::MacroExpand
                )
            ) {
                bail!(
                    "entry '{entry_id}' RPM body transforms must preserve macro-expand then query-format order"
                );
            }
            previous = Some(*transform);
        }
        if self
            .body_transforms
            .contains(&RpmBodyTransform::HeaderQueryFormat)
            && self.header_context.is_empty()
        {
            bail!(
                "entry '{entry_id}' declares RPM header query-format expansion without typed header facts"
            );
        }
        if self.program == RpmProgram::Sysusers && !self.body_transforms.is_empty() {
            bail!("entry '{entry_id}' RPM sysusers program declares script body transforms");
        }
        for prefix in &self.install_prefixes {
            required_string("RPM install prefix", prefix)?;
            if !prefix.starts_with('/') || prefix.contains('\0') {
                bail!(
                    "entry '{entry_id}' RPM install prefix '{prefix}' is not an absolute NUL-free path"
                );
            }
        }
        let mut macro_names = BTreeSet::new();
        for definition in &self.macro_context.definitions {
            validate_rpm_identifier("macro", &definition.name, entry_id)?;
            if definition.body.contains('\0') {
                bail!(
                    "entry '{entry_id}' RPM macro '{}' contains NUL",
                    definition.name
                );
            }
            if !macro_names.insert(definition.name.as_str()) {
                bail!(
                    "entry '{entry_id}' repeats RPM macro definition '{}'",
                    definition.name
                );
            }
        }
        let mut header_tags = BTreeSet::new();
        let mut header_names = BTreeSet::new();
        for fact in &self.header_context.facts {
            if !header_tags.insert(fact.tag) {
                bail!("entry '{entry_id}' repeats RPM header tag {}", fact.tag);
            }
            if let Some(name) = &fact.name {
                validate_rpm_identifier("header tag", name, entry_id)?;
                let folded = name.to_ascii_uppercase();
                if !header_names.insert(folded) {
                    bail!("entry '{entry_id}' repeats RPM header tag name '{name}'");
                }
            }
            if let RpmHeaderValue::Binary(value) = &fact.value {
                use base64::Engine as _;
                base64::engine::general_purpose::STANDARD
                    .decode(value)
                    .map_err(|error| {
                        anyhow::anyhow!(
                            "entry '{entry_id}' RPM header tag {} has invalid base64: {error}",
                            fact.tag
                        )
                    })?;
            }
        }
        if let RpmQueryFormatTimezone::FixedOffset {
            seconds_east_of_utc,
        } = self.header_context.format_environment.timezone
            && !(-86_399..=86_399).contains(&seconds_east_of_utc)
        {
            bail!(
                "entry '{entry_id}' RPM query-format timezone offset {seconds_east_of_utc} is outside -86399..=86399"
            );
        }
        if let Some(version) = &self.package_rpm_version {
            required_string("RPM package RPM version", version)?;
        }
        Ok(())
    }
}

impl RpmSysusersMetadata {
    pub(super) fn validate(&self, entry_id: &str) -> Result<()> {
        if self.lines.is_empty() || self.lines.len() != self.directives.len() {
            bail!(
                "entry '{entry_id}' RPM sysusers lines/directives must be non-empty and one-to-one"
            );
        }
        if let Some(path) = &self.source_path
            && (!path.starts_with('/') || !path.ends_with(".conf") || path.contains('\0'))
        {
            bail!(
                "entry '{entry_id}' RPM sysusers source path '{path}' is not an absolute .conf path"
            );
        }
        for line in &self.lines {
            required_string("RPM sysusers source line", line)?;
            if line.contains(['\n', '\r', '\0']) {
                bail!(
                    "entry '{entry_id}' RPM sysusers metadata contains a non-single-line declaration"
                );
            }
        }
        for directive in &self.directives {
            match directive {
                RpmSysusersDirective::User { name, .. }
                | RpmSysusersDirective::Group { name, .. } => {
                    required_string("RPM sysusers account name", name)?;
                }
                RpmSysusersDirective::Member { user, group } => {
                    required_string("RPM sysusers member user", user)?;
                    required_string("RPM sysusers member group", group)?;
                }
            }
        }
        Ok(())
    }
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

fn validate_rpm_identifier(kind: &str, value: &str, entry_id: &str) -> Result<()> {
    required_string(kind, value)?;
    if value.contains('\0')
        || !value
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
    {
        bail!("entry '{entry_id}' RPM {kind} '{value}' is malformed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        RpmHeaderContext, RpmQueryFormatEnvironment, RpmQueryFormatLocale, RpmQueryFormatTimezone,
    };

    #[test]
    fn query_format_environment_is_required_persisted_state() {
        let context = RpmHeaderContext {
            facts: Vec::new(),
            format_environment: RpmQueryFormatEnvironment {
                locale: RpmQueryFormatLocale::C,
                timezone: RpmQueryFormatTimezone::FixedOffset {
                    seconds_east_of_utc: 3_600,
                },
                verbose: true,
            },
            database_instance: 17,
        };
        let encoded = serde_json::to_value(&context).expect("serialize header context");
        assert_eq!(encoded["format_environment"]["locale"], "c");
        assert_eq!(
            encoded["format_environment"]["timezone"]["seconds_east_of_utc"],
            3_600
        );
        assert_eq!(encoded["database_instance"], 17);
        assert_eq!(encoded["format_environment"]["verbose"], true);

        let error = serde_json::from_value::<RpmHeaderContext>(serde_json::json!({
            "facts": [],
            "database_instance": 0
        }))
        .expect_err("the clean runtime contract requires its environment");
        assert!(error.to_string().contains("format_environment"));

        let error = serde_json::from_value::<RpmHeaderContext>(serde_json::json!({
            "facts": [],
            "format_environment": {
                "locale": "c",
                "timezone": { "kind": "utc" }
            },
            "database_instance": 0
        }))
        .expect_err("the clean runtime contract requires explicit verbose state");
        assert!(error.to_string().contains("verbose"));
    }
}
