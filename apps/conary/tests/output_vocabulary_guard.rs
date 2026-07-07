// tests/output_vocabulary_guard.rs
//! Fails if guarded status-vocabulary literals are emitted outside the `ui`
//! module. This is a line-level lint: it matches the exact listed words only.

use std::fs;
use std::path::Path;

const FORBIDDEN: &[&str] = &[
    "[OK]",
    "[COMPLETE]",
    "[DONE]",
    "[VALID]",
    "[FAIL]",
    "[FAILED]",
    "[ERROR]",
    "[WARN]",
    "[WARNING]",
    "[INFO]",
    "[OFF]",
    "[MISSING]",
    "[PENDING]",
];

fn string_literals(line: &str) -> Vec<String> {
    let mut literals = Vec::new();
    let mut chars = line.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch != '"' {
            continue;
        }

        let mut literal = String::new();
        let mut escaped = false;
        for next in chars.by_ref() {
            if escaped {
                literal.push(next);
                escaped = false;
                continue;
            }
            match next {
                '\\' => escaped = true,
                '"' => break,
                _ => literal.push(next),
            }
        }
        literals.push(literal);
    }

    literals
}

fn scan(dir: &Path, violations: &mut Vec<String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "ui") {
                continue;
            }
            scan(&path, violations);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let text = fs::read_to_string(&path).unwrap();
            for (idx, line) in text.lines().enumerate() {
                let hit = string_literals(line).into_iter().any(|literal| {
                    let upper = literal.to_uppercase();
                    FORBIDDEN.iter().any(|token| upper.contains(token))
                        || literal.starts_with("Warning:")
                });
                if hit {
                    violations.push(format!("{}:{}: {}", path.display(), idx + 1, line.trim()));
                }
            }
        }
    }
}

#[test]
fn no_raw_status_vocabulary_outside_ui() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();
    scan(&src, &mut violations);
    assert!(
        violations.is_empty(),
        "raw status vocabulary must route through the ui module ({} sites):\n{}",
        violations.len(),
        violations.join("\n")
    );
}
