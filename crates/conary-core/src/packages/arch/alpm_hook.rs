// conary-core/src/packages/arch/alpm_hook.rs

//! Strict parser for the current `alpm-hooks(5)` control-file grammar.

use crate::error::{Error, Result};
use crate::packages::traits::{
    ArchAlpmHookAction, ArchAlpmHookMetadata, ArchAlpmHookOperation, ArchAlpmHookTrigger,
    ArchAlpmHookTriggerType, NativeTransactionPosition,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    None,
    Trigger,
    Action,
}

#[derive(Default)]
struct TriggerBuilder {
    operations: Vec<ArchAlpmHookOperation>,
    trigger_type: Option<ArchAlpmHookTriggerType>,
    targets: Vec<String>,
}

impl TriggerBuilder {
    fn finish(self, path: &str, line: usize) -> Result<ArchAlpmHookTrigger> {
        if self.operations.is_empty() {
            return Err(parse_error(
                path,
                line,
                "each [Trigger] requires at least one Operation",
            ));
        }
        let trigger_type = self
            .trigger_type
            .ok_or_else(|| parse_error(path, line, "each [Trigger] requires Type"))?;
        if self.targets.is_empty() {
            return Err(parse_error(
                path,
                line,
                "each [Trigger] requires at least one Target",
            ));
        }
        Ok(ArchAlpmHookTrigger {
            operations: self.operations,
            trigger_type,
            targets: self.targets,
        })
    }
}

#[derive(Default)]
struct ActionBuilder {
    description: Option<String>,
    when: Option<NativeTransactionPosition>,
    exec: Option<String>,
    depends: Vec<String>,
    abort_on_fail: bool,
    needs_targets: bool,
}

impl ActionBuilder {
    fn finish(self, path: &str, line: usize) -> Result<ArchAlpmHookAction> {
        let when = self
            .when
            .ok_or_else(|| parse_error(path, line, "[Action] requires When"))?;
        let exec = self
            .exec
            .ok_or_else(|| parse_error(path, line, "[Action] requires Exec"))?;
        let argv = split_alpm_words(&exec)
            .filter(|argv| !argv.is_empty())
            .ok_or_else(|| parse_error(path, line, "Exec is not a valid quoted command line"))?;
        Ok(ArchAlpmHookAction {
            description: self.description,
            when,
            exec,
            argv,
            depends: self.depends,
            abort_on_fail: self.abort_on_fail,
            needs_targets: self.needs_targets,
        })
    }
}

