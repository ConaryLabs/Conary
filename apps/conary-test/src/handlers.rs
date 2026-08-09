// conary-test/src/handlers.rs

use super::{
    BOLD, GREEN, RED, RESET, YELLOW, color, manifest_dir, print_step, project_dir, run_command,
};
use anyhow::{Context, Result, bail};
use conary_test::build_info::BuildInfo;
use conary_test::deploy::status::{
    BinaryStatus, DeploymentStatus, RolloutStatus, evaluate_rollout_status, load_rollout_provenance,
};
use conary_test::paths;
use conary_test::remi_client::RemiClient;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct CheckoutStatus {
    git_branch: String,
    git_commit: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct DeployStatusOutput {
    binary: BinaryStatus,
    rollout: Option<RolloutStatus>,
    checkout: CheckoutStatus,
    checkout_matches_binary: bool,
    reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
struct HealthEnvelope {
    mode: String,
    deploy_status: DeploymentStatus,
    remi: Option<Value>,
    reason: Option<String>,
}

fn combine_deploy_status(
    deploy_status: DeploymentStatus,
    checkout: CheckoutStatus,
    reason: Option<String>,
) -> DeployStatusOutput {
    let checkout_matches_binary = deploy_status.binary.git_commit == checkout.git_commit;

    DeployStatusOutput {
        binary: deploy_status.binary,
        rollout: None,
        checkout,
        checkout_matches_binary,
        reason,
    }
}

fn local_deploy_status() -> DeploymentStatus {
    let build_info = BuildInfo::current();
    DeploymentStatus {
        binary: BinaryStatus {
            version: build_info.version,
            git_commit: build_info.git_commit,
            commit_timestamp: build_info.commit_timestamp,
            build_timestamp: build_info.build_timestamp,
        },
    }
}

fn build_health_envelope(
    mode: &str,
    deploy_status: DeploymentStatus,
    remi: Option<Value>,
    reason: Option<String>,
) -> HealthEnvelope {
    HealthEnvelope {
        mode: mode.to_string(),
        deploy_status,
        remi,
        reason,
    }
}

async fn current_checkout_status() -> CheckoutStatus {
    let dir = project_dir().unwrap_or_default();
    let (_, git_branch, _) = run_command("git", &["rev-parse", "--abbrev-ref", "HEAD"], Some(&dir))
        .await
        .unwrap_or((1, "unknown".to_string(), String::new()));
    let (_, git_commit, _) = run_command("git", &["rev-parse", "HEAD"], Some(&dir))
        .await
        .unwrap_or((1, "unknown".to_string(), String::new()));

    CheckoutStatus {
        git_branch: git_branch.trim().to_string(),
        git_commit: git_commit.trim().to_string(),
    }
}

fn append_reason(reason: Option<String>, extra: impl Into<String>) -> Option<String> {
    let extra = extra.into();
    match reason {
        Some(existing) => Some(format!("{existing}; {extra}")),
        None => Some(extra),
    }
}

fn attach_rollout_status(
    mut output: DeployStatusOutput,
    rollout: Option<RolloutStatus>,
    rollout_error: Option<String>,
) -> DeployStatusOutput {
    output.rollout = rollout;
    if let Some(error) = rollout_error {
        output.reason = append_reason(output.reason, error);
    }
    output
}

pub(super) async fn cmd_deploy_status(json: bool) -> Result<()> {
    let checkout = current_checkout_status().await;
    let output = combine_deploy_status(local_deploy_status(), checkout, None);
    let rollout_path = paths::rollout_provenance_path();
    let output = match rollout_path {
        Ok(path) => match load_rollout_provenance(&path) {
            Ok(Some(rollout)) => {
                let binary_commit = Some(output.binary.git_commit.as_str());
                let checkout_commit = Some(output.checkout.git_commit.as_str());
                let rollout = evaluate_rollout_status(&rollout, binary_commit, checkout_commit);
                attach_rollout_status(output, Some(rollout), None)
            }
            Ok(None) => output,
            Err(error) => attach_rollout_status(
                output,
                None,
                Some(format!("rollout provenance unavailable: {error}")),
            ),
        },
        Err(error) => attach_rollout_status(
            output,
            None,
            Some(format!("rollout provenance path unavailable: {error}")),
        ),
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    println!("{}conary-test deployment status{}", BOLD, RESET);
    println!("  Binary version: {}", output.binary.version);
    println!("  Binary commit:  {}", output.binary.git_commit);
    println!("  Checkout branch: {}", output.checkout.git_branch);
    println!("  Checkout commit: {}", output.checkout.git_commit);
    if let Some(rollout) = &output.rollout {
        println!("  Rollout target:  {}", rollout.rollout_name);
        println!("  Rollout source:  {:?}", rollout.source_kind);
        println!("  Rollout commit:  {}", rollout.resolved_commit);
        let rollout_drift = if rollout.drifted {
            color("drifted", YELLOW)
        } else {
            color("matched", GREEN)
        };
        println!("  Rollout drift:   {rollout_drift}");
    }
    if output.checkout_matches_binary {
        println!(
            "  Drift:          {}",
            color("checkout matches local binary", GREEN)
        );
    } else {
        println!(
            "  Drift:          {}",
            color("checkout differs from local binary", YELLOW)
        );
    }
    if let Some(reason) = &output.reason {
        println!("  Note:           {}", color(reason, YELLOW));
    }

    Ok(())
}

pub(super) async fn cmd_fixtures_build(groups: &str, json: bool) -> Result<()> {
    let dir = project_dir()?;
    let fixture_dir = paths::fixtures_root()?.join("adversarial");

    let script = match groups {
        "all" => "build-all.sh",
        "corrupted" => "build-corrupted.sh",
        "malicious" => "build-malicious.sh",
        "deps" => "build-deps.sh",
        "boot" => "build-boot-image.sh",
        "large" => "build-large.sh",
        other => bail!(
            "unknown fixture group: {other}. Expected: all, corrupted, malicious, deps, boot, large"
        ),
    };

    let script_path = fixture_dir.join(script);
    let script = script_path.display().to_string();
    let (code, stdout, stderr) = run_command("bash", &[&script], Some(&dir)).await?;
    print_step(
        &format!("build-fixtures ({groups})"),
        code,
        &stdout,
        &stderr,
        json,
    );

    if code != 0 {
        bail!("fixture build failed (exit {code})");
    }
    Ok(())
}

pub(super) async fn cmd_fixtures_publish(json: bool) -> Result<()> {
    let dir = project_dir()?;
    let script_path = format!("{dir}/scripts/publish-test-fixtures.sh");

    let (code, stdout, stderr) = run_command("bash", &[&script_path], Some(&dir)).await?;
    print_step("publish-fixtures", code, &stdout, &stderr, json);

    if code != 0 {
        bail!("fixture publish failed (exit {code})");
    }
    Ok(())
}

pub(super) async fn cmd_logs(
    test_id: &str,
    run_id: Option<u64>,
    step: Option<u32>,
    stream: Option<&str>,
    json: bool,
) -> Result<()> {
    let client = RemiClient::from_env()
        .context("logs command requires REMI_ADMIN_TOKEN and REMI_ADMIN_ENDPOINT to be set")?;

    let rid = run_id.context("--run is required for the logs command")?;

    let data = client
        .get_logs(rid as i64, test_id, stream, step)
        .await
        .with_context(|| format!("failed to fetch logs for {test_id} in run {rid}"))?;

    if json {
        println!("{}", serde_json::to_string_pretty(&data)?);
        return Ok(());
    }

    if let Some(logs) = data.as_array() {
        if logs.is_empty() {
            println!("No logs found for {test_id} in run {rid}");
            return Ok(());
        }
        for entry in logs {
            let step_idx = entry
                .get("step_index")
                .and_then(|value| value.as_u64())
                .unwrap_or(0);
            let stream_name = entry
                .get("stream")
                .and_then(|value| value.as_str())
                .unwrap_or("stdout");
            let content = entry
                .get("content")
                .and_then(|value| value.as_str())
                .unwrap_or("");

            let label = format!("step {step_idx} {stream_name}");
            let header_color = if stream_name == "stderr" { RED } else { GREEN };
            println!("--- {} ---", color(&label, header_color));
            println!("{content}");
        }
    } else {
        println!("{}", serde_json::to_string_pretty(&data)?);
    }

    Ok(())
}

pub(super) async fn cmd_health(json: bool) -> Result<()> {
    let local_status = local_deploy_status();

    match RemiClient::from_env() {
        Ok(client) => match client.health().await {
            Ok(data) => {
                let envelope =
                    build_health_envelope("remi", local_status.clone(), Some(data), None);

                if json {
                    println!("{}", serde_json::to_string_pretty(&envelope)?);
                    return Ok(());
                }

                println!("{}Test infrastructure health{}", BOLD, RESET);
                if let Some(remi) = &envelope.remi {
                    if let Some(obj) = remi.as_object() {
                        for (key, value) in obj {
                            let display_val = match value {
                                serde_json::Value::String(string) => string.clone(),
                                other => other.to_string(),
                            };
                            println!("  {key}: {display_val}");
                        }
                    } else {
                        println!("{}", serde_json::to_string_pretty(remi)?);
                    }
                }
                println!(
                    "  Local binary: {}",
                    envelope.deploy_status.binary.git_commit
                );
            }
            Err(error) => {
                let envelope = build_health_envelope(
                    "local",
                    local_status.clone(),
                    None,
                    Some(format!("failed to fetch health from Remi: {error}")),
                );

                if json {
                    println!("{}", serde_json::to_string_pretty(&envelope)?);
                    return Ok(());
                }

                println!("{}Local status{}", BOLD, RESET);
                if let Some(reason) = &envelope.reason {
                    println!("  Note: {}", color(reason, YELLOW));
                }
                println!(
                    "  Local binary: {}",
                    envelope.deploy_status.binary.git_commit
                );
            }
        },
        Err(_) => {
            let envelope = build_health_envelope(
                "local",
                local_status,
                None,
                Some("REMI_ADMIN_TOKEN or REMI_ADMIN_ENDPOINT not set".to_string()),
            );

            if json {
                println!("{}", serde_json::to_string_pretty(&envelope)?);
                return Ok(());
            }

            println!(
                "{}Local status{} (REMI_ADMIN_TOKEN or REMI_ADMIN_ENDPOINT not set)",
                BOLD, RESET
            );
            println!(
                "  Local binary: {}",
                envelope.deploy_status.binary.git_commit
            );
            if let Some(reason) = &envelope.reason {
                println!("  Note: {}", color(reason, YELLOW));
            }
        }
    }

    Ok(())
}

pub(super) async fn cmd_images_prune(keep: usize, json: bool) -> Result<()> {
    let (code, stdout, _stderr) = run_command(
        "podman",
        &[
            "image",
            "ls",
            "--format",
            "{{.Repository}}:{{.Tag}} {{.ID}} {{.CreatedAt}}",
            "--filter",
            "reference=conary-test-*",
            "--no-trunc",
        ],
        None,
    )
    .await?;

    if code != 0 {
        bail!("failed to list podman images");
    }

    let mut by_distro: HashMap<String, Vec<(String, String, String)>> = HashMap::new();
    for line in stdout.lines() {
        let parts: Vec<&str> = line.splitn(3, ' ').collect();
        if parts.len() < 3 {
            continue;
        }
        let tag = parts[0];
        let id = parts[1];
        let created = parts[2];

        let distro = tag
            .strip_prefix("conary-test-")
            .and_then(|rest| rest.split(':').next())
            .unwrap_or("unknown");

        by_distro.entry(distro.to_string()).or_default().push((
            tag.to_string(),
            id.to_string(),
            created.to_string(),
        ));
    }

    let mut removed = 0u32;
    let mut errors = Vec::new();

    for (_distro, mut images) in by_distro {
        images.sort_by(|a, b| b.2.cmp(&a.2));
        for (_tag, id, _created) in images.into_iter().skip(keep) {
            let (code, _stdout, stderr) =
                run_command("podman", &["image", "rm", "--force", &id], None).await?;
            if code == 0 {
                removed += 1;
            } else {
                errors.push(format!(
                    "failed to remove {}: {}",
                    &id[..12.min(id.len())],
                    stderr.trim()
                ));
            }
        }
    }

    if json {
        println!(
            "{}",
            serde_json::json!({
                "removed": removed,
                "kept_per_distro": keep,
                "errors": errors,
            })
        );
    } else {
        println!("Pruned {removed} images (keeping {keep} per distro)");
        for error in &errors {
            println!("  {}", color(error, RED));
        }
    }

    Ok(())
}

pub(super) async fn cmd_images_info(image: &str, json: bool) -> Result<()> {
    let (code, stdout, stderr) = run_command(
        "podman",
        &["image", "inspect", "--format", "{{json .}}", image],
        None,
    )
    .await?;

    if code != 0 {
        bail!("image '{}' not found: {}", image, stderr.trim());
    }

    let inspect: serde_json::Value =
        serde_json::from_str(&stdout).context("failed to parse podman inspect output")?;

    let value = serde_json::json!({
        "image": image,
        "id": inspect.get("Id").and_then(|value| value.as_str()).unwrap_or(""),
        "created": inspect.get("Created").and_then(|value| value.as_str()).unwrap_or(""),
        "size": inspect.get("Size").and_then(|value| value.as_u64()).unwrap_or(0),
        "labels": inspect
            .pointer("/Config/Labels")
            .cloned()
            .unwrap_or(serde_json::json!({})),
        "repo_tags": inspect
            .get("RepoTags")
            .cloned()
            .unwrap_or(serde_json::json!([])),
    });

    if json {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        let id = value["id"].as_str().unwrap_or("");
        let short_id = if id.len() > 12 { &id[..12] } else { id };
        let created = value["created"].as_str().unwrap_or("");
        let size = value["size"].as_u64().unwrap_or(0);
        let size_mb = size / (1024 * 1024);

        println!("{}Image: {}{}", BOLD, image, RESET);
        println!("  ID:      {short_id}");
        println!("  Created: {created}");
        println!("  Size:    {size_mb} MB");

        if let Some(tags) = value["repo_tags"].as_array() {
            let tag_strs: Vec<&str> = tags.iter().filter_map(|tag| tag.as_str()).collect();
            if !tag_strs.is_empty() {
                println!("  Tags:    {}", tag_strs.join(", "));
            }
        }

        if let Some(labels) = value["labels"].as_object()
            && !labels.is_empty()
        {
            println!("  Labels:");
            for (key, value) in labels {
                let owned = value.to_string();
                let display = value.as_str().unwrap_or(&owned);
                println!("    {key}: {display}");
            }
        }
    }

    Ok(())
}

pub(super) fn cmd_manifests_reload(json: bool) -> Result<()> {
    let dir = manifest_dir()?;
    let dir_path = dir.as_path();

    if !dir_path.is_dir() {
        bail!("manifest directory not found: {}", dir_path.display());
    }

    let mut entries: Vec<_> = std::fs::read_dir(dir_path)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "toml"))
        .collect();
    entries.sort_by_key(|entry| entry.file_name());

    let mut suites = Vec::new();
    for entry in &entries {
        let path = entry.path();
        if let Ok(manifest) = conary_test::config::load_manifest(&path) {
            suites.push(serde_json::json!({
                "name": manifest.suite.name,
                "phase": manifest.suite.phase,
                "test_count": manifest.test.len(),
            }));
        }
    }

    if json {
        println!(
            "{}",
            serde_json::json!({
                "status": "reloaded",
                "manifest_dir": dir.display().to_string(),
                "manifests_found": suites.len(),
                "suites": suites,
            })
        );
    } else {
        println!("Reloaded manifests from {}", dir.display());
        println!();
        println!("{:<30} {:<8} TESTS", "NAME", "PHASE");
        println!("{}", "-".repeat(50));
        for suite in &suites {
            let name = suite["name"].as_str().unwrap_or("");
            let phase = suite["phase"].as_u64().unwrap_or(0);
            let count = suite["test_count"].as_u64().unwrap_or(0);
            println!("{name:<30} {phase:<8} {count}");
        }
        println!();
        println!(
            "{} manifests found",
            color(&suites.len().to_string(), GREEN)
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use conary_test::deploy::status::{
        BinaryStatus, RolloutProvenance, RolloutSourceKind, RolloutTargetKind,
    };
    use serde_json::json;

    fn sample_deploy_status(git_commit: &str) -> DeploymentStatus {
        DeploymentStatus {
            binary: BinaryStatus {
                version: "0.3.0".to_string(),
                git_commit: git_commit.to_string(),
                commit_timestamp: "2026-04-09T00:00:00Z".to_string(),
                build_timestamp: None,
            },
        }
    }

    fn sample_rollout() -> RolloutStatus {
        let provenance = RolloutProvenance {
            source_kind: RolloutSourceKind::GitRef,
            requested_ref: Some("main".to_string()),
            resolved_commit: "abc123".to_string(),
            target_kind: RolloutTargetKind::Group,
            rollout_name: "control_plane".to_string(),
            units: vec!["conary_test".to_string()],
            deployed_at: "2026-04-09T00:00:00+00:00".to_string(),
        };

        evaluate_rollout_status(&provenance, Some("abc123"), Some("abc123"))
    }

    #[test]
    fn combine_deploy_status_marks_binary_checkout_drift() {
        let output = combine_deploy_status(
            sample_deploy_status("abc123"),
            CheckoutStatus {
                git_branch: "main".to_string(),
                git_commit: "def456".to_string(),
            },
            None,
        );

        assert!(!output.checkout_matches_binary);
        assert_eq!(output.binary.git_commit, "abc123");
    }

    #[test]
    fn combine_deploy_status_has_no_server_runtime_or_degraded_fields() {
        let output = combine_deploy_status(
            sample_deploy_status("abc123"),
            CheckoutStatus {
                git_branch: "main".to_string(),
                git_commit: "abc123".to_string(),
            },
            None,
        );

        let json = serde_json::to_value(output).unwrap();
        assert_eq!(json["binary"]["git_commit"], "abc123");
        assert_eq!(json["checkout_matches_binary"], true);
        assert!(json.get("degraded").is_none());
        assert!(json.get("runtime").is_none());
        assert!(json.get("service").is_none());
    }

    #[test]
    fn attach_rollout_status_includes_rollout_section_in_json_output() {
        let output = combine_deploy_status(
            sample_deploy_status("abc123"),
            CheckoutStatus {
                git_branch: "main".to_string(),
                git_commit: "abc123".to_string(),
            },
            None,
        );

        let value =
            serde_json::to_value(attach_rollout_status(output, Some(sample_rollout()), None))
                .expect("json serializes");

        assert_eq!(value["rollout"]["rollout_name"], "control_plane");
        assert_eq!(value["rollout"]["drifted"], false);
    }

    #[test]
    fn build_health_envelope_uses_one_normalized_json_shape() {
        let envelope = build_health_envelope(
            "local",
            sample_deploy_status("abc123"),
            Some(json!({"status": "ok"})),
            Some("fallback".to_string()),
        );

        let value = serde_json::to_value(envelope).unwrap();
        assert_eq!(value["mode"], "local");
        assert_eq!(value["deploy_status"]["binary"]["git_commit"], "abc123");
        assert!(value.get("remi").is_some());
        assert_eq!(value["reason"], "fallback");
    }
}
