// conary-test/src/handlers.rs

use super::{
    BOLD, GREEN, RED, RESET, YELLOW, color, manifest_dir, print_step, project_dir, run_command,
};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use chrono::Utc;
use conary_test::deploy::manifest::load_rollout_manifest_from_file;
use conary_test::deploy::orchestrator::{RolloutExecutor, execute_rollout};
use conary_test::deploy::plan::{RolloutPlan, RolloutPlanRequest, build_rollout_plan};
use conary_test::deploy::status::{
    RolloutProvenance, RolloutStatus, evaluate_rollout_status, load_rollout_provenance,
    write_rollout_provenance,
};
use conary_test::paths;
use conary_test::server::service::DeploymentStatus;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct CheckoutStatus {
    git_branch: String,
    git_commit: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct DeployStatusOutput {
    binary: Option<conary_test::server::service::BinaryStatus>,
    runtime: Option<conary_test::server::service::RuntimeStatus>,
    service: Option<conary_test::server::service::ServiceStatus>,
    rollout: Option<RolloutStatus>,
    checkout: CheckoutStatus,
    checkout_matches_binary: Option<bool>,
    degraded: bool,
    reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
struct HealthEnvelope {
    mode: String,
    deploy_status: Option<DeploymentStatus>,
    remi: Option<Value>,
    reason: Option<String>,
}

struct HandlerRolloutExecutor {
    json: bool,
}

impl HandlerRolloutExecutor {
    async fn run_checked(
        &self,
        label: &str,
        cmd: &str,
        args: &[&str],
        cwd: Option<&Path>,
    ) -> Result<()> {
        let cwd_string = cwd
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_default();
        let cwd_ref = if cwd.is_some() {
            Some(cwd_string.as_str())
        } else {
            None
        };

        let (code, stdout, stderr) = run_command(cmd, args, cwd_ref).await?;
        print_step(label, code, &stdout, &stderr, self.json);
        if code != 0 {
            bail!("{label} failed (exit {code})");
        }
        Ok(())
    }
}

#[async_trait]
impl RolloutExecutor for HandlerRolloutExecutor {
    async fn git_fetch(&mut self, repo_dir: &Path, git_ref: &str) -> Result<()> {
        self.run_checked(
            &format!("git fetch {git_ref}"),
            "git",
            &["fetch", "origin", git_ref],
            Some(repo_dir),
        )
        .await
    }

    async fn git_checkout(&mut self, repo_dir: &Path, git_ref: &str) -> Result<()> {
        let _ = git_ref;
        self.run_checked(
            "git checkout FETCH_HEAD",
            "git",
            &["checkout", "--detach", "FETCH_HEAD"],
            Some(repo_dir),
        )
        .await
    }

    async fn cargo_build_package(&mut self, repo_dir: &Path, package: &str) -> Result<()> {
        self.run_checked(
            &format!("cargo build {package}"),
            "cargo",
            &["build", "-p", package],
            Some(repo_dir),
        )
        .await
    }

    async fn restart_systemd_user_unit(&mut self, unit: &str) -> Result<()> {
        self.run_checked(
            &format!("systemctl --user restart {unit}"),
            "systemctl",
            &["--user", "restart", unit],
            None,
        )
        .await?;

        tokio::time::sleep(Duration::from_secs(1)).await;
        self.run_checked(
            &format!("systemctl --user is-active {unit}"),
            "systemctl",
            &["--user", "is-active", unit],
            None,
        )
        .await
    }

    async fn verify(&mut self, verify_mode: &str, repo_dir: &Path) -> Result<()> {
        match verify_mode {
            "forge_smoke" => {
                self.run_checked(
                    "forge smoke",
                    "bash",
                    &["scripts/forge-smoke.sh"],
                    Some(repo_dir),
                )
                .await
            }
            other => bail!("unsupported verify mode `{other}`"),
        }
    }

    async fn record_success(&mut self, plan: &RolloutPlan, work_tree: &Path) -> Result<()> {
        let work_tree_string = work_tree.to_string_lossy().to_string();
        let (code, stdout, stderr) = run_command(
            "git",
            &["rev-parse", "HEAD"],
            Some(work_tree_string.as_str()),
        )
        .await?;
        print_step("git rev-parse HEAD", code, &stdout, &stderr, self.json);
        if code != 0 {
            bail!("git rev-parse HEAD failed (exit {code})");
        }

        let provenance = RolloutProvenance::from_plan(plan, stdout.trim(), Utc::now());
        let provenance_path = paths::rollout_provenance_path()?;
        write_rollout_provenance(&provenance_path, &provenance)?;
        Ok(())
    }
}

fn local_service_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/v1/deploy/status")
}

fn combine_deploy_status(
    deploy_status: Option<DeploymentStatus>,
    checkout: CheckoutStatus,
    reason: Option<String>,
) -> DeployStatusOutput {
    let checkout_matches_binary = deploy_status
        .as_ref()
        .map(|status| status.binary.git_commit == checkout.git_commit);

    DeployStatusOutput {
        binary: deploy_status.as_ref().map(|status| status.binary.clone()),
        runtime: deploy_status.as_ref().map(|status| status.runtime.clone()),
        service: deploy_status.as_ref().map(|status| status.service.clone()),
        rollout: None,
        checkout,
        checkout_matches_binary,
        degraded: deploy_status.is_none(),
        reason,
    }
}

fn build_health_envelope(
    mode: &str,
    deploy_status: Option<DeploymentStatus>,
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

async fn fetch_local_deploy_status(port: u16) -> Result<DeploymentStatus> {
    let response = reqwest::get(local_service_url(port))
        .await
        .with_context(|| format!("failed to reach local conary-test service on port {port}"))?;
    let status = response.status();
    if !status.is_success() {
        bail!("local conary-test service returned HTTP {status}");
    }

    response
        .json::<DeploymentStatus>()
        .await
        .context("failed to parse local deployment status JSON")
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

pub(super) async fn cmd_deploy_rollout(
    unit: Option<String>,
    group: Option<String>,
    git_ref: Option<String>,
    path: Option<PathBuf>,
    json: bool,
) -> Result<()> {
    let project_root = PathBuf::from(project_dir()?);
    let manifest_path = project_root.join("deploy/forge-rollouts.toml");
    let manifest = load_rollout_manifest_from_file(&manifest_path)?;
    let plan = build_rollout_plan(
        &manifest,
        RolloutPlanRequest {
            unit,
            group,
            git_ref,
            path,
        },
    )?;

    let mut executor = HandlerRolloutExecutor { json };
    execute_rollout(&mut executor, &plan, &project_root).await?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "status": "ok",
                "target": format!("{:?}", plan.target),
                "source": format!("{:?}", plan.source),
            })
        );
    } else {
        println!("Managed rollout completed successfully.");
    }

    Ok(())
}

