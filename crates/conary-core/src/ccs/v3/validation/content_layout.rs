// conary-core/src/ccs/v3/validation/content_layout.rs

//! Signed regular-file storage and reconstruction authority.

use crate::ccs::archive_layout::is_lower_hex;
use crate::ccs::chunking::{AVG_CHUNK_SIZE, MAX_CHUNK_SIZE, MIN_CHUNK_SIZE};
use crate::ccs::v3::diagnostics::{V3Diagnostic, V3DiagnosticCode};
use crate::ccs::v3::schema::{FileAuthorityV3, FileContentLayoutV3};

pub(super) fn validate_file_content_layout(
    file: &FileAuthorityV3,
    diagnostics: &mut Vec<V3Diagnostic>,
) {
    let field = "kind.package.files.content_layout";
    let mut invalid = |message: String| {
        diagnostics.push(V3Diagnostic::error(
            V3DiagnosticCode::KindContractViolation,
            format!("file {} has invalid content layout: {message}", file.path),
            Some(field.to_string()),
            "use no-content for non-regular nodes, whole-object for unchunked regular files, or the canonical FastCDC v2020 layout",
        ));
    };

    if !file.node.kind.is_regular() {
        if file.content_layout != FileContentLayoutV3::NoContent {
            invalid("a non-regular node declares payload object authority".to_string());
        }
        return;
    }

    let Some(content) = file.content.as_ref() else {
        invalid("a regular node has no whole-file content authority".to_string());
        return;
    };

    match &file.content_layout {
        FileContentLayoutV3::NoContent => {
            invalid("a regular node declares no content storage".to_string());
        }
        FileContentLayoutV3::WholeObject => {}
        FileContentLayoutV3::FastCdcV2020 {
            min_size,
            average_size,
            max_size,
            chunks,
        } => {
            if (*min_size, *average_size, *max_size)
                != (MIN_CHUNK_SIZE, AVG_CHUNK_SIZE, MAX_CHUNK_SIZE)
            {
                invalid(format!(
                    "unknown FastCDC v2020 profile {min_size}/{average_size}/{max_size}"
                ));
            }
            if content.size < u64::from(MIN_CHUNK_SIZE) {
                invalid(format!(
                    "{} bytes is below the canonical chunking threshold {MIN_CHUNK_SIZE}",
                    content.size
                ));
            }
            if chunks.is_empty() {
                invalid("a chunked file declares no chunks".to_string());
                return;
            }

            let mut total = 0_u64;
            for (index, chunk) in chunks.iter().enumerate() {
                if chunk.sha256.len() != 64 || !is_lower_hex(&chunk.sha256) {
                    invalid(format!(
                        "chunk {index} digest {:?} is not canonical lowercase SHA-256",
                        chunk.sha256
                    ));
                }
                if chunk.size == 0 || chunk.size > MAX_CHUNK_SIZE {
                    invalid(format!(
                        "chunk {index} size {} is outside 1..={MAX_CHUNK_SIZE}",
                        chunk.size
                    ));
                }
                if index + 1 != chunks.len() && chunk.size < MIN_CHUNK_SIZE {
                    invalid(format!(
                        "non-final chunk {index} size {} is below {MIN_CHUNK_SIZE}",
                        chunk.size
                    ));
                }
                let Some(next) = total.checked_add(u64::from(chunk.size)) else {
                    invalid("chunk-size total overflows u64".to_string());
                    return;
                };
                total = next;
            }
            if total != content.size {
                invalid(format!(
                    "ordered chunks total {total} bytes but whole-file authority requires {}",
                    content.size
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ccs::v3::schema::PackageKindV3;

    fn authority_with_layout(layout: FileContentLayoutV3) -> crate::ccs::v3::AuthorityDocumentV3 {
        let mut authority =
            crate::ccs::v3::AuthorityDocumentV3::package_for_tests("content-layout");
        let PackageKindV3::Package(package) = &mut authority.kind else {
            unreachable!()
        };
        package.files[0].content_layout = layout;
        authority
    }

    fn error_text(authority: &crate::ccs::v3::AuthorityDocumentV3) -> String {
        crate::ccs::v3::validate_authority(authority)
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn current_schema_requires_explicit_content_layout_authority() {
        let authority = crate::ccs::v3::AuthorityDocumentV3::package_for_tests("content-layout");
        let mut json = serde_json::to_value(authority).unwrap();
        json.pointer_mut("/kind/data/files/0")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("content_layout");
        let error = serde_json::from_value::<crate::ccs::v3::AuthorityDocumentV3>(json)
            .unwrap_err()
            .to_string();
        assert!(error.contains("content_layout"), "{error}");
    }

    #[test]
    fn explicit_whole_object_layout_remains_valid_above_the_default_chunk_threshold() {
        let mut authority = authority_with_layout(FileContentLayoutV3::WholeObject);
        let PackageKindV3::Package(package) = &mut authority.kind else {
            unreachable!()
        };
        package.files[0].content.as_mut().unwrap().size = u64::from(MIN_CHUNK_SIZE);
        let mut diagnostics = Vec::new();
        validate_file_content_layout(&package.files[0], &mut diagnostics);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn unknown_fastcdc_profile_fails_closed() {
        let authority = authority_with_layout(FileContentLayoutV3::FastCdcV2020 {
            min_size: MIN_CHUNK_SIZE,
            average_size: AVG_CHUNK_SIZE + 1,
            max_size: MAX_CHUNK_SIZE,
            chunks: vec![crate::ccs::chunking::ChunkReference {
                sha256: crate::hash::sha256(b"hello world\n"),
                size: 12,
            }],
        });
        assert!(error_text(&authority).contains("unknown FastCDC v2020 profile"));
    }

    #[test]
    fn layout_kind_and_ordered_size_authority_must_match_the_file() {
        let mut non_regular = authority_with_layout(FileContentLayoutV3::WholeObject);
        let PackageKindV3::Package(package) = &mut non_regular.kind else {
            unreachable!()
        };
        package.files[0].node.kind = crate::payload::PayloadNodeKind::Directory;
        package.files[0].node.mode = libc::S_IFDIR | 0o755;
        package.files[0].content = None;
        assert!(error_text(&non_regular).contains("non-regular node"));

        let chunk = vec![b'x'; MIN_CHUNK_SIZE as usize];
        let mut wrong_total = authority_with_layout(FileContentLayoutV3::FastCdcV2020 {
            min_size: MIN_CHUNK_SIZE,
            average_size: AVG_CHUNK_SIZE,
            max_size: MAX_CHUNK_SIZE,
            chunks: vec![crate::ccs::chunking::ChunkReference {
                sha256: crate::hash::sha256(&chunk),
                size: MIN_CHUNK_SIZE,
            }],
        });
        let PackageKindV3::Package(package) = &mut wrong_total.kind else {
            unreachable!()
        };
        package.files[0].content.as_mut().unwrap().size = u64::from(MIN_CHUNK_SIZE) + 1;
        wrong_total.components.get_mut("main").unwrap().total_size = u64::from(MIN_CHUNK_SIZE) + 1;
        assert!(error_text(&wrong_total).contains("ordered chunks total"));
    }
}
