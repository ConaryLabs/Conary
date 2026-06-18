// apps/remi/src/server/native_publish/test_support.rs
//! Test helpers for native Remi publication.

#[cfg(test)]
pub fn assert_json_code(body: &str, expected: &str) {
    let value: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(value["code"], expected);
}
