// conary-core/src/ccs/convert/command_evidence.rs

use crate::ccs::native_lifecycle::CommandArgumentProvenance;
use crate::packages::native_abi::{
    ArchNativeScriptletMetadata, NativeLifecyclePath, NativeScriptletEntry, NativeScriptletKind,
    NativeScriptletMetadata,
};
use crate::packages::traits::DiagnosticScriptletEvidence;
use tree_sitter::{Node, Parser, Point};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum CommandExecutionContext {
    Unconditional,
    Conditional,
    Loop,
    Function,
    Pipeline,
    Subshell,
    CommandSubstitution,
    #[default]
    Unresolved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum CommandEvidenceSource {
    ShellAst,
    NativeShellAst,
    #[default]
    Unresolved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandInvocation {
    pub id: String,
    pub entry_id: String,
    pub source: CommandEvidenceSource,
    pub phase: Option<String>,
    pub lifecycle_paths: Vec<String>,
    pub interpreter: Option<String>,
    pub command: String,
    pub command_provenance: CommandArgumentProvenance,
    pub argv: Vec<String>,
    pub argument_provenance: Vec<CommandArgumentProvenance>,
    pub execution_context: CommandExecutionContext,
    pub pipeline_id: Option<usize>,
    pub raw_line: Option<String>,
    pub cwd: Option<String>,
    pub environment: Vec<CommandEnvironmentFact>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ShellParseError {
    #[error("failed to initialize the tree-sitter Bash grammar")]
    LanguageInitialization,
    #[error("tree-sitter returned no Bash syntax tree")]
    MissingTree,
    #[error("shell syntax error at {start_row}:{start_column}-{end_row}:{end_column}")]
    Syntax {
        start_row: usize,
        start_column: usize,
        end_row: usize,
        end_column: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandEnvironmentFact {
    pub name: String,
    pub value: Option<String>,
}

pub fn extract_scriptlet_invocations(
    entry_id: &str,
    scriptlet: &DiagnosticScriptletEvidence,
) -> Result<Vec<CommandInvocation>, ShellParseError> {
    if !is_shell_interpreter(&scriptlet.interpreter) {
        return Ok(Vec::new());
    }

    extract_invocations_from_text(InvocationText {
        entry_id,
        content: &scriptlet.content,
        source: CommandEvidenceSource::ShellAst,
        phase: Some(scriptlet.phase.to_string()),
        lifecycle_paths: vec![scriptlet.phase.to_string()],
        interpreter: Some(scriptlet.interpreter.clone()),
    })
}

pub fn extract_native_entry_invocations(
    entry: &NativeScriptletEntry,
) -> Result<Vec<CommandInvocation>, ShellParseError> {
    if entry.kind != NativeScriptletKind::Executable {
        return Ok(Vec::new());
    }
    let Some(content) = native_entry_invocation_text(entry) else {
        return Ok(Vec::new());
    };

    if !entry
        .interpreter
        .as_deref()
        .is_none_or(is_shell_interpreter)
    {
        return Ok(Vec::new());
    }

    extract_invocations_from_text(InvocationText {
        entry_id: &entry.id,
        content: &content,
        source: CommandEvidenceSource::NativeShellAst,
        phase: Some(native_lifecycle_label(entry.primary_lifecycle).to_string()),
        lifecycle_paths: entry
            .lifecycle_paths
            .iter()
            .copied()
            .map(native_lifecycle_label)
            .map(str::to_string)
            .collect(),
        interpreter: entry.interpreter.clone(),
    })
}

fn native_entry_invocation_text(entry: &NativeScriptletEntry) -> Option<String> {
    match &entry.metadata {
        NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::Install(metadata)) => {
            diagnostic_arch_function_body(
                entry.body.text.as_deref()?,
                metadata.function_name.as_str(),
            )
        }
        _ => entry.body.text.clone(),
    }
}

fn diagnostic_arch_function_body(source: &str, function_name: &str) -> Option<String> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .ok()?;
    let tree = parser.parse(source, None)?;
    diagnostic_arch_function_body_in_node(tree.root_node(), source, function_name)
}

fn diagnostic_arch_function_body_in_node(
    node: Node<'_>,
    source: &str,
    function_name: &str,
) -> Option<String> {
    if node.kind() == "function_definition" {
        let name = node.child_by_field_name("name")?;
        if node_text(name, source) == function_name {
            return node
                .child_by_field_name("body")
                .map(|body| node_text(body, source).to_string());
        }
    }

    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find_map(|child| diagnostic_arch_function_body_in_node(child, source, function_name))
}

pub fn extract_invocations_from_shell_text(
    entry_id: &str,
    content: &str,
    phase: Option<&str>,
) -> Result<Vec<CommandInvocation>, ShellParseError> {
    extract_invocations_from_text(InvocationText {
        entry_id,
        content,
        source: CommandEvidenceSource::ShellAst,
        phase: phase.map(str::to_string),
        lifecycle_paths: phase.map(str::to_string).into_iter().collect(),
        interpreter: Some("/bin/sh".to_string()),
    })
}

struct InvocationText<'a> {
    entry_id: &'a str,
    content: &'a str,
    source: CommandEvidenceSource,
    phase: Option<String>,
    lifecycle_paths: Vec<String>,
    interpreter: Option<String>,
}

fn extract_invocations_from_text(
    input: InvocationText<'_>,
) -> Result<Vec<CommandInvocation>, ShellParseError> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .map_err(|_| ShellParseError::LanguageInitialization)?;
    let tree = parser
        .parse(input.content, None)
        .ok_or(ShellParseError::MissingTree)?;
    let root = tree.root_node();
    if root.has_error() {
        let range = first_syntax_error(root)
            .map(|node| node.range())
            .unwrap_or_else(|| root.range());
        return Err(syntax_error(range.start_point, range.end_point));
    }

    let mut invocations = Vec::new();
    collect_command_nodes(
        root,
        CommandExecutionContext::Unconditional,
        &input,
        &mut invocations,
    );
    Ok(invocations)
}

fn collect_command_nodes(
    node: Node<'_>,
    inherited_context: CommandExecutionContext,
    input: &InvocationText<'_>,
    invocations: &mut Vec<CommandInvocation>,
) {
    let context = context_for_node(node.kind(), inherited_context);
    if node.kind() == "command"
        && let Some(invocation) = invocation_from_command_node(node, context, input)
    {
        invocations.push(invocation);
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_command_nodes(child, context, input, invocations);
    }
}

fn invocation_from_command_node(
    node: Node<'_>,
    execution_context: CommandExecutionContext,
    input: &InvocationText<'_>,
) -> Option<CommandInvocation> {
    let name = node.child_by_field_name("name")?;
    let command_provenance = token_provenance(name, input.content);
    let command_text = node_text(name, input.content);
    let command = if command_provenance == CommandArgumentProvenance::Literal {
        decode_literal_shell_word(command_text)
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_string()
    } else {
        "<dynamic-command>".to_string()
    };
    if command.is_empty() {
        return None;
    }

    let mut argv = Vec::new();
    let mut argument_provenance = Vec::new();
    let mut environment = Vec::new();
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            let child = cursor.node();
            match cursor.field_name() {
                Some("argument") => {
                    let provenance = token_provenance(child, input.content);
                    let raw = node_text(child, input.content);
                    let value = if provenance == CommandArgumentProvenance::Literal {
                        decode_literal_shell_word(raw)
                    } else {
                        raw.to_string()
                    };
                    argv.push(value);
                    argument_provenance.push(provenance);
                }
                _ if child.kind() == "variable_assignment" => {
                    if let Some(fact) = environment_fact_from_node(child, input.content) {
                        environment.push(fact);
                    }
                }
                _ => {}
            }
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
    append_redirect_suffix_arguments(node, input.content, &mut argv, &mut argument_provenance);

    Some(CommandInvocation {
        id: format!("{}:byte{}", input.entry_id, node.start_byte()),
        entry_id: input.entry_id.to_string(),
        source: input.source,
        phase: input.phase.clone(),
        lifecycle_paths: input.lifecycle_paths.clone(),
        interpreter: input.interpreter.clone(),
        command,
        command_provenance,
        argv,
        argument_provenance,
        execution_context,
        pipeline_id: ancestor_start_byte(node, "pipeline"),
        raw_line: Some(node_text(node, input.content).to_string()),
        cwd: None,
        environment,
    })
}

fn ancestor_start_byte(mut node: Node<'_>, kind: &str) -> Option<usize> {
    while let Some(parent) = node.parent() {
        if parent.kind() == kind {
            return Some(parent.start_byte());
        }
        node = parent;
    }
    None
}

fn append_redirect_suffix_arguments(
    command: Node<'_>,
    source: &str,
    argv: &mut Vec<String>,
    argument_provenance: &mut Vec<CommandArgumentProvenance>,
) {
    let Some(parent) = command.parent() else {
        return;
    };
    if parent.kind() != "redirected_statement" {
        return;
    }

    let mut parent_cursor = parent.walk();
    for redirect in parent.named_children(&mut parent_cursor) {
        if redirect.kind() != "file_redirect" || redirect.start_byte() < command.end_byte() {
            continue;
        }
        let mut destination_seen = false;
        let mut redirect_cursor = redirect.walk();
        if !redirect_cursor.goto_first_child() {
            continue;
        }
        loop {
            let child = redirect_cursor.node();
            if redirect_cursor.field_name() == Some("destination") {
                if destination_seen {
                    let provenance = token_provenance(child, source);
                    let raw = node_text(child, source);
                    let value = if provenance == CommandArgumentProvenance::Literal {
                        decode_literal_shell_word(raw)
                    } else {
                        raw.to_string()
                    };
                    argv.push(value);
                    argument_provenance.push(provenance);
                } else {
                    destination_seen = true;
                }
            }
            if !redirect_cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

fn environment_fact_from_node(node: Node<'_>, source: &str) -> Option<CommandEnvironmentFact> {
    let name = node.child_by_field_name("name")?;
    let name = node_text(name, source);
    let value = node
        .child_by_field_name("value")
        .map(|value| node_text(value, source).to_string());
    Some(CommandEnvironmentFact {
        name: name.to_string(),
        value,
    })
}

fn token_provenance(node: Node<'_>, source: &str) -> CommandArgumentProvenance {
    let mut kinds = Vec::new();
    collect_dynamic_kinds(node, source, &mut kinds);
    kinds.sort();
    kinds.dedup();
    match kinds.as_slice() {
        [] => CommandArgumentProvenance::Literal,
        [kind] => *kind,
        _ => CommandArgumentProvenance::Mixed,
    }
}

fn collect_dynamic_kinds(node: Node<'_>, source: &str, kinds: &mut Vec<CommandArgumentProvenance>) {
    match node.kind() {
        "command_substitution" => kinds.push(CommandArgumentProvenance::CommandSubstitution),
        "process_substitution" => kinds.push(CommandArgumentProvenance::ProcessSubstitution),
        "simple_expansion" | "expansion" | "arithmetic_expansion" => {
            kinds.push(CommandArgumentProvenance::Expansion);
        }
        "extglob_pattern" => kinds.push(CommandArgumentProvenance::Glob),
        "word" if contains_unescaped_glob(node_text(node, source)) => {
            kinds.push(CommandArgumentProvenance::Glob);
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_dynamic_kinds(child, source, kinds);
    }
}

fn contains_unescaped_glob(value: &str) -> bool {
    let mut escaped = false;
    for ch in value.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
        } else if matches!(ch, '*' | '?' | '[') {
            return true;
        }
    }
    false
}

fn decode_literal_shell_word(value: &str) -> String {
    let mut decoded = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    let mut quote = None;
    while let Some(ch) = chars.next() {
        match quote {
            Some('\'') => {
                if ch == '\'' {
                    quote = None;
                } else {
                    decoded.push(ch);
                }
            }
            Some('"') => match ch {
                '"' => quote = None,
                '\\' => {
                    if let Some(next) = chars.peek().copied()
                        && matches!(next, '$' | '`' | '"' | '\\' | '\n')
                    {
                        chars.next();
                        if next != '\n' {
                            decoded.push(next);
                        }
                    } else {
                        decoded.push(ch);
                    }
                }
                _ => decoded.push(ch),
            },
            _ => match ch {
                '\'' | '"' => quote = Some(ch),
                '\\' => {
                    if let Some(next) = chars.next()
                        && next != '\n'
                    {
                        decoded.push(next);
                    }
                }
                _ => decoded.push(ch),
            },
        }
    }
    decoded
}

fn context_for_node(kind: &str, inherited: CommandExecutionContext) -> CommandExecutionContext {
    match kind {
        "command_substitution" => CommandExecutionContext::CommandSubstitution,
        "function_definition" => CommandExecutionContext::Function,
        "for_statement" | "c_style_for_statement" | "while_statement" => {
            CommandExecutionContext::Loop
        }
        "if_statement" | "case_statement" | "list" => CommandExecutionContext::Conditional,
        "pipeline" => CommandExecutionContext::Pipeline,
        "subshell" => CommandExecutionContext::Subshell,
        _ => inherited,
    }
}

fn first_syntax_error(node: Node<'_>) -> Option<Node<'_>> {
    if node.is_error() || node.is_missing() {
        return Some(node);
    }
    let mut cursor = node.walk();
    node.children(&mut cursor).find_map(first_syntax_error)
}

fn syntax_error(start: Point, end: Point) -> ShellParseError {
    ShellParseError::Syntax {
        start_row: start.row + 1,
        start_column: start.column + 1,
        end_row: end.row + 1,
        end_column: end.column + 1,
    }
}

fn node_text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    node.utf8_text(source.as_bytes()).unwrap_or_default()
}

fn is_shell_interpreter(interpreter: &str) -> bool {
    let interpreter = interpreter.split_whitespace().next().unwrap_or(interpreter);
    let command = interpreter.rsplit('/').next().unwrap_or(interpreter);
    matches!(command, "sh" | "bash" | "dash" | "ksh" | "zsh")
}

fn native_lifecycle_label(path: NativeLifecyclePath) -> &'static str {
    match path {
        NativeLifecyclePath::PackageControl => "package-control",
        NativeLifecyclePath::PreInstall => "pre-install",
        NativeLifecyclePath::Sysusers => "rpm-sysusers",
        NativeLifecyclePath::PostInstall => "post-install",
        NativeLifecyclePath::PreUpgrade => "pre-upgrade",
        NativeLifecyclePath::PostUpgrade => "post-upgrade",
        NativeLifecyclePath::PreRemove => "pre-remove",
        NativeLifecyclePath::PostRemove => "post-remove",
        NativeLifecyclePath::PreTransaction => "pre-transaction",
        NativeLifecyclePath::PostTransaction => "post-transaction",
        NativeLifecyclePath::PreUntransaction => "pre-untransaction",
        NativeLifecyclePath::PostUntransaction => "post-untransaction",
        NativeLifecyclePath::Verify => "verify",
        NativeLifecyclePath::Config => "config",
        NativeLifecyclePath::Trigger => "trigger",
        NativeLifecyclePath::FileTrigger => "file-trigger",
        NativeLifecyclePath::TransactionFileTrigger => "transaction-file-trigger",
        NativeLifecyclePath::Purge => "purge",
        NativeLifecyclePath::Abort => "abort",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packages::native_abi::*;
    use crate::packages::traits::{DiagnosticScriptletEvidence, DiagnosticScriptletPhase};

    fn scriptlet(content: &str) -> DiagnosticScriptletEvidence {
        DiagnosticScriptletEvidence {
            phase: DiagnosticScriptletPhase::PostInstall,
            interpreter: "/bin/sh".to_string(),
            content: content.to_string(),
            flags: None,
        }
    }

    fn native_entry(content: &str, support: NativeScriptletSupport) -> NativeScriptletEntry {
        NativeScriptletEntry {
            id: "rpm:%post".to_string(),
            format: NativeScriptletFormat::Rpm,
            kind: NativeScriptletKind::Executable,
            native_slot: "%post".to_string(),
            primary_lifecycle: NativeLifecyclePath::PostInstall,
            lifecycle_paths: vec![NativeLifecyclePath::PostInstall],
            interpreter: Some("/bin/sh".to_string()),
            interpreter_args: vec![],
            body: NativeScriptletBody::from_bytes(content.as_bytes().to_vec()),
            invocation: NativeInvocationContract::none(),
            order: NativeTransactionOrder::new(NativeTransactionPosition::AfterPayload),
            support,
            metadata: NativeScriptletMetadata::Rpm(RpmNativeScriptletMetadata {
                slot: RpmScriptletSlot::Post,
                runtime: rpm_runtime(),
                trigger: None,
                sysusers: None,
            }),
        }
    }

    fn rpm_runtime() -> RpmScriptletRuntimeMetadata {
        RpmScriptletRuntimeMetadata {
            program: RpmScriptletProgram::External,
            flags: RpmScriptletFlagsMetadata {
                names: Vec::new(),
                raw_bits: 0,
                unknown_bits: 0,
                expand: false,
                query_format: false,
                critical: false,
                criticality: RpmScriptletCriticality::WarningOnly,
            },
            install_prefixes: Vec::new(),
            macro_context: Default::default(),
            header_context: Default::default(),
            package_rpm_version: None,
        }
    }

    fn arch_install_entry(install_source: &str, function_name: &str) -> NativeScriptletEntry {
        NativeScriptletEntry {
            id: format!("arch:{function_name}"),
            format: NativeScriptletFormat::Arch,
            kind: NativeScriptletKind::Executable,
            native_slot: function_name.to_string(),
            primary_lifecycle: NativeLifecyclePath::PostInstall,
            lifecycle_paths: vec![NativeLifecyclePath::PostInstall],
            interpreter: None,
            interpreter_args: vec![],
            body: NativeScriptletBody::from_bytes(install_source.as_bytes().to_vec()),
            invocation: NativeInvocationContract::none(),
            order: NativeTransactionOrder::new(NativeTransactionPosition::AfterPayload),
            support: NativeScriptletSupport::Parsed,
            metadata: NativeScriptletMetadata::Arch(ArchNativeScriptletMetadata::Install(
                ArchInstallScriptletMetadata {
                    install_source_sha256: crate::hash::sha256_prefixed(install_source.as_bytes()),
                    function_name: function_name.to_string(),
                    selection_contract:
                        crate::packages::native_abi::ArchInstallSelectionContract::LibalpmGrepV1,
                },
            )),
        }
    }

    #[test]
    fn command_evidence_splits_control_operators_with_stable_ids() {
        let invocations = extract_scriptlet_invocations(
            "rpm:%post",
            &scriptlet("VAR=1 /usr/bin/systemctl daemon-reload && /sbin/ldconfig\n"),
        )
        .unwrap();

        assert_eq!(invocations.len(), 2);
        assert_eq!(invocations[0].id, "rpm:%post:byte0");
        assert_eq!(invocations[0].entry_id, "rpm:%post");
        assert_eq!(invocations[0].source, CommandEvidenceSource::ShellAst);
        assert_eq!(invocations[0].phase.as_deref(), Some("post-install"));
        assert_eq!(invocations[0].lifecycle_paths, vec!["post-install"]);
        assert_eq!(invocations[0].interpreter.as_deref(), Some("/bin/sh"));
        assert_eq!(
            invocations[0].environment,
            vec![CommandEnvironmentFact {
                name: "VAR".to_string(),
                value: Some("1".to_string()),
            }]
        );
        assert_eq!(invocations[0].command, "systemctl");
        assert_eq!(invocations[0].argv, vec!["daemon-reload"]);
        assert_eq!(
            invocations[0].execution_context,
            CommandExecutionContext::Conditional
        );
        assert_eq!(invocations[1].command, "ldconfig");
    }

    #[test]
    fn command_evidence_preserves_wrapper_semantics_instead_of_guessing_through_them() {
        let invocations = extract_scriptlet_invocations(
            "deb:postinst",
            &scriptlet("env -i chroot /target /usr/bin/update-mime-database /usr/share/mime\n"),
        )
        .unwrap();

        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].command, "env");
        assert_eq!(
            invocations[0].argv,
            vec![
                "-i",
                "chroot",
                "/target",
                "/usr/bin/update-mime-database",
                "/usr/share/mime"
            ]
        );
        assert_eq!(
            invocations[0].raw_line.as_deref(),
            Some("env -i chroot /target /usr/bin/update-mime-database /usr/share/mime")
        );
    }

    #[test]
    fn command_evidence_does_not_promote_nested_wrapper_text_to_an_invocation() {
        let invocations = extract_scriptlet_invocations(
            "rpm:%post",
            &scriptlet("sudo -u nobody chroot /target /sbin/ldconfig\n"),
        )
        .unwrap();

        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].command, "sudo");
        assert_eq!(
            invocations[0].argv,
            vec!["-u", "nobody", "chroot", "/target", "/sbin/ldconfig"]
        );
    }

    #[test]
    fn command_evidence_ignores_non_shell_interpreters() {
        let mut perl = scriptlet("#!/usr/bin/perl\nprint 'ok';\n");
        perl.interpreter = "/usr/bin/perl".to_string();

        assert!(
            extract_scriptlet_invocations("deb:config", &perl)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn native_command_evidence_preserves_native_metadata_source() {
        let invocations = extract_native_entry_invocations(&native_entry(
            "/sbin/ldconfig\n",
            NativeScriptletSupport::Parsed,
        ))
        .unwrap();

        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].id, "rpm:%post:byte0");
        assert_eq!(invocations[0].entry_id, "rpm:%post");
        assert_eq!(invocations[0].source, CommandEvidenceSource::NativeShellAst);
        assert_eq!(invocations[0].phase.as_deref(), Some("post-install"));
        assert_eq!(invocations[0].lifecycle_paths, vec!["post-install"]);
        assert_eq!(invocations[0].command, "ldconfig");
    }

    #[test]
    fn native_command_evidence_uses_arch_install_function_body() {
        let install_source = "\
pre_install() {
    custom-pre-helper --mutate
}

