// conary-core/src/json.rs

//! Canonical JSON serialization for deterministic cryptographic operations.
//!
//! Provides a single shared implementation of canonical JSON (OLPC-style)
//! used wherever deterministic serialization is required -- TUF key IDs,
//! TUF metadata signatures, and model collection signing.

/// Produce deterministic (canonical) JSON bytes for any serializable value.
///
/// Canonical JSON as defined by OLPC:
/// - Object keys sorted lexicographically
/// - No unnecessary whitespace
/// - No trailing commas
///
/// Used for computing TUF key IDs, signing TUF metadata, and signing
/// model collection data.
///
/// # Errors
///
/// Returns an error string if the value cannot be serialized.
pub fn canonical_json<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, String> {
    let json_value = serde_json::to_value(value)
        .map_err(|e| format!("Failed to serialize to Value: {e}"))?;
    let sorted = sort_json_keys(&json_value);
    serde_json::to_vec(&sorted)
        .map_err(|e| format!("Failed to serialize to Vec: {e}"))
}

/// Recursively sort all JSON object keys for canonical representation.
///
/// Uses `BTreeMap` ordering (lexicographic) to ensure stable key order
/// regardless of original `HashMap`/`IndexMap` iteration order.
fn sort_json_keys(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            // BTreeMap sorts keys lexicographically
            let sorted: serde_json::Map<String, serde_json::Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), sort_json_keys(v)))
                .collect();
            serde_json::Value::Object(sorted)
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(sort_json_keys).collect())
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_canonical_json_deterministic() {
        // Two maps with different insertion orders should produce identical canonical JSON
        let mut map1 = serde_json::Map::new();
        map1.insert("zebra".to_string(), serde_json::Value::from(1));
        map1.insert("apple".to_string(), serde_json::Value::from(2));

        let mut map2 = serde_json::Map::new();
        map2.insert("apple".to_string(), serde_json::Value::from(2));
        map2.insert("zebra".to_string(), serde_json::Value::from(1));

        let val1 = serde_json::Value::Object(map1);
        let val2 = serde_json::Value::Object(map2);

        let c1 = canonical_json(&val1).unwrap();
        let c2 = canonical_json(&val2).unwrap();
        assert_eq!(c1, c2);

        // Keys should be sorted: apple before zebra
        let s = String::from_utf8(c1).unwrap();
        let apple_pos = s.find("apple").unwrap();
        let zebra_pos = s.find("zebra").unwrap();
        assert!(apple_pos < zebra_pos);
    }

    #[test]
    fn test_canonical_json_nested_sorting() {
        let mut inner = serde_json::Map::new();
        inner.insert("z".to_string(), serde_json::Value::from("last"));
        inner.insert("a".to_string(), serde_json::Value::from("first"));

        let mut outer = serde_json::Map::new();
        outer.insert("nested".to_string(), serde_json::Value::Object(inner));
        outer.insert("top".to_string(), serde_json::Value::from(42));

        let val = serde_json::Value::Object(outer);
        let c = canonical_json(&val).unwrap();
        let s = String::from_utf8(c).unwrap();

        // Outer keys sorted: nested before top
        let nested_pos = s.find("nested").unwrap();
        let top_pos = s.find("top").unwrap();
        assert!(nested_pos < top_pos);

        // Inner keys sorted: a before z
        let a_pos = s.find("\"a\"").unwrap();
        let z_pos = s.find("\"z\"").unwrap();
        assert!(a_pos < z_pos);
    }

    #[test]
    fn test_canonical_json_no_whitespace() {
        let mut map = serde_json::Map::new();
        map.insert("key".to_string(), serde_json::Value::from("value"));
        let val = serde_json::Value::Object(map);

        let c = canonical_json(&val).unwrap();
        let s = String::from_utf8(c).unwrap();
        assert_eq!(s, "{\"key\":\"value\"}");
    }

    #[test]
    fn test_canonical_json_array_preserved() {
        let arr = serde_json::json!([3, 1, 2]);
        let c = canonical_json(&arr).unwrap();
        let s = String::from_utf8(c).unwrap();
        // Arrays are NOT reordered -- only object keys are sorted
        assert_eq!(s, "[3,1,2]");
    }

    #[test]
    fn test_canonical_json_primitives() {
        assert_eq!(canonical_json(&42u32).unwrap(), b"42");
        assert_eq!(canonical_json(&"hello").unwrap(), b"\"hello\"");
        assert_eq!(canonical_json(&true).unwrap(), b"true");
        assert_eq!(canonical_json(&serde_json::Value::Null).unwrap(), b"null");
    }
}
