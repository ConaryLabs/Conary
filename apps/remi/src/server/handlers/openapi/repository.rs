// apps/remi/src/server/handlers/openapi/repository.rs

//! Repository parser and ecosystem-native trust schemas.

use serde_json::{Map, Value, json};

pub(super) fn schemas() -> Map<String, Value> {
    json!({
        "OpenPgpTrustRoot": {
            "type": "object",
            "additionalProperties": false,
            "required": ["url", "fingerprint"],
            "properties": {
                "url": { "type": "string", "format": "uri", "pattern": "^https://" },
                "fingerprint": { "type": "string", "pattern": "^([0-9A-F]{40}|[0-9A-F]{64})$" }
            }
        },
        "RepositoryParser": {
            "type": "object",
            "description": "Exact tagged metadata grammar. Required on create and replace.",
            "required": ["package_format"],
            "properties": {
                "package_format": { "type": "string", "enum": ["rpm", "deb", "arch", "json"] },
                "distribution": { "type": "string" },
                "component": { "type": "string" },
                "architecture": { "type": "string" },
                "database": { "type": "string" }
            }
        },
        "RepositoryTrustPolicy": {
            "oneOf": [
                {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["ecosystem", "release_keys"],
                    "properties": {
                        "ecosystem": { "const": "debian" },
                        "release_keys": {
                            "type": "array",
                            "minItems": 1,
                            "items": { "$ref": "#/components/schemas/OpenPgpTrustRoot" }
                        }
                    }
                },
                {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["ecosystem", "metadata", "package_keys"],
                    "properties": {
                        "ecosystem": { "const": "rpm" },
                        "metadata": {
                            "oneOf": [
                                {
                                    "type": "object",
                                    "additionalProperties": false,
                                    "required": ["kind", "keys"],
                                    "properties": {
                                        "kind": { "const": "open-pgp" },
                                        "keys": {
                                            "type": "array",
                                            "minItems": 1,
                                            "items": { "$ref": "#/components/schemas/OpenPgpTrustRoot" }
                                        }
                                    }
                                },
                                {
                                    "type": "object",
                                    "additionalProperties": false,
                                    "required": ["kind", "url"],
                                    "properties": {
                                        "kind": { "const": "metalink" },
                                        "url": { "type": "string", "format": "uri", "pattern": "^https://" }
                                    }
                                }
                            ]
                        },
                        "package_keys": {
                            "type": "array",
                            "minItems": 1,
                            "items": { "$ref": "#/components/schemas/OpenPgpTrustRoot" }
                        }
                    }
                },
                {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["ecosystem", "keyring", "sig_level"],
                    "properties": {
                        "ecosystem": { "const": "arch" },
                        "keyring": {
                            "type": "object",
                            "additionalProperties": false,
                            "required": [
                                "url",
                                "format",
                                "master_fingerprints",
                                "packager_key_threshold"
                            ],
                            "properties": {
                                "url": { "type": "string", "format": "uri", "pattern": "^https://" },
                                "format": {
                                    "type": "string",
                                    "enum": ["open-pgp", "alpm-package-zstd"]
                                },
                                "packager_key_threshold": {
                                    "type": "integer",
                                    "minimum": 1,
                                    "description": "Distinct pinned master certifications required for a packager key"
                                },
                                "master_fingerprints": {
                                    "type": "array",
                                    "minItems": 1,
                                    "items": {
                                        "type": "string",
                                        "pattern": "^([0-9A-F]{40}|[0-9A-F]{64})$"
                                    }
                                }
                            }
                        },
                        "sig_level": {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["database", "package", "trust"],
                            "properties": {
                                "database": {
                                    "type": "string",
                                    "enum": ["required", "optional"]
                                },
                                "package": { "const": "required" },
                                "trust": { "const": "trusted-only" }
                            }
                        }
                    }
                }
            ]
        }
    })
    .as_object()
    .expect("repository OpenAPI schemas must be an object")
    .clone()
}