post_install() {
    /sbin/ldconfig
}
";
        let entry = arch_install_entry(install_source, "post_install");

        let invocations = extract_native_entry_invocations(&entry).unwrap();

        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].entry_id, "arch:post_install");
        assert_eq!(invocations[0].command, "ldconfig");
        assert!(invocations[0].argv.is_empty());
    }

    #[test]
    fn command_evidence_extracts_invocations_from_shell_text() {
        let invocations = extract_invocations_from_shell_text(
            "recipe:make",
            "npm install atomic-lockfile && bun add js-digest",
            Some("make"),
        )
        .unwrap();

        assert_eq!(invocations.len(), 2);
        assert_eq!(invocations[0].entry_id, "recipe:make");
        assert_eq!(invocations[0].source, CommandEvidenceSource::ShellAst);
        assert_eq!(invocations[0].phase.as_deref(), Some("make"));
        assert_eq!(invocations[0].lifecycle_paths, vec!["make"]);
        assert_eq!(invocations[0].interpreter.as_deref(), Some("/bin/sh"));
        assert_eq!(invocations[0].command, "npm");
        assert_eq!(invocations[0].argv, vec!["install", "atomic-lockfile"]);
        assert_eq!(invocations[1].command, "bun");
        assert_eq!(invocations[1].argv, vec!["add", "js-digest"]);
        assert!(invocations.iter().all(|invocation| {
            invocation.execution_context == CommandExecutionContext::Conditional
        }));
    }

    #[test]
    fn parser_rejects_malformed_shell_instead_of_emitting_fake_commands() {
        let error = extract_invocations_from_shell_text(
            "rpm:%post",
            "if true; then systemctl daemon-reload",
            Some("post-install"),
        )
        .unwrap_err();

        assert!(matches!(error, ShellParseError::Syntax { .. }));
    }

    #[test]
    fn parser_records_dynamic_arguments_and_nested_execution_context() {
        let invocations = extract_invocations_from_shell_text(
            "deb:postinst",
            "if test \"$1\" = configure; then helper \"$@\"; fi",
            Some("post-install"),
        )
        .unwrap();
        let helper = invocations
            .iter()
            .find(|invocation| invocation.command == "helper")
            .unwrap();

        assert_eq!(
            helper.argument_provenance,
            [CommandArgumentProvenance::Expansion]
        );
        assert_eq!(
            helper.execution_context,
            CommandExecutionContext::Conditional
        );
    }

    #[test]
    fn parser_preserves_quoted_maintscript_argv_forwarding() {
        let invocations = extract_invocations_from_shell_text(
            "deb:preinst",
            "dpkg-maintscript-helper rm_conffile /etc/demo.conf 2.0-1~ demo -- \"$@\"",
            Some("pre-install"),
        )
        .unwrap();

        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].argv.last().unwrap(), "\"$@\"");
        assert_eq!(
            invocations[0].argument_provenance.last().unwrap(),
            &CommandArgumentProvenance::Expansion
        );
    }

    #[test]
    fn parser_preserves_arguments_after_file_descriptor_redirection() {
        let invocations = extract_invocations_from_shell_text(
            "recipe:make",
            "go 2>&1 install example.org/tool",
            Some("make"),
        )
        .unwrap();

        assert_eq!(invocations.len(), 1);
        assert_eq!(
            invocations[0].argv,
            ["install".to_string(), "example.org/tool".to_string()]
        );
    }
}
