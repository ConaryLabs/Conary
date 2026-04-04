// conary-test/src/handlers.rs

use super::{
    BOLD, GREEN, RED, RESET, YELLOW, color, manifest_dir, print_step, project_dir, run_command,
};
use anyhow::{Context, Result, bail};
use conary_test::paths;
use std::collections::HashMap;
use std::time::Duration;

pub(super) async fn cmd_deploy_source(git_ref: Option<&str>, json: bool) -> Result<()> {
    let dir = project_dir()?;

    if let Some(git_ref) = git_ref {
        let (code, stdout, stderr) = run_command("git", &["fetch", "--all"], Some(&dir)).await?;
        print_step("git fetch", code, &stdout, &stderr, json);
        if code != 0 {
            bail!("git fetch failed (exit {})", code);
        }

        let (code, stdout, stderr) = run_command("git", &["checkout", git_ref], Some(&dir)).await?;
        print_step("git checkout", code, &stdout, &stderr, json);
        if code != 0 {
            bail!("git checkout failed (exit {})", code);
        }
    } else {
        let (code, stdout, stderr) = run_command("git", &["pull"], Some(&dir)).await?;
        print_step("git pull", code, &stdout, &stderr, json);
        if code != 0 {
            bail!("git pull failed (exit {})", code);
        }
    }

    let (code, stdout, stderr) =
        run_command("cargo", &["build", "-p", "conary-test"], Some(&dir)).await?;
    print_step("cargo build conary-test", code, &stdout, &stderr, json);

    let (code, stdout, stderr) = run_command("cargo", &["build"], Some(&dir)).await?;
    print_step("cargo build conary", code, &stdout, &stderr, json);

    Ok(())
}

pub(super) async fn cmd_deploy_rebuild(crate_name: Option<&str>, json: bool) -> Result<()> {
    let dir = project_dir()?;

    let crates: Vec<(&str, &[&str])> = match crate_name {
        Some("conary-test") => vec![("conary-test", &["build", "-p", "conary-test"])],
        Some("conary") => vec![("conary", &["build"])],
        Some(other) => bail!("unknown crate: {other}. Expected: conary, conary-test"),
        None => vec![
            ("conary-test", &["build", "-p", "conary-test"] as &[&str]),
            ("conary", &["build"]),
        ],
    };

    for (label, args) in crates {
        let (code, stdout, stderr) = run_command("cargo", args, Some(&dir)).await?;
        let full_label = format!("cargo build {label}");
        print_step(&full_label, code, &stdout, &stderr, json);
        if code != 0 {
            bail!("{full_label} failed (exit {code})");
        }
    }

    Ok(())
}

pub(super) async fn cmd_deploy_restart(json: bool) -> Result<()> {
    let (code, stdout, stderr) =
        run_command("systemctl", &["--user", "restart", "conary-test"], None).await?;
    print_step(
        "systemctl --user restart conary-test",
        code,
        &stdout,
        &stderr,
        json,
    );

    if code != 0 {
        bail!("service restart failed (exit {code})");
    }

    tokio::time::sleep(Duration::from_secs(1)).await;
    let (code, stdout, _) =
        run_command("systemctl", &["--user", "is-active", "conary-test"], None).await?;
    let status = stdout.trim();
    if json {
        println!(
            "{}",
            serde_json::json!({"service_status": status, "exit_code": code})
        );
    } else if code == 0 {
        println!("Service status: {}", color(status, GREEN));
    } else {
        println!("Service status: {}", color(status, RED));
    }

    Ok(())
}

pub(super) async fn cmd_deploy_status(json: bool) -> Result<()> {
    let version = env!("CARGO_PKG_VERSION");

    let (service_code, service_stdout, _) =
        run_command("systemctl", &["--user", "is-active", "conary-test"], None)
            .await
            .unwrap_or((-1, "unknown".to_string(), String::new()));
    let service_status = if service_code == 0 {
        service_stdout.trim().to_string()
    } else {
        "unknown".to_string()
    };

    let dir = project_dir().unwrap_or_default();
    let (_, git_branch, _) = run_command("git", &["rev-parse", "--abbrev-ref", "HEAD"], Some(&dir))
        .await
        .unwrap_or((1, "unknown".to_string(), String::new()));
    let (_, git_commit, _) = run_command("git", &["rev-parse", "--short", "HEAD"], Some(&dir))
        .await
        .unwrap_or((1, "unknown".to_string(), String::new()));

    if json {
        println!(
            "{}",
            serde_json::json!({
                "version": version,
                "service_status": service_status,
                "git_branch": git_branch.trim(),
                "git_commit": git_commit.trim(),
            })
        );
    } else {
        let status_colored = if service_status == "active" {
            color(&service_status, GREEN)
        } else {
            color(&service_status, YELLOW)
        };
        println!("{}conary-test deployment status{}", BOLD, RESET);
        println!("  Version:  {version}");
        println!("  Service:  {status_colored}");
        println!("  Branch:   {}", git_branch.trim());
        println!("  Commit:   {}", git_commit.trim());
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
    let client = conary_test::server::remi_client::RemiClient::from_env()
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
    match conary_test::server::remi_client::RemiClient::from_env() {
        Ok(client) => {
            let data = client
                .health()
                .await
                .context("failed to fetch health from Remi")?;

            if json {
                println!("{}", serde_json::to_string_pretty(&data)?);
                return Ok(());
            }

            println!("{}Test infrastructure health{}", BOLD, RESET);
            if let Some(obj) = data.as_object() {
                for (key, value) in obj {
                    let display_val = match value {
                        serde_json::Value::String(string) => string.clone(),
                        other => other.to_string(),
                    };
                    println!("  {key}: {display_val}");
                }
            } else {
                println!("{}", serde_json::to_string_pretty(&data)?);
            }
        }
        Err(_) => {
            println!(
                "{}Local status{} (REMI_ADMIN_TOKEN or REMI_ADMIN_ENDPOINT not set)",
                BOLD, RESET
            );
            cmd_deploy_status(json).await?;
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
        println!("{} manifests found", color(&suites.len().to_string(), GREEN));
    }

    Ok(())
}
