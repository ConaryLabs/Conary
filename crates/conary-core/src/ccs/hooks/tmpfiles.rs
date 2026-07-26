// conary-core/src/ccs/hooks/tmpfiles.rs

//! Tmpfiles integration for CCS hooks
//!
//! Persists and applies tmpfiles.d entries inside a materialized selected root.

use super::HookExecutor;
use anyhow::{Context, Result};
use std::fs;
use std::process::Command;
use tracing::debug;

pub(crate) fn validate_tmpfiles_entry_type(entry_type: &str) -> Result<()> {
    let mut characters = entry_type.chars();
    let Some(base_type) = characters.next() else {
        return Err(anyhow::anyhow!("tmpfiles entry type must not be empty"));
    };
    if !base_type.is_ascii_alphabetic() {
        return Err(anyhow::anyhow!(
            "tmpfiles entry type must start with one ASCII letter"
        ));
    }
    if let Some(modifier) =
        characters.find(|modifier| !matches!(modifier, '+' | '!' | '-' | '=' | '~' | '^' | '$'))
    {
        return Err(anyhow::anyhow!(
            "tmpfiles entry type contains invalid modifier '{modifier}'"
        ));
    }
    Ok(())
}

fn validate_tmpfiles_column(name: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(anyhow::anyhow!("tmpfiles {name} column must not be empty"));
    }

    let mut quote = None;
    let mut escaped = false;
    for character in value.chars() {
        if matches!(character, '\0' | '\n' | '\r') {
            return Err(anyhow::anyhow!(
                "tmpfiles {name} column contains a line terminator or NUL"
            ));
        }
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(delimiter) = quote {
            if character == delimiter {
                quote = None;
            }
            continue;
        }
        if matches!(character, '"' | '\'') {
            quote = Some(character);
        } else if character.is_ascii_whitespace() {
            return Err(anyhow::anyhow!(
                "tmpfiles {name} must encode exactly one field; use quoting or C-style escapes for whitespace"
            ));
        }
    }
    if escaped {
        return Err(anyhow::anyhow!(
            "tmpfiles {name} column ends with an incomplete escape"
        ));
    }
    if quote.is_some() {
        return Err(anyhow::anyhow!(
            "tmpfiles {name} column contains an unterminated quote"
        ));
    }
    Ok(())
}

fn validate_tmpfiles_argument(argument: &str) -> Result<()> {
    if argument.is_empty() {
        return Err(anyhow::anyhow!(
            "tmpfiles argument column must not be empty; use '-' for no argument"
        ));
    }
    if argument
        .chars()
        .any(|character| matches!(character, '\0' | '\n' | '\r'))
    {
        return Err(anyhow::anyhow!(
            "tmpfiles argument column contains a line terminator or NUL"
        ));
    }
    Ok(())
}

pub(crate) fn validate_tmpfiles_fields(
    entry_type: &str,
    path: &str,
    mode: &str,
    user: &str,
    group: &str,
    age: &str,
    argument: &str,
) -> Result<()> {
    validate_tmpfiles_entry_type(entry_type)?;
    validate_tmpfiles_column("path", path)?;
    validate_tmpfiles_column("mode", mode)?;
    validate_tmpfiles_column("user", user)?;
    validate_tmpfiles_column("group", group)?;
    validate_tmpfiles_column("age", age)?;
    validate_tmpfiles_argument(argument)
}

fn render_tmpfiles_entry(entry: &crate::ccs::manifest::TmpfilesHook) -> Result<String> {
    validate_tmpfiles_fields(
        &entry.entry_type,
        &entry.path,
        &entry.mode,
        &entry.user,
        &entry.group,
        &entry.age,
        &entry.argument,
    )?;
    Ok(format!(
        "{} {} {} {} {} {} {}\n",
        entry.entry_type,
        entry.path,
        entry.mode,
        entry.user,
        entry.group,
        entry.age,
        entry.argument
    ))
}

impl HookExecutor {
    /// Apply a tmpfiles entry
    ///
    pub(super) fn apply_tmpfile(&self, entry: &crate::ccs::manifest::TmpfilesHook) -> Result<()> {
        let config = render_tmpfiles_entry(entry)?;
        let tmpfiles_dir = self.root.join("etc/tmpfiles.d");
        fs::create_dir_all(&tmpfiles_dir)?;
        let filename = format!("conary-{:x}.conf", hash_string(&entry.path));
        let config_path = tmpfiles_dir.join(&filename);
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&config_path)
            .with_context(|| format!("Failed to create {}", config_path.display()))?;
        file.write_all(config.as_bytes())?;

        let executable = self.capabilities.tmpfiles_path().ok_or_else(|| {
            anyhow::anyhow!(
                "systemd-tmpfiles command interface is absent from the persisted host capability inventory; run `conary system init`"
            )
        })?;
        let root = self.root.to_str().ok_or_else(|| {
            anyhow::anyhow!(
                "selected root path contains non-UTF-8 characters: {}",
                self.root.display()
            )
        })?;
        let guest_config = format!("/etc/tmpfiles.d/{filename}");
        let status = Command::new(executable)
            .arg(format!("--root={root}"))
            .args(["--create", &guest_config])
            .status()
            .with_context(|| {
                format!(
                    "Failed to run persisted systemd-tmpfiles interface {}",
                    executable.display()
                )
            })?;

