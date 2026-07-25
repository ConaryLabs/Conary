// conary-core/src/scriptlet/rpm_runtime/macro_expand/parameters.rs
//! RPM parametric-macro argument and automatic-macro projection.

use anyhow::{Result, bail};
use std::collections::{BTreeMap, BTreeSet};

const QUOTE_MARKER: char = '\u{001f}';

#[derive(Debug)]
pub(super) struct ParameterContext {
    pub(super) automatic: BTreeMap<String, String>,
    pub(super) arguments: Vec<String>,
    pub(super) options: BTreeMap<String, String>,
}

pub(super) fn split_words(input: &str) -> Result<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut keep_empty = false;
    for character in input.chars() {
        match character {
            QUOTE_MARKER => {
                quoted = !quoted;
                keep_empty = true;
            }
            ' ' | '\t' if !quoted => {
                if !word.is_empty() || keep_empty {
                    words.push(std::mem::take(&mut word));
                    keep_empty = false;
                }
            }
            character => word.push(character),
        }
    }
    if quoted {
        bail!("RpmMacroArgumentError: unterminated quoted macro argument");
    }
    if !word.is_empty() || keep_empty {
        words.push(word);
    }
    Ok(words)
}

pub(super) fn automatic_macros(
    name: &str,
    option_spec: &str,
    arguments: &[String],
) -> Result<ParameterContext> {
    let mut macros = BTreeMap::new();
    let mut option_values = BTreeMap::new();
    macros.insert("0".to_string(), name.to_string());
    macros.insert(
        "**".to_string(),
        arguments
            .iter()
            .map(|argument| format!("{QUOTE_MARKER}{argument}{QUOTE_MARKER}"))
            .collect::<Vec<_>>()
            .join(" "),
    );

    let (known, takes_value, disable_options) = parse_option_spec(option_spec);
    let mut positionals = Vec::new();
    let mut index = 0usize;
    let mut options_done = disable_options;
    while index < arguments.len() {
        let argument = &arguments[index];
        if !options_done && argument == "--" {
            options_done = true;
            index += 1;
            continue;
        }
        if !options_done && argument.starts_with('-') && argument != "-" {
            let mut option_bytes = argument[1..].bytes().peekable();
            while let Some(option) = option_bytes.next() {
                if !known.contains(&option) {
                    bail!(
                        "RpmMacroArgumentError: unknown option -{} in %{name}({option_spec})",
                        char::from(option)
                    );
                }
                let option_name = format!("-{}", char::from(option));
                if takes_value.contains(&option) {
                    let inline = option_bytes.map(char::from).collect::<String>();
                    let value = if inline.is_empty() {
                        index += 1;
                        arguments.get(index).cloned().ok_or_else(|| {
                            anyhow::anyhow!(
                                "RpmMacroArgumentError: option -{} requires an argument",
                                char::from(option)
                            )
                        })?
                    } else {
                        inline
                    };
                    macros.insert(option_name.clone(), format!("{option_name} {value}"));
                    macros.insert(format!("{option_name}*"), value.clone());
                    option_values.insert(char::from(option).to_string(), value);
                    break;
                }
                macros.insert(option_name.clone(), option_name);
                option_values.insert(char::from(option).to_string(), String::new());
            }
        } else {
            positionals.push(argument.clone());
        }
        index += 1;
    }

    macros.insert("#".to_string(), positionals.len().to_string());
    macros.insert("*".to_string(), positionals.join(" "));
    for (index, argument) in positionals.iter().enumerate() {
        macros.insert((index + 1).to_string(), argument.clone());
    }
    Ok(ParameterContext {
        automatic: macros,
        arguments: positionals,
        options: option_values,
    })
}

fn parse_option_spec(spec: &str) -> (BTreeSet<u8>, BTreeSet<u8>, bool) {
    if spec == "-" {
        return (BTreeSet::new(), BTreeSet::new(), true);
    }
    let bytes = spec.as_bytes();
    let mut known = BTreeSet::new();
    let mut takes_value = BTreeSet::new();
    let mut index = 0usize;
    while index < bytes.len() {
        let option = bytes[index];
        if option != b':' {
            known.insert(option);
            if bytes.get(index + 1) == Some(&b':') {
                takes_value.insert(option);
                index += 1;
            }
        }
        index += 1;
    }
    (known, takes_value, false)
}

#[cfg(test)]
mod tests {
    use super::{QUOTE_MARKER, automatic_macros, split_words};

    #[test]
    fn projects_rpm_positional_and_option_automatic_macros() {
        let arguments = split_words("5 a -z -b2").expect("arguments");
        let context = automatic_macros("foo", "zb:", &arguments).expect("automatic macros");
        let macros = context.automatic;
        assert_eq!(macros["0"], "foo");
        assert_eq!(macros["**"].replace(QUOTE_MARKER, ""), "5 a -z -b2");
        assert_eq!(macros["#"], "2");
        assert_eq!(macros["1"], "5");
        assert_eq!(macros["2"], "a");
        assert_eq!(macros["*"], "5 a");
        assert_eq!(macros["-z"], "-z");
        assert_eq!(macros["-b"], "-b 2");
        assert_eq!(macros["-b*"], "2");
        assert_eq!(context.arguments, ["5", "a"]);
        assert_eq!(context.options["z"], "");
        assert_eq!(context.options["b"], "2");
    }

    #[test]
    fn dash_option_spec_keeps_option_like_arguments_positional() {
        let arguments = split_words("5 -z -b2").expect("arguments");
        let context = automatic_macros("foo", "-", &arguments).expect("automatic macros");
        assert_eq!(context.automatic["*"], "5 -z -b2");
        assert_eq!(context.automatic["#"], "3");
        assert_eq!(context.arguments, ["5", "-z", "-b2"]);
        assert!(context.options.is_empty());
    }
}
