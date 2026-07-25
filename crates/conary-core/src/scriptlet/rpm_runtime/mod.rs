// conary-core/src/scriptlet/rpm_runtime/mod.rs

//! Conary-owned runtime for RPM scriptlet body transforms and embedded Lua.
//!
//! The implementation consumes only the typed context persisted with the
//! lifecycle entry. It never shells out to RPM or reads a host RPM database.

mod lua;
mod macro_expand;
mod query_format;
mod target;

use crate::ccs::native_lifecycle::{RpmBodyTransform, RpmRuntimeMetadata};
use crate::scriptlet::ScriptletExecutor;
use anyhow::Result;

pub(super) use lua::execute_embedded_lua;
pub(super) use macro_expand::RpmMacroEngine;

/// Apply RPM's install-time script body transforms in header-defined order.
pub(super) fn prepare_body(
    executor: &ScriptletExecutor,
    body: &str,
    environment: &[(String, String)],
    runtime: &RpmRuntimeMetadata,
) -> Result<String> {
    let mut body = body.to_string();
    let macros = RpmMacroEngine::from_runtime(executor, environment, runtime);
    for transform in &runtime.body_transforms {
        body = match transform {
            RpmBodyTransform::MacroExpand => macros.expand(&body)?,
            RpmBodyTransform::HeaderQueryFormat => {
                query_format::format_with_engine(&body, &runtime.header_context, &macros)?
            }
        };
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::prepare_body;
    use crate::ccs::native_lifecycle::{
        RpmBodyTransform, RpmCriticality, RpmHeaderContext, RpmHeaderFact, RpmHeaderFactSource,
        RpmHeaderValue, RpmMacroContext, RpmMacroDefinition, RpmMacroDefinitionSource, RpmProgram,
        RpmRuntimeMetadata,
    };
    use crate::scriptlet::{PackageFormat, ScriptletExecutor};
    use std::path::Path;

    fn runtime(transforms: Vec<RpmBodyTransform>) -> RpmRuntimeMetadata {
        RpmRuntimeMetadata {
            program: RpmProgram::External,
            body_transforms: transforms,
            critical: false,
            criticality: RpmCriticality::WarningOnly,
            raw_flags: 0,
            unknown_flags: 0,
            install_prefixes: Vec::new(),
            macro_context: RpmMacroContext {
                definitions: vec![RpmMacroDefinition {
                    name: "runtime_tag".to_string(),
                    body: "NAME".to_string(),
                    source: RpmMacroDefinitionSource::PackageHeader,
                }],
            },
            header_context: RpmHeaderContext {
                facts: vec![RpmHeaderFact {
                    tag: 1000,
                    name: Some("NAME".to_string()),
                    value: RpmHeaderValue::String("demo".to_string()),
                    source: RpmHeaderFactSource::Header,
                }],
                format_environment: Default::default(),
                database_instance: 0,
            },
            package_rpm_version: Some("6.0.0".to_string()),
        }
    }

    #[test]
    fn rpm_body_transforms_expand_macros_before_query_format() {
        let runtime = runtime(vec![
            RpmBodyTransform::MacroExpand,
            RpmBodyTransform::HeaderQueryFormat,
        ]);

        let executor = ScriptletExecutor::new(Path::new("/"), "demo", "1", PackageFormat::Rpm);
        let transformed = prepare_body(&executor, "name=%%{%{runtime_tag}}", &[], &runtime)
            .expect("body transforms");

        assert_eq!(transformed, "name=demo");
    }
}
