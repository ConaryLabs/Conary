// conary-core/src/util.rs

//! General-purpose utility functions shared across conary-core modules.

/// Format a byte count as a human-readable size string.
///
/// Uses binary (1024-based) prefixes. Values below 1 KiB are rendered as
/// `"N B"`. Larger values are rendered with two decimal places (e.g.
/// `"1.50 KB"`, `"700.00 GB"`). Supports up to TB.
///
/// # Examples
///
/// ```
/// use conary_core::util::format_bytes;
/// assert_eq!(format_bytes(512),  "512 B");
/// assert_eq!(format_bytes(1024), "1.00 KB");
/// assert_eq!(format_bytes(1_048_576), "1.00 MB");
/// ```
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.2} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Format a signed byte count as a human-readable size string.
///
/// Thin wrapper around [`format_bytes`] that accepts `i64` (as used in
/// database models). Negative values are treated as zero.
///
/// # Examples
///
/// ```
/// use conary_core::util::format_size;
/// assert_eq!(format_size(1024), "1.00 KB");
/// assert_eq!(format_size(500),  "500 B");
/// ```
pub fn format_size(bytes: i64) -> String {
    format_bytes(bytes.max(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bytes_below_kb() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1023), "1023 B");
    }

    #[test]
    fn test_format_bytes_kilobytes() {
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1536), "1.50 KB");
        assert_eq!(format_bytes(2048), "2.00 KB");
    }

    #[test]
    fn test_format_bytes_megabytes() {
        assert_eq!(format_bytes(1024 * 1024), "1.00 MB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.00 MB");
    }

    #[test]
    fn test_format_bytes_gigabytes() {
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.00 GB");
        assert_eq!(format_bytes(700 * 1024 * 1024 * 1024), "700.00 GB");
        assert_eq!(format_bytes(2_684_354_560), "2.50 GB");
    }

    #[test]
    fn test_format_bytes_terabytes() {
        assert_eq!(format_bytes(1024 * 1024 * 1024 * 1024), "1.00 TB");
    }

    #[test]
    fn test_format_size_delegates() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(500), "500 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1024), "1.00 KB");
        assert_eq!(format_size(1024 * 1024), "1.00 MB");
        assert_eq!(format_size(1024 * 1024 * 1024), "1.00 GB");
    }
}
