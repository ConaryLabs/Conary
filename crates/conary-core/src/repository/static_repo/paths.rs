// conary-core/src/repository/static_repo/paths.rs

use anyhow::{Result, anyhow, bail};

pub fn validate_repo_relative_path(path: &str) -> Result<()> {
    if path.is_empty() {
        bail!("repo-relative path must not be empty");
    }

    if path.starts_with('/') || path.starts_with('\\') {
        bail!("repo-relative path must not be absolute");
    }

    if has_url_scheme(path) {
        bail!("repo-relative path must not contain a URL scheme");
    }

    validate_percent_encoding(path)?;

    let decoded = urlencoding::decode(path).map_err(|error| {
        anyhow!("repo-relative path is not valid UTF-8 after decoding: {error}")
    })?;
    validate_decoded_path_characters(&decoded)?;

    if decoded.contains('\\') {
        bail!("repo-relative path must not contain backslash separators");
    }

    for component in decoded.split('/') {
        if component.is_empty() {
            bail!("repo-relative path must not contain empty components");
        }

        if component == "." || component == ".." {
            bail!("repo-relative path must not contain dot or dot-dot components");
        }
    }

    Ok(())
}

fn validate_decoded_path_characters(path: &str) -> Result<()> {
    for byte in path.bytes() {
        if byte == b'?' || byte == b'#' {
            bail!("repo-relative path must not contain URL delimiter characters");
        }

        if byte <= 0x1f || byte == 0x7f {
            bail!("repo-relative path must not contain ASCII control characters");
        }
    }

    Ok(())
}

fn validate_percent_encoding(path: &str) -> Result<()> {
    let bytes = path.as_bytes();
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] != b'%' {
            index += 1;
            continue;
        }

        if index + 2 >= bytes.len() {
            bail!("repo-relative path contains incomplete percent encoding");
        }

        let high = hex_value(bytes[index + 1])
            .ok_or_else(|| anyhow!("repo-relative path contains invalid percent encoding"))?;
        let low = hex_value(bytes[index + 2])
            .ok_or_else(|| anyhow!("repo-relative path contains invalid percent encoding"))?;
        let decoded = (high << 4) | low;

        if decoded == b'/' || decoded == b'\\' {
            bail!("repo-relative path must not contain percent-encoded separators");
        }

        index += 3;
    }

    Ok(())
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn has_url_scheme(path: &str) -> bool {
    let Some(colon_index) = path.find(':') else {
        return false;
    };

    let scheme = &path[..colon_index];
    let mut bytes = scheme.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };

    first.is_ascii_alphabetic()
        && bytes.all(|byte| {
            matches!(
                byte,
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'+' | b'-' | b'.'
            )
        })
}

#[cfg(test)]
mod tests {
    use super::validate_repo_relative_path;

    #[test]
    fn target_path_rejects_percent_encoded_traversal() {
        assert!(validate_repo_relative_path("packages/%2e%2e/evil.ccs").is_err());
        assert!(validate_repo_relative_path("packages/foo%2fbar.ccs").is_err());
        assert!(validate_repo_relative_path("/packages/rooted.ccs").is_err());
        assert!(validate_repo_relative_path("https://example.test/pkg.ccs").is_err());
        assert!(validate_repo_relative_path("packages/foo..bar/pkg.ccs").is_ok());
    }

    #[test]
    fn target_path_rejects_dot_and_empty_components() {
        assert!(validate_repo_relative_path("").is_err());
        assert!(validate_repo_relative_path("packages//pkg.ccs").is_err());
        assert!(validate_repo_relative_path("packages/./pkg.ccs").is_err());
        assert!(validate_repo_relative_path("packages/foo%5cbar.ccs").is_err());
    }

    #[test]
    fn target_path_rejects_url_delimiters_and_control_characters() {
        assert!(validate_repo_relative_path("packages/pkg.ccs?download=1").is_err());
        assert!(validate_repo_relative_path("packages/pkg.ccs#sha256").is_err());
        assert!(validate_repo_relative_path("packages/pkg%3fdownload.ccs").is_err());
        assert!(validate_repo_relative_path("packages/pkg%23sha256.ccs").is_err());
        assert!(validate_repo_relative_path("packages/pkg\u{1f}.ccs").is_err());
        assert!(validate_repo_relative_path("packages/pkg%1f.ccs").is_err());
        assert!(validate_repo_relative_path("packages/foo..bar/pkg.ccs").is_ok());
    }
}