pub(super) fn parse(path: &str, bytes: &[u8]) -> Result<ArchAlpmHookMetadata> {
    package_hook_basename(path)?;
    let text = std::str::from_utf8(bytes)
        .map_err(|error| parse_error(path, 0, &format!("hook is not UTF-8: {error}")))?;

    let mut section = Section::None;
    let mut triggers = Vec::new();
    let mut current_trigger: Option<TriggerBuilder> = None;
    let mut action: Option<ActionBuilder> = None;
    let mut last_line = 0;

    for (index, raw_line) in text.lines().enumerate() {
        let line_number = index + 1;
        last_line = line_number;
        // libalpm's C parser observes the bytes before the first NUL and only
        // treats `#` as a comment when it is the first non-whitespace byte.
        let line = raw_line.split('\0').next().unwrap_or("").trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            match &line[1..line.len() - 1] {
                "Trigger" => {
                    if let Some(trigger) = current_trigger.take() {
                        triggers.push(trigger.finish(path, line_number)?);
                    }
                    current_trigger = Some(TriggerBuilder::default());
                    section = Section::Trigger;
                }
                "Action" => {
                    if let Some(trigger) = current_trigger.take() {
                        triggers.push(trigger.finish(path, line_number)?);
                    }
                    action.get_or_insert_with(ActionBuilder::default);
                    section = Section::Action;
                }
                name => {
                    return Err(parse_error(
                        path,
                        line_number,
                        &format!("unknown section '[{name}]'"),
                    ));
                }
            }
            continue;
        }

        let (key, value) = line
            .split_once('=')
            .map(|(key, value)| (key.trim(), Some(value.trim())))
            .unwrap_or((line, None));

        match section {
            Section::None => {
                return Err(parse_error(
                    path,
                    line_number,
                    "directive appears before a [Trigger] or [Action] section",
                ));
            }
            Section::Trigger => {
                let trigger = current_trigger
                    .as_mut()
                    .expect("trigger section has a builder");
                let value = value.ok_or_else(|| {
                    parse_error(
                        path,
                        line_number,
                        &format!("{key} requires Key = Value syntax"),
                    )
                })?;
                match key {
                    "Operation" => {
                        let operation = match value {
                            "Install" => ArchAlpmHookOperation::Install,
                            "Upgrade" => ArchAlpmHookOperation::Upgrade,
                            "Remove" => ArchAlpmHookOperation::Remove,
                            _ => {
                                return Err(parse_error(
                                    path,
                                    line_number,
                                    &format!("unsupported Operation '{value}'"),
                                ));
                            }
                        };
                        if !trigger.operations.contains(&operation) {
                            trigger.operations.push(operation);
                        }
                    }
                    "Type" => {
                        trigger.trigger_type = Some(match value {
                            "Package" => ArchAlpmHookTriggerType::Package,
                            "Path" => ArchAlpmHookTriggerType::Path,
                            _ => {
                                return Err(parse_error(
                                    path,
                                    line_number,
                                    &format!("unsupported Type '{value}'"),
                                ));
                            }
                        });
                    }
                    "Target" => trigger.targets.push(value.to_string()),
                    _ => {
                        return Err(parse_error(
                            path,
                            line_number,
                            &format!("unknown [Trigger] directive '{key}'"),
                        ));
                    }
                }
            }
            Section::Action => {
                let action = action.as_mut().expect("action section has a builder");
                match key {
                    "AbortOnFail" => action.abort_on_fail = true,
                    "NeedsTargets" => action.needs_targets = true,
                    "Description" => {
                        action.description = Some(required_value(path, line_number, key, value)?);
                    }
                    "When" => {
                        action.when = Some(
                            match required_value(path, line_number, key, value)?.as_str() {
                                "PreTransaction" => NativeTransactionPosition::BeforeTransaction,
                                "PostTransaction" => NativeTransactionPosition::AfterTransaction,
                                value => {
                                    return Err(parse_error(
                                        path,
                                        line_number,
                                        &format!("unsupported When '{value}'"),
                                    ));
                                }
                            },
                        );
                    }
                    "Exec" => {
                        action.exec = Some(required_value(path, line_number, key, value)?);
                    }
                    "Depends" => {
                        action
                            .depends
                            .push(required_value(path, line_number, key, value)?);
                    }
                    _ => {
                        return Err(parse_error(
                            path,
                            line_number,
                            &format!("unknown [Action] directive '{key}'"),
                        ));
                    }
                }
            }
        }
    }

    if let Some(trigger) = current_trigger {
        triggers.push(trigger.finish(path, last_line)?);
    }
    if triggers.is_empty() {
        return Err(parse_error(path, last_line, "hook requires [Trigger]"));
    }
    let action = action
        .ok_or_else(|| parse_error(path, last_line, "hook requires [Action]"))?
        .finish(path, last_line)?;

    Ok(ArchAlpmHookMetadata {
        hook_path: path.to_string(),
        triggers,
        action,
    })
}

fn required_value(path: &str, line: usize, key: &str, value: Option<&str>) -> Result<String> {
    value
        .map(str::to_string)
        .ok_or_else(|| parse_error(path, line, &format!("{key} requires Key = Value syntax")))
}

/// Current libalpm `wordsplit()` semantics.
///
/// This deliberately is not shell parsing: only matching quote bytes group a
/// word, and a backslash is special only immediately before a quote.
pub(crate) fn split_alpm_words(input: &str) -> Option<Vec<String>> {
    let bytes = input.as_bytes();
    let mut words = Vec::new();
    let mut cursor = 0;

    while cursor < bytes.len() {
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor == bytes.len() {
            break;
        }

        let mut word = Vec::new();
        while cursor < bytes.len() && !bytes[cursor].is_ascii_whitespace() {
            match bytes[cursor] {
                quote @ (b'\'' | b'"') => {
                    cursor += 1;
                    loop {
                        if cursor == bytes.len() {
                            return None;
                        }
                        if bytes[cursor] == quote {
                            cursor += 1;
                            break;
                        }
                        if bytes[cursor] == b'\\'
                            && cursor + 1 < bytes.len()
                            && bytes[cursor + 1] == quote
                        {
                            cursor += 1;
                        }
                        word.push(bytes[cursor]);
                        cursor += 1;
                    }
                }
                b'\\' if cursor + 1 < bytes.len() && matches!(bytes[cursor + 1], b'\'' | b'"') => {
                    cursor += 1;
                    word.push(bytes[cursor]);
                    cursor += 1;
                }
                byte => {
                    word.push(byte);
                    cursor += 1;
                }
            }
        }
        words.push(String::from_utf8(word).expect("removing ASCII quotes preserves UTF-8"));
    }

    Some(words)
}