        if status.success() {
            debug!(
                "Persisted and applied tmpfiles entry '{}' in selected root {}",
                entry.path,
                self.root.display()
            );
            Ok(())
        } else {
            Err(anyhow::anyhow!("systemd-tmpfiles --create failed"))
        }
    }
}

/// Deterministic string hash for generating unique filenames.
///
/// Uses FNV-1a (64-bit), which is stable across Rust versions and platforms,
/// unlike `DefaultHasher` which may change its algorithm between releases.
pub fn hash_string(s: &str) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0100_0000_01b3;

    let mut hash = FNV_OFFSET_BASIS;
    for byte in s.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::hooks::{HostCapabilityInventory, TmpfilesInterface};
    use crate::ccs::manifest::{Hooks, TmpfilesHook};

    fn entry(entry_type: &str) -> TmpfilesHook {
        TmpfilesHook {
            entry_type: entry_type.to_string(),
            path: "/run/conary example".replace(' ', "\\x20"),
            mode: "~:0640".to_string(),
            user: ":conary".to_string(),
            group: "conary".to_string(),
            age: "am:30d".to_string(),
            argument: "exact argument with spaces".to_string(),
            reversible: Some(true),
        }
    }

    #[test]
    fn test_hash_string() {
        let hash1 = hash_string("/var/lib/test");
        let hash2 = hash_string("/var/lib/test");
        let hash3 = hash_string("/var/lib/other");

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn structural_type_grammar_accepts_native_types_and_documented_modifiers() {
        for entry_type in [
            "f", "L", "C", "w", "x", "f+", "r!", "w-", "d=", "f~^", "C$", "w+!-=~^$",
        ] {
            assert!(
                validate_tmpfiles_entry_type(entry_type).is_ok(),
                "expected {entry_type} to be accepted"
            );
        }
    }

    #[test]
    fn structural_type_grammar_rejects_malformed_tokens() {
        for entry_type in ["", "11", "+f", "dd", "d?", "d ", "d\0", "d\n"] {
            assert!(
                validate_tmpfiles_entry_type(entry_type).is_err(),
                "expected {entry_type:?} to be rejected"
            );
        }
    }

    #[test]
    fn structural_columns_reject_line_injection_and_ambiguous_fields() {
        assert!(
            validate_tmpfiles_fields("d", "/run/a\nw /etc/x", "-", "-", "-", "-", "-").is_err()
        );
        assert!(validate_tmpfiles_fields("d", "/run/two words", "-", "-", "-", "-", "-").is_err());
        assert!(
            validate_tmpfiles_fields("d", "\"/run/two words\"", "-", "-", "-", "-", "-").is_ok()
        );
        assert!(
            validate_tmpfiles_fields("d", "/run/a", "-", "-", "-", "-", "one\nw /etc/x").is_err()
        );
    }

    #[test]
    fn rendering_preserves_all_seven_columns_exactly() {
        let entry = entry("w+!-$");
        assert_eq!(
            render_tmpfiles_entry(&entry).unwrap(),
            "w+!-$ /run/conary\\x20example ~:0640 :conary conary am:30d exact argument with spaces\n"
        );
    }

    #[test]
    fn selected_root_requires_the_typed_tmpfiles_interface() {
        let root = tempfile::tempdir().unwrap();
        let executor = HookExecutor::new(root.path(), HostCapabilityInventory::default());
        let entry = entry("x");
        let mut hooks = Hooks::default();
        hooks.tmpfiles.push(entry);

        assert!(executor.preflight_hooks(&hooks).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn selected_root_persists_and_executes_the_typed_tmpfiles_interface() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("systemd-tmpfiles");
        let captured = directory.path().join("captured.conf");
        fs::write(
            &executable,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'systemd 257'; exit 0; fi\nif [ \"$1\" = \"--dry-run\" ]; then cat >/dev/null; exit 0; fi\ncase \"$1\" in --root=*) root=${{1#--root=}} ;; *) exit 2 ;; esac\n[ \"$2\" = \"--create\" ] || exit 2\ncp \"$root$3\" '{}'\n",
                captured.display()
            ),
        )
        .unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).unwrap();

        let capabilities = HostCapabilityInventory {
            tmpfiles: Some(TmpfilesInterface::probe(executable).unwrap()),
            ..HostCapabilityInventory::default()
        };
        let target = tempfile::tempdir().unwrap();
        let executor = HookExecutor::new(target.path(), capabilities);
        let entry = entry("L+");
        let mut hooks = Hooks::default();
        hooks.tmpfiles.push(entry.clone());

        executor.preflight_hooks(&hooks).unwrap();
        executor.apply_tmpfile(&entry).unwrap();

        assert_eq!(
            fs::read_to_string(captured).unwrap(),
            render_tmpfiles_entry(&entry).unwrap()
        );
        let config_path = target.path().join(format!(
            "etc/tmpfiles.d/conary-{:x}.conf",
            hash_string(&entry.path)
        ));
        assert_eq!(
            fs::read_to_string(config_path).unwrap(),
            render_tmpfiles_entry(&entry).unwrap()
        );
    }
}
