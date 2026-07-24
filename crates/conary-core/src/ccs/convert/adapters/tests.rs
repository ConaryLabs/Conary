// conary-core/src/ccs/convert/adapters/tests.rs

use super::*;
use crate::ccs::convert::command_evidence::{CommandEvidenceSource, CommandInvocation};
use crate::ccs::convert::effects::ScriptletClassification;
use crate::ccs::convert::payload_hints::PayloadHints;
use crate::ccs::legacy_scriptlets::{
    CommandArgumentProvenance, CommandExecutionContext, EffectReplacement,
};
use crate::packages::traits::ExtractedFile;

fn invocation(command: &str, argv: &[&str]) -> CommandInvocation {
    CommandInvocation {
        id: format!("entry:line0:cmd0:{command}"),
        entry_id: "entry".to_string(),
        source: CommandEvidenceSource::ShellAst,
        phase: Some("post-install".to_string()),
        lifecycle_paths: vec!["post-install".to_string()],
        interpreter: Some("/bin/sh".to_string()),
        command: command.to_string(),
        command_provenance: CommandArgumentProvenance::Literal,
        argv: argv.iter().map(|arg| arg.to_string()).collect(),
        argument_provenance: vec![CommandArgumentProvenance::Literal; argv.len()],
        execution_context: CommandExecutionContext::Unconditional,
        pipeline_id: None,
        raw_line: Some(format!("{} {}", command, argv.join(" ")).trim().to_string()),
        cwd: None,
        environment: vec![],
    }
}

fn extra_str<'a>(effect: &'a ScriptletEffectEvidence, key: &str) -> Option<&'a str> {
    effect.extra.get(key).and_then(toml::Value::as_str)
}

fn extra_bool(effect: &ScriptletEffectEvidence, key: &str) -> Option<bool> {
    effect.extra.get(key).and_then(toml::Value::as_bool)
}

fn extra_string_array(effect: &ScriptletEffectEvidence, key: &str) -> Vec<String> {
    effect
        .extra
        .get(key)
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_str)
        .map(str::to_string)
        .collect()
}

fn file(path: &str) -> ExtractedFile {
    ExtractedFile {
        path: path.to_string(),
        content: Vec::new(),
        size: 0,
        mode: 0o644,
        sha256: None,
        symlink_target: None,
    }
}

mod desktop;
mod lifecycle;
mod registry;
mod security;
