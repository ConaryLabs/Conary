// conary-core/src/packages/arch/install_script.rs

use crate::packages::traits::NativeLifecyclePath;

pub(super) const LIFECYCLE_FUNCTIONS: &[(&str, NativeLifecyclePath)] = &[
    ("pre_install", NativeLifecyclePath::PreInstall),
    ("post_install", NativeLifecyclePath::PostInstall),
    ("pre_upgrade", NativeLifecyclePath::PreUpgrade),
    ("post_upgrade", NativeLifecyclePath::PostUpgrade),
    ("pre_remove", NativeLifecyclePath::PreRemove),
    ("post_remove", NativeLifecyclePath::PostRemove),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InstallFunction {
    pub name: &'static str,
    pub lifecycle: NativeLifecyclePath,
}

/// Reproduce libalpm's `_alpm_runscriptlet` selection predicate.
///
/// libalpm reads at most 1023 bytes per `fgets` call, truncates each chunk at
/// the first NUL and then at the first `#`, and tests the remainder for the
/// lifecycle function name as a literal byte substring. This is source package
/// manager ABI, not Conary script classification: changing it would select a
/// different set of lifecycle calls than pacman.
pub(super) fn parse(bytes: &[u8]) -> Vec<InstallFunction> {
    LIFECYCLE_FUNCTIONS
        .iter()
        .filter(|(name, _)| libalpm_grep(bytes, name.as_bytes()))
        .map(|(name, lifecycle)| InstallFunction {
            name,
            lifecycle: *lifecycle,
        })
        .collect()
}

fn libalpm_grep(bytes: &[u8], needle: &[u8]) -> bool {
    const FGETS_PAYLOAD_LIMIT: usize = 1023;

    let mut cursor = 0;
    while cursor < bytes.len() {
        let bounded_end = (cursor + FGETS_PAYLOAD_LIMIT).min(bytes.len());
        let end = bytes[cursor..bounded_end]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(bounded_end, |newline| cursor + newline + 1);
        let chunk = &bytes[cursor..end];
        cursor = end;

        let c_string_end = chunk
            .iter()
            .position(|byte| *byte == b'\0')
            .unwrap_or(chunk.len());
        let c_string = &chunk[..c_string_end];
        let uncommented_end = c_string
            .iter()
            .position(|byte| *byte == b'#')
            .unwrap_or(c_string.len());
        if c_string[..uncommented_end]
            .windows(needle.len())
            .any(|window| window == needle)
        {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::{libalpm_grep, parse};

    #[test]
    fn selects_lifecycle_functions_with_libalpm_predicate() {
        let functions = parse(
            br#"
                # braces in comments do not affect the syntax tree: }
                pre_install () {
                    printf '%s\n' "$1"
                }
                function post_install {
                    if true; then
                        echo "nested { brace }"
                    fi
                }
            "#,
        );

        assert_eq!(
            functions
                .iter()
                .map(|function| function.name)
                .collect::<Vec<_>>(),
            vec!["pre_install", "post_install"]
        );
    }

    #[test]
    fn ignores_function_names_after_comment_or_nul_boundaries() {
        assert!(parse(b"# post_install() { :; }\n").is_empty());
        assert!(parse(b"echo ok\0post_install() { :; }\n").is_empty());
    }

    #[test]
    fn preserves_libalpm_1023_byte_chunk_boundary() {
        let mut source = vec![b'x'; 1015];
        source.extend_from_slice(b"post_ins");
        source.extend_from_slice(b"tall() { :; }\n");

        assert!(!libalpm_grep(&source, b"post_install"));
        assert!(parse(&source).is_empty());
    }

    #[test]
    fn selects_dynamic_definition_when_libalpm_would_select_it() {
        let functions = parse(b"eval 'post_install() { :; }'\n");
        assert_eq!(functions.len(), 1);
        assert_eq!(functions[0].name, "post_install");
    }
}