/// Match libalpm's ordered Target list exactly.
///
/// `_alpm_fnmatch_patterns` scans from the last Target to the first, strips a
/// leading `!` for inversion or a leading `\` for an escaped literal `!`, and
/// calls POSIX `fnmatch(3)` with flags `0`.
pub(crate) fn targets_match(targets: &[String], candidate: &str) -> Result<bool> {
    for target in targets.iter().rev() {
        let (inverted, pattern) = if let Some(pattern) = target.strip_prefix('!') {
            (true, pattern)
        } else if let Some(pattern) = target.strip_prefix('\\') {
            (false, pattern)
        } else {
            (false, target.as_str())
        };
        if posix_fnmatch(pattern, candidate)? {
            return Ok(!inverted);
        }
    }
    Ok(false)
}

fn posix_fnmatch(pattern: &str, candidate: &str) -> Result<bool> {
    use std::ffi::CString;

    let pattern = CString::new(pattern)
        .map_err(|_| Error::InitError("ALPM Target pattern contains NUL".to_string()))?;
    let candidate = CString::new(candidate)
        .map_err(|_| Error::InitError("ALPM Target candidate contains NUL".to_string()))?;
    // SAFETY: both arguments are live NUL-terminated C strings and libalpm
    // calls the same libc interface with flags 0.
    Ok(unsafe { libc::fnmatch(pattern.as_ptr(), candidate.as_ptr(), 0) } == 0)
}

pub(super) fn is_package_hook_path(path: &str) -> bool {
    package_hook_basename_value(path).is_some()
}

pub(crate) fn package_hook_basename(path: &str) -> Result<&str> {
    package_hook_basename_value(path).ok_or_else(|| {
        parse_error(
            path,
            0,
            "package hook path must be /usr/share/libalpm/hooks/<basename>.hook",
        )
    })
}

fn package_hook_basename_value(path: &str) -> Option<&str> {
    const PREFIX: &str = "/usr/share/libalpm/hooks/";

    path.strip_prefix(PREFIX)
        .filter(|basename| !basename.contains('/'))
        .and_then(|basename| basename.strip_suffix(".hook"))
        .filter(|basename| !basename.is_empty())
}

fn parse_error(path: &str, line: usize, message: &str) -> Error {
    let location = if line == 0 {
        path.to_string()
    } else {
        format!("{path}:{line}")
    };
    Error::InitError(format!("Invalid ALPM hook {location}: {message}"))
}

#[cfg(test)]
mod tests {
    use super::{package_hook_basename, parse, split_alpm_words, targets_match};
    use crate::packages::traits::{
        ArchAlpmHookOperation, ArchAlpmHookTriggerType, NativeTransactionPosition,
    };

