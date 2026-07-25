// conary-core/src/packages/deb/triggers.rs

//! Strict parser for the current `deb-triggers(5)` control-file grammar.

use crate::error::{Error, Result};
use crate::packages::traits::{DebTriggerAwaitMode, DebTriggerDeclaration, DebTriggerDirective};

pub(crate) fn parse(bytes: &[u8]) -> Result<Vec<DebTriggerDeclaration>> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| Error::InitError(format!("Invalid DEBIAN/triggers UTF-8: {error}")))?;
    let mut declarations = Vec::new();

    for (index, raw_line) in text.lines().enumerate() {
        let line_number = index + 1;
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let parts = line.split_whitespace().collect::<Vec<_>>();
        if parts.len() != 2 {
            return Err(parse_error(
                line_number,
                "expected exactly '<directive> <trigger-name>'",
            ));
        }
        let (directive, await_mode) = match parts[0] {
            "interest" => (DebTriggerDirective::Interest, DebTriggerAwaitMode::Default),
            "interest-await" => (DebTriggerDirective::Interest, DebTriggerAwaitMode::Await),
            "interest-noawait" => (DebTriggerDirective::Interest, DebTriggerAwaitMode::NoAwait),
            "activate" => (DebTriggerDirective::Activate, DebTriggerAwaitMode::Default),
            "activate-await" => (DebTriggerDirective::Activate, DebTriggerAwaitMode::Await),
            "activate-noawait" => (DebTriggerDirective::Activate, DebTriggerAwaitMode::NoAwait),
            unknown => {
                return Err(parse_error(
                    line_number,
                    &format!("unknown directive '{unknown}'"),
                ));
            }
        };
        let trigger_name = parts[1];
        validate_trigger_name(trigger_name)
            .map_err(|message| parse_error(line_number, &message))?;

        declarations.push(DebTriggerDeclaration {
            directive,
            trigger_name: trigger_name.to_string(),
            await_mode,
            raw_line: raw_line.to_string(),
        });
    }

    Ok(declarations)
}

fn validate_trigger_name(trigger_name: &str) -> std::result::Result<(), String> {
    if trigger_name.is_empty() || !trigger_name.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(format!(
            "trigger name '{trigger_name}' contains a non-printing or non-ASCII character"
        ));
    }

    Ok(())
}

fn parse_error(line: usize, message: &str) -> Error {
    Error::InitError(format!("Invalid DEBIAN/triggers:{line}: {message}"))
}

#[cfg(test)]
mod tests {
    use super::parse;
    use crate::packages::traits::{DebTriggerAwaitMode, DebTriggerDirective};

    #[test]
    fn parses_every_current_deb_trigger_directive() {
        let declarations = parse(
            b"interest cache\ninterest-await icons\ninterest-noawait docs\n\
              activate refresh\nactivate-await schema\nactivate-noawait menus\n",
        )
        .unwrap();

        assert_eq!(declarations.len(), 6);
        assert_eq!(declarations[0].directive, DebTriggerDirective::Interest);
        assert_eq!(declarations[1].await_mode, DebTriggerAwaitMode::Await);
        assert_eq!(declarations[2].await_mode, DebTriggerAwaitMode::NoAwait);
        assert_eq!(declarations[3].directive, DebTriggerDirective::Activate);
    }

    #[test]
    fn rejects_unknown_or_malformed_lines_instead_of_dropping_them() {
        for body in [
            b"future-directive cache\n".as_slice(),
            b"interest\n".as_slice(),
            b"interest cache unexpected\n".as_slice(),
            b"interest caf\xc3\xa9\n".as_slice(),
        ] {
            assert!(parse(body).is_err(), "{body:?}");
        }
    }

    #[test]
    fn accepts_current_explicit_and_file_trigger_name_grammars() {
        let declarations = parse(
            b"# comment-only lines and empty files are valid\n\
              interest update-icon-caches\n\
              interest-await /usr/share/icons\n\
              activate-noawait ldconfig+abi.1\n\
              interest Uppercase\n\
              activate a\n\
              interest /usr//share/icons/\n",
        )
        .unwrap();

        assert_eq!(
            declarations
                .iter()
                .map(|declaration| declaration.trigger_name.as_str())
                .collect::<Vec<_>>(),
            vec![
                "update-icon-caches",
                "/usr/share/icons",
                "ldconfig+abi.1",
                "Uppercase",
                "a",
                "/usr//share/icons/",
            ]
        );
    }

    #[test]
    fn accepts_empty_and_comment_only_control_files() {
        assert!(parse(b"").unwrap().is_empty());
        assert!(
            parse(b"  # no declarations\n\t# still empty\n")
                .unwrap()
                .is_empty()
        );
    }
}
