// conary-core/src/ccs/v2/test_support.rs

use super::schema::*;
use std::collections::BTreeMap;

pub(crate) fn package_authority_with_one_file(name: &str) -> AuthorityDocumentV2 {
    AuthorityDocumentV2::package_for_tests(name)
}

pub(crate) fn one_file_payloads_for_tests() -> BTreeMap<String, Vec<u8>> {
    BTreeMap::from([("/usr/bin/hello".to_string(), b"hello world\n".to_vec())])
}