    #[test]
    fn parses_complete_current_alpm_hook_grammar() {
        let metadata = parse(
            "/usr/share/libalpm/hooks/90-systemd.hook",
            br#"
[Trigger]
Operation = Install
Operation = Upgrade
Type = Path
Target = usr/lib/systemd/system/*
Target = !usr/lib/systemd/system/example.service

[Action]
Description = Reload system manager
When = PostTransaction
Exec = /usr/bin/systemctl daemon-reload
Depends = systemd
NeedsTargets
"#,
        )
        .unwrap();

        assert_eq!(metadata.triggers.len(), 1);
        assert_eq!(
            metadata.triggers[0].operations,
            vec![
                ArchAlpmHookOperation::Install,
                ArchAlpmHookOperation::Upgrade
            ]
        );
        assert_eq!(
            metadata.triggers[0].trigger_type,
            ArchAlpmHookTriggerType::Path
        );
        assert_eq!(
            metadata.action.when,
            NativeTransactionPosition::AfterTransaction
        );
        assert!(metadata.action.needs_targets);
    }

    #[test]
    fn rejects_unknown_directive_instead_of_silently_dropping_it() {
        let error = parse(
            "/usr/share/libalpm/hooks/bad.hook",
            b"[Trigger]\nOperation=Install\nType=Package\nTarget=*\nBogus=yes\n\
              [Action]\nWhen=PostTransaction\nExec=/bin/true\n",
        )
        .unwrap_err();

        assert!(error.to_string().contains("unknown [Trigger] directive"));
    }

    #[test]
    fn rejects_incomplete_action_or_unbalanced_exec_contract() {
        for body in [
            "[Trigger]\nOperation=Install\nType=Package\nTarget=*\n",
            "[Trigger]\nOperation=Install\nType=Package\nTarget=*\n\
             [Action]\nWhen=PostTransaction\nExec=/bin/tool \"unterminated\n",
        ] {
            assert!(
                parse("/usr/share/libalpm/hooks/bad.hook", body.as_bytes()).is_err(),
                "{body}"
            );
        }
    }

    #[test]
    fn follows_libalpm_ini_overwrite_comment_and_wordsplit_semantics() {
        let metadata = parse(
            "/usr/share/libalpm/hooks/grammar.hook",
            br#"
[Action]
Description = replaced
Description = literal # description
When = PreTransaction
When = PostTransaction
Exec = /bin/false
Exec = /usr/bin/tool "two words" slash\ space \"quoted\" 'single word'
AbortOnFail
AbortOnFail = ignored

[Trigger]
Operation = Install
Operation = Install
Type = Package
Type = Path
Target = literal#target
"#,
        )
        .unwrap();

        assert_eq!(
            metadata.action.description.as_deref(),
            Some("literal # description")
        );
        assert_eq!(
            metadata.action.when,
            NativeTransactionPosition::AfterTransaction
        );
        assert!(metadata.action.abort_on_fail);
        assert_eq!(
            metadata.action.argv,
            vec![
                "/usr/bin/tool",
                "two words",
                r"slash\",
                "space",
                "\"quoted\"",
                "single word",
            ]
        );
        assert_eq!(
            metadata.triggers[0].operations,
            vec![ArchAlpmHookOperation::Install]
        );
        assert_eq!(
            metadata.triggers[0].trigger_type,
            ArchAlpmHookTriggerType::Path
        );
        assert_eq!(metadata.triggers[0].targets, vec!["literal#target"]);
    }

    #[test]
    fn libalpm_wordsplit_only_treats_backslash_as_special_before_quotes() {
        assert_eq!(
            split_alpm_words(r#"/bin/tool a\ b \"quoted\" 'x\'y' """#).unwrap(),
            vec!["/bin/tool", r"a\", "b", "\"quoted\"", "x'y", ""]
        );
        assert!(split_alpm_words(r#"/bin/tool "unterminated"#).is_none());
    }

    #[test]
    fn target_matching_uses_posix_fnmatch_and_last_matching_target() {
        let targets = vec![
            "*".to_string(),
            "!private/*".to_string(),
            "private/allow[[:digit:]]***".to_string(),
        ];
        assert!(targets_match(&targets, "private/allow7").unwrap());
        assert!(!targets_match(&targets, "private/deny").unwrap());
        assert!(targets_match(&[r"\!literal".to_string()], "!literal").unwrap());
        assert!(targets_match(&[r"literal\*".to_string()], "literal*").unwrap());
        assert!(targets_match(&["literal#target".to_string()], "literal#target").unwrap());
    }

    #[test]
    fn package_hook_path_is_one_immediate_system_hook_basename() {
        assert_eq!(
            package_hook_basename("/usr/share/libalpm/hooks/90-cache.hook").unwrap(),
            "90-cache"
        );
        for path in [
            "usr/share/libalpm/hooks/90-cache.hook",
            "/etc/pacman.d/hooks/90-cache.hook",
            "/usr/share/libalpm/hooks/nested/90-cache.hook",
            "/usr/share/libalpm/hooks/.hook",
        ] {
            assert!(package_hook_basename(path).is_err(), "{path}");
        }
    }

    #[test]
    fn current_libalpm_rejects_removed_file_trigger_type_alias() {
        let error = parse(
            "/usr/share/libalpm/hooks/file.hook",
            b"[Trigger]\nOperation=Install\nType=File\nTarget=*\n\
              [Action]\nWhen=PostTransaction\nExec=/bin/true\n",
        )
        .unwrap_err();
        assert!(error.to_string().contains("unsupported Type 'File'"));
    }
}
