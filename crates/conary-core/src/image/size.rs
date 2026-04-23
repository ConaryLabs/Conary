// crates/conary-core/src/image/size.rs

use std::str::FromStr;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ImageSizeParseError {
    #[error("invalid image size: empty size")]
    Empty,

    #[error("invalid image size: {0}")]
    Invalid(String),

    #[error("image size overflows u64: {0}")]
    Overflow(String),
}

/// Image size in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageSize(pub u64);

impl FromStr for ImageSize {
    type Err = ImageSizeParseError;

    /// Parse size from string (e.g., "4G", "512M", "8192").
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            return Err(ImageSizeParseError::Empty);
        }

        let (num_str, multiplier) = if let Some(n) = s.strip_suffix(['G', 'g']) {
            (n, 1024 * 1024 * 1024u64)
        } else if let Some(n) = s.strip_suffix(['M', 'm']) {
            (n, 1024 * 1024u64)
        } else if let Some(n) = s.strip_suffix(['K', 'k']) {
            (n, 1024u64)
        } else if let Some(n) = s.strip_suffix(['T', 't']) {
            (n, 1024 * 1024 * 1024 * 1024u64)
        } else {
            (s, 1u64)
        };

        let num: u64 = num_str
            .trim()
            .parse()
            .map_err(|_| ImageSizeParseError::Invalid(s.to_string()))?;
        let bytes = num
            .checked_mul(multiplier)
            .ok_or_else(|| ImageSizeParseError::Overflow(s.to_string()))?;

        Ok(Self(bytes))
    }
}

impl ImageSize {
    /// Get size in bytes.
    pub fn bytes(&self) -> u64 {
        self.0
    }

    /// Get size in megabytes.
    pub fn megabytes(&self) -> u64 {
        self.0 / (1024 * 1024)
    }

    /// Get size in gigabytes.
    pub fn gigabytes(&self) -> u64 {
        self.0 / (1024 * 1024 * 1024)
    }
}

impl std::fmt::Display for ImageSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0 >= 1024 * 1024 * 1024 {
            write!(f, "{}G", self.gigabytes())
        } else if self.0 >= 1024 * 1024 {
            write!(f, "{}M", self.megabytes())
        } else {
            write!(f, "{}", self.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bootstrap_size_syntax() {
        assert_eq!(ImageSize::from_str("4G").unwrap().gigabytes(), 4);
        assert_eq!(ImageSize::from_str("512M").unwrap().megabytes(), 512);
        assert_eq!(ImageSize::from_str("1024K").unwrap().bytes(), 1024 * 1024);
        assert_eq!(ImageSize::from_str("1T").unwrap().gigabytes(), 1024);
        assert_eq!(ImageSize::from_str("1048576").unwrap().bytes(), 1048576);
        assert!(ImageSize::from_str("").is_err());
        assert!(ImageSize::from_str("abc").is_err());
    }

    #[test]
    fn display_uses_largest_whole_unit() {
        assert_eq!(ImageSize::from_str("4G").unwrap().to_string(), "4G");
        assert_eq!(ImageSize::from_str("512M").unwrap().to_string(), "512M");
    }
}
