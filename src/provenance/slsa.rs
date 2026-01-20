// src/provenance/slsa.rs

//! SLSA provenance generation for in-toto attestations.

use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SlsaError {
    #[error("missing DNA hash for SLSA export")]
    MissingDnaHash,
    #[error("failed to parse build dependencies JSON: {0}")]
    BuildDepsJson(#[from] serde_json::Error),
}

#[derive(Debug, Clone)]
pub struct SlsaContext<'a> {
    pub name: &'a str,
    pub version: &'a str,
    pub dna_hash: Option<&'a str>,
    pub upstream_url: Option<&'a str>,
    pub upstream_hash: Option<&'a str>,
    pub git_commit: Option<&'a str>,
    pub recipe_hash: Option<&'a str>,
    pub build_deps_json: Option<&'a str>,
    pub host_arch: Option<&'a str>,
    pub host_kernel: Option<&'a str>,
    pub dependencies: &'a [(String, String, Option<String>)],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InTotoStatement {
    #[serde(rename = "_type")]
    statement_type: String,
    subject: Vec<Subject>,
    predicate_type: String,
    predicate: SlsaPredicate,
}

#[derive(Serialize)]
struct Subject {
    name: String,
    digest: BTreeMap<String, String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SlsaPredicate {
    builder: Builder,
    build_type: String,
    invocation: Invocation,
    build_config: Value,
    materials: Vec<Material>,
}

#[derive(Serialize)]
struct Builder {
    id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Invocation {
    parameters: Value,
    environment: Value,
}

#[derive(Serialize)]
struct Material {
    uri: String,
    digest: BTreeMap<String, String>,
}

pub fn build_slsa_statement(context: SlsaContext<'_>) -> Result<String, SlsaError> {
    let dna = context.dna_hash.ok_or(SlsaError::MissingDnaHash)?;
    let dna_value = strip_sha256_prefix(dna);

    let subject = Subject {
        name: format!("pkg:conary/{}@{}", context.name, context.version),
        digest: single_digest("sha256", dna_value),
    };

    let invocation = Invocation {
        parameters: Value::Object(Map::new()),
        environment: build_environment(context.host_arch, context.host_kernel),
    };

    let build_config = build_config(
        context.recipe_hash,
        context.build_deps_json,
        context.host_arch,
        context.host_kernel,
    )?;

    let materials = build_materials(context)?;

    let predicate = SlsaPredicate {
        builder: Builder {
            id: "https://conary.dev/builder".to_string(),
        },
        build_type: "https://conary.dev/provenance/build/v1".to_string(),
        invocation,
        build_config,
        materials,
    };

    let statement = InTotoStatement {
        statement_type: "https://in-toto.io/Statement/v1".to_string(),
        subject: vec![subject],
        predicate_type: "https://slsa.dev/provenance/v1".to_string(),
        predicate,
    };

    Ok(serde_json::to_string_pretty(&statement)?)
}

fn build_materials(context: SlsaContext<'_>) -> Result<Vec<Material>, SlsaError> {
    let mut materials = Vec::new();

    if let (Some(url), Some(hash)) = (context.upstream_url, context.upstream_hash) {
        if let Some((alg, value)) = split_hash(hash) {
            materials.push(Material {
                uri: url.to_string(),
                digest: single_digest(&alg, &value),
            });
        }
    }

    if let Some(commit) = context.git_commit {
        let uri = if let Some(url) = context.upstream_url {
            format!("git+{}@{}", url, commit)
        } else {
            format!("git:{}", commit)
        };
        let digest = if commit.len() == 40 {
            single_digest("sha1", commit)
        } else {
            single_digest("sha256", strip_sha256_prefix(commit))
        };
        materials.push(Material { uri, digest });
    }

    for (name, version, dna) in context.dependencies {
        if let Some(dna) = dna.as_deref() {
            let digest = single_digest("sha256", strip_sha256_prefix(dna));
            materials.push(Material {
                uri: format!("pkg:conary/{}@{}", name, version),
                digest,
            });
        }
    }

    Ok(materials)
}

fn build_config(
    recipe_hash: Option<&str>,
    build_deps_json: Option<&str>,
    host_arch: Option<&str>,
    host_kernel: Option<&str>,
) -> Result<Value, SlsaError> {
    let mut config = Map::new();

    if let Some(hash) = recipe_hash {
        config.insert("recipe_hash".to_string(), Value::String(hash.to_string()));
    }

    if let Some(build_deps_json) = build_deps_json {
        let build_deps: Value = serde_json::from_str(build_deps_json)?;
        config.insert("build_deps".to_string(), build_deps);
    }

    if host_arch.is_some() || host_kernel.is_some() {
        let mut host = Map::new();
        if let Some(arch) = host_arch {
            host.insert("arch".to_string(), Value::String(arch.to_string()));
        }
        if let Some(kernel) = host_kernel {
            host.insert("kernel".to_string(), Value::String(kernel.to_string()));
        }
        config.insert("host".to_string(), Value::Object(host));
    }

    Ok(Value::Object(config))
}

fn build_environment(host_arch: Option<&str>, host_kernel: Option<&str>) -> Value {
    let mut env = Map::new();
    if let Some(arch) = host_arch {
        env.insert("arch".to_string(), Value::String(arch.to_string()));
    }
    if let Some(kernel) = host_kernel {
        env.insert("kernel".to_string(), Value::String(kernel.to_string()));
    }
    Value::Object(env)
}

fn single_digest(algorithm: &str, value: &str) -> BTreeMap<String, String> {
    let mut digest = BTreeMap::new();
    digest.insert(algorithm.to_string(), value.to_string());
    digest
}

fn strip_sha256_prefix(value: &str) -> &str {
    value.strip_prefix("sha256:").unwrap_or(value)
}

fn split_hash(hash: &str) -> Option<(String, String)> {
    let mut parts = hash.splitn(2, ':');
    let algorithm = parts.next()?.to_string();
    let value = parts.next()?.to_string();
    Some((algorithm, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_statement_with_dna() {
        let deps = vec![(
            "libfoo".to_string(),
            "1.2.3".to_string(),
            Some("sha256:abc123".to_string()),
        )];
        let context = SlsaContext {
            name: "demo",
            version: "0.1.0",
            dna_hash: Some("sha256:deadbeef"),
            upstream_url: Some("https://example.com/src.tar.gz"),
            upstream_hash: Some("sha256:beadfeed"),
            git_commit: None,
            recipe_hash: Some("sha256:recipe"),
            build_deps_json: Some("[{\"name\":\"libfoo\"}]"),
            host_arch: Some("x86_64"),
            host_kernel: Some("6.1.0"),
            dependencies: &deps,
        };

        let statement = build_slsa_statement(context).unwrap();
        assert!(statement.contains("https://slsa.dev/provenance/v1"));
        assert!(statement.contains("pkg:conary/demo@0.1.0"));
    }
}
