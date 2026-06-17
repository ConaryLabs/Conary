// conary-core/src/ccs/v2/legacy.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestFormatClassification {
    V2Native,
    LegacyV1,
    Unknown,
}

pub fn classify_manifest_format(
    format_version: Option<u64>,
) -> ManifestFormatClassification {
    match format_version {
        Some(2) => ManifestFormatClassification::V2Native,
        Some(1) => ManifestFormatClassification::LegacyV1,
        _ => ManifestFormatClassification::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_v2_v1_and_unknown_formats() {
        assert_eq!(
            classify_manifest_format(Some(2)),
            ManifestFormatClassification::V2Native
        );
        assert_eq!(
            classify_manifest_format(Some(1)),
            ManifestFormatClassification::LegacyV1
        );
        assert_eq!(
            classify_manifest_format(Some(0)),
            ManifestFormatClassification::Unknown
        );
        assert_eq!(
            classify_manifest_format(None),
            ManifestFormatClassification::Unknown
        );
    }
}
