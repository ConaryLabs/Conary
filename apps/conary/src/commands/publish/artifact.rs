// apps/conary/src/commands/publish/artifact.rs

//! Publication of an already-built, attested CCS artifact.

use super::*;

pub(super) async fn publish_artifact_form(
    options: PublishOptions,
    target: &str,
    writer: &mut impl Write,
) -> Result<()> {
    let operation_id = publish_operation_id();
    match classify_publish_target(target)? {
        PublishTargetRoute::StaticLocal => {
            publish_static_artifact_form(options, target, writer, operation_id).await
        }
        PublishTargetRoute::RemiRelease if options.json => {
            let message = "Remi publish JSON output is not supported in M3a";
            let output = publish_failure_output(
                &operation_id,
                PackagingDiagnosticCode::PublishJsonUnsupported,
                message,
            );
            super::super::diagnostics::write_packaging_output(&output, true, writer)?;
            super::super::diagnostics::write_packaging_record_if_possible(&output);
            bail!("{message}")
        }
        PublishTargetRoute::RemiRelease => publish_remi_artifact_form(options, target).await,
    }
}

async fn publish_static_artifact_form(
    options: PublishOptions,
    target: &str,
    writer: &mut impl Write,
    operation_id: String,
) -> Result<()> {
    let json = options.json;
    let input = StaticArtifactPublishServiceInput {
        artifact_path: PathBuf::from(&options.what),
        target: target.to_string(),
        key_dir: options.key_dir.as_deref().map(PathBuf::from),
        state_file: options.state_file.as_deref().map(PathBuf::from),
        refresh: options.refresh,
        rotate_publish_key: options.rotate_publish_key,
        rotate_root_key: options.rotate_root_key,
        operation_id,
    };
    let output = publish_static_artifact_form_service(input).await?;

    if output.status == PackagingCommandStatus::Failed {
        super::super::diagnostics::write_packaging_output(&output, json, writer)?;
        super::super::diagnostics::write_packaging_record_if_possible(&output);
        bail!("{}", publish_failure_message_from_output(&output));
    }

    if json {
        super::super::diagnostics::write_packaging_output(&output, true, writer)?;
    } else {
        let repo_name = derive_repo_name(target)?;
        writeln!(
            writer,
            "Published attested artifact to static repo: {repo_name}"
        )?;
        if let Some(publish_key_id) = publish_key_id_from_output(&output) {
            writeln!(writer, "Publish key ID: {publish_key_id}")?;
        }
    }
    super::super::diagnostics::write_packaging_record_if_possible(&output);

    Ok(())
}

pub(crate) async fn publish_static_artifact_form_service(
    input: StaticArtifactPublishServiceInput,
) -> Result<PackagingCommandOutput> {
    let artifact_path = input.artifact_path;
    let destination = RepoLocation::parse(&input.target)
        .with_context(|| format!("parse publish target {}", input.target))?;
    ensure_static_local_publish_destination(&destination)?;
    let repo_name = derive_repo_name(&input.target)?;
    let key_dir = match input.key_dir {
        Some(key_dir) => key_dir,
        None => resolve_key_dir(None, &repo_name)?,
    };
    let prepared = prepare_artifact_form_static_context(&destination, &key_dir)
        .with_context(|| format!("prepare static artifact publish context for {repo_name}"))?;
    let report = verify_static_artifact_publish_eligibility(
        &artifact_path,
        &prepared.accepted_signers,
        &prepared.publish_policy_digest,
    )?;
    if !report.is_passed() {
        return Ok(publish_gate_failure_output(&input.operation_id, &report));
    }
    let state_file = input
        .state_file
        .unwrap_or_else(|| key_dir.join("last-published.toml"));
    let outcome = publish_static_repo(StaticPublishOptions {
        repo_name: repo_name.clone(),
        repo_description: None,
        destination,
        key_dir,
        state_file,
        package_paths: vec![artifact_path.clone()],
        refresh: input.refresh,
        rotate_publish_key: input.rotate_publish_key,
        rotate_root_key: input.rotate_root_key,
        artifact_gate_context: Some(prepared.artifact_gate_context()),
    })
    .with_context(|| format!("publish attested artifact to static repo {repo_name}"))?;

    let mut output =
        publish_success_output(&input.operation_id, "Published static artifact to repo");
    output.artifacts.push(PackagingArtifact {
        path: artifact_path.display().to_string(),
        kind: Some("ccs".to_string()),
    });
    output.events.push(PackagingEvent {
        schema_version: conary_core::diagnostics::PACKAGING_JSON_SCHEMA_VERSION,
        operation_id: input.operation_id,
        sequence: 1,
        phase: PackagingPhase::Publish,
        kind: PackagingEventKind::OperationFinished,
        message: Some(format!("Publish key ID: {}", outcome.publish_key_id)),
        diagnostic: None,
        artifact: None,
        progress: None,
    });
    Ok(output)
}

fn publish_failure_message_from_output(output: &PackagingCommandOutput) -> String {
    output
        .diagnostics
        .first()
        .map(|diagnostic| diagnostic.message.clone())
        .unwrap_or_else(|| "static artifact publish failed".to_string())
}

fn publish_key_id_from_output(output: &PackagingCommandOutput) -> Option<&str> {
    output
        .events
        .iter()
        .filter_map(|event| event.message.as_deref())
        .find_map(|message| message.strip_prefix("Publish key ID: "))
}

async fn publish_remi_artifact_form(options: PublishOptions, target: &str) -> Result<()> {
    let artifact_path = PathBuf::from(&options.what);
    let key_dir = options.key_dir.as_deref().context(
        "Remi artifact publication requires --key-dir so the artifact signer can be authenticated before upload",
    )?;
    let publish_key = conary_core::ccs::SigningKeyPair::load_from_file(
        &Path::new(key_dir).join("publish.private"),
    )
    .map_err(anyhow::Error::from)
    .with_context(|| format!("load Remi artifact publish key from {key_dir}"))?;
    let trust_policy = conary_core::ccs::TrustPolicy::strict(vec![publish_key.public_key_base64()]);
    let bearer_token = resolve_remi_publish_bearer_token()?;
    publish_to_remi(RemiPublishOptions {
        artifact_path: &artifact_path,
        target_url: target,
        bearer_token: &bearer_token,
        trust_policy: &trust_policy,
    })
    .await?;

    println!("Published attested artifact to Remi release endpoint: {target}");
    Ok(())
}