pub(super) async fn cmd_deploy_status(json: bool, port: u16) -> Result<()> {
    let checkout = current_checkout_status().await;
    let local_status = fetch_local_deploy_status(port).await;
    let output = match local_status {
        Ok(status) => combine_deploy_status(Some(status), checkout, None),
        Err(error) => combine_deploy_status(
            None,
            checkout,
            Some(format!("local deployment status unavailable: {error}")),
        ),
    };
    let rollout_path = paths::rollout_provenance_path();
    let output = match rollout_path {
        Ok(path) => match load_rollout_provenance(&path) {
            Ok(Some(rollout)) => {
                let binary_commit = output
                    .binary
                    .as_ref()
                    .map(|binary| binary.git_commit.as_str());
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
    if let Some(binary) = &output.binary {
        println!("  Binary version: {}", binary.version);
        println!("  Binary commit:  {}", binary.git_commit);
    } else {
        println!("  Binary:         {}", color("unavailable", YELLOW));
    }
    if let Some(runtime) = &output.runtime {
        println!("  Uptime:         {}", runtime.uptime_human);
        println!("  WAL pending:    {}", runtime.wal_pending);
    }
    if let Some(service) = &output.service {
        let service_colored = if service.status == "running" {
            color(&service.status, GREEN)
        } else {
            color(&service.status, YELLOW)
        };
        println!("  Service:        {service_colored}");
    }
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
    match output.checkout_matches_binary {
        Some(true) => println!(
            "  Drift:          {}",
            color("checkout matches running binary", GREEN)
        ),
        Some(false) => println!(
            "  Drift:          {}",
            color("checkout differs from running binary", YELLOW)
        ),
        None => println!("  Drift:          {}", color("unknown", YELLOW)),
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

pub(super) async fn cmd_health(json: bool, port: u16) -> Result<()> {
    let local_status = fetch_local_deploy_status(port).await.ok();

    match conary_test::server::remi_client::RemiClient::from_env() {
        Ok(client) => match client.health().await {
            Ok(data) => {
                let envelope = build_health_envelope("remi", local_status, Some(data), None);

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
                if let Some(deploy_status) = &envelope.deploy_status {
                    println!("  Local binary: {}", deploy_status.binary.git_commit);
                }
            }
            Err(error) => {
                let envelope = build_health_envelope(
                    "local",
                    local_status,
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
                if let Some(deploy_status) = &envelope.deploy_status {
                    println!("  Local binary: {}", deploy_status.binary.git_commit);
                }
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
            let deploy_output = combine_deploy_status(
                envelope.deploy_status.clone(),
                current_checkout_status().await,
                envelope.reason.clone(),
            );
            if let Some(binary) = &deploy_output.binary {
                println!("  Binary commit: {}", binary.git_commit);
            }
            if let Some(reason) = &deploy_output.reason {
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use conary_test::deploy::manifest::load_rollout_manifest_from_str;
    use conary_test::deploy::plan::{RolloutPlanRequest, build_rollout_plan};
    use conary_test::deploy::status::RolloutProvenance;
    use conary_test::server::service::{BinaryStatus, RuntimeStatus, ServiceStatus};
    use serde_json::json;

    fn sample_deploy_status(git_commit: &str) -> DeploymentStatus {
        DeploymentStatus {
            binary: BinaryStatus {
                version: "0.3.0".to_string(),
                git_commit: git_commit.to_string(),
                commit_timestamp: "2026-04-09T00:00:00Z".to_string(),
                build_timestamp: None,
            },
            runtime: RuntimeStatus {
                started_at: "2026-04-09T00:00:00Z".to_string(),
                uptime_seconds: 42,
                uptime_human: "0d 0h 0m 42s".to_string(),
                wal_pending: 1,
                active_runs: 0,
            },
            service: ServiceStatus {
                status: "running".to_string(),
            },
        }
    }

    fn sample_rollout() -> RolloutStatus {
        let manifest = load_rollout_manifest_from_str(
            r#"
[units.conary_test]
build = { cargo_package = "conary-test" }
restart = { systemd_user_unit = "conary-test.service" }
verify = "forge_smoke"

[groups.control_plane]
units = ["conary_test"]
"#,
        )
        .expect("manifest parses");
        let plan = build_rollout_plan(
            &manifest,
            RolloutPlanRequest {
                unit: None,
                group: Some("control_plane".to_string()),
                git_ref: Some("main".to_string()),
                path: None,
            },
        )
        .expect("plan builds");
        let provenance = RolloutProvenance::from_plan(
            &plan,
            "abc123".to_string(),
            DateTime::parse_from_rfc3339("2026-04-09T00:00:00Z")
                .expect("timestamp parses")
                .with_timezone(&Utc),
        );

        evaluate_rollout_status(&provenance, Some("abc123"), Some("abc123"))
    }

    #[test]
    fn combine_deploy_status_marks_binary_checkout_drift() {
        let output = combine_deploy_status(
            Some(sample_deploy_status("abc123")),
            CheckoutStatus {
                git_branch: "main".to_string(),
                git_commit: "def456".to_string(),
            },
            None,
        );

        assert_eq!(output.checkout_matches_binary, Some(false));
        assert!(!output.degraded);
        assert_eq!(output.binary.unwrap().git_commit, "abc123");
    }

    #[test]
    fn combine_deploy_status_marks_degraded_output_when_service_is_unreachable() {
        let output = combine_deploy_status(
            None,
            CheckoutStatus {
                git_branch: "main".to_string(),
                git_commit: "def456".to_string(),
            },
            Some("local deployment status unavailable".to_string()),
        );

        let json = serde_json::to_value(output).unwrap();
        assert_eq!(json["degraded"], true);
        assert_eq!(json["reason"], "local deployment status unavailable");
        assert!(json["binary"].is_null());
    }

    #[test]
    fn attach_rollout_status_includes_rollout_section_in_json_output() {
        let output = combine_deploy_status(
            Some(sample_deploy_status("abc123")),
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
            Some(sample_deploy_status("abc123")),
            Some(json!({"status": "ok"})),
            Some("fallback".to_string()),
        );

        let value = serde_json::to_value(envelope).unwrap();
        assert_eq!(value["mode"], "local");
        assert!(value.get("deploy_status").is_some());
        assert!(value.get("remi").is_some());
        assert_eq!(value["reason"], "fallback");
    }
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
