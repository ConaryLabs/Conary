// crates/conary-core/src/repository/catalog/parity/debian/resolution/worker.rs

//! Private apt-pkg worker process launch and line-delimited wire protocol.

use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde::{Deserialize, Serialize};

use super::{PackageResolutionIndexReader, resolve_exact_root};
use crate::error::{Error, Result};
use crate::repository::catalog::parity::resolution_parallel::{
    ResolutionWorkerCount, ResolutionWorkerRequest,
};
use crate::repository::catalog::parity::resolution_root::{
    NativeResolutionWireErrorV1, NativeRootResolutionError, NativeRootResolutionResult,
    NativeRootResolutionSuccess,
};
use crate::repository::catalog::parity::resolution_survey::{
    NativeResolutionSurveyErrorReasonV1, NativeResolutionSurveyNativeExplanationV1,
};
use crate::repository::catalog::parity::{
    NativeParityPackageV1, NativeResolutionArchitectureAdmissionV1,
    NativeResolutionInstalledStateV1, NativeResolutionOutcomeV1, NativeResolutionPolicyV1,
    NativeResolutionProviderPolicyV1, NativeResolutionRequirementPolicyV1,
    NativeResolutionRootPolicyV1,
};
use crate::repository::versioning::VersionScheme;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DebianResolutionWorkerRequest {
    root: DebianResolutionRoot,
    explanation_byte_limit: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DebianResolutionRoot {
    pub(super) package_key_sha256: String,
    pub(super) source_profile: String,
    pub(super) name: String,
    pub(super) version: String,
    pub(super) architecture: Option<String>,
    pub(super) version_scheme: VersionScheme,
}

impl From<&NativeParityPackageV1> for DebianResolutionRoot {
    fn from(root: &NativeParityPackageV1) -> Self {
        Self {
            package_key_sha256: root.package_key_sha256.clone(),
            source_profile: root.source_profile.clone(),
            name: root.name.clone(),
            version: root.version.clone(),
            architecture: root.architecture.clone(),
            version_scheme: root.version_scheme,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum DebianResolutionWorkerResponse {
    Ready,
    Outcome {
        outcome: NativeResolutionOutcomeV1,
        explanation: Option<NativeResolutionSurveyNativeExplanationV1>,
    },
    Failure {
        error: NativeResolutionWireErrorV1,
        reason: NativeResolutionSurveyErrorReasonV1,
        explanation: NativeResolutionSurveyNativeExplanationV1,
    },
}

pub(super) struct DebianResolutionProcess {
    child: Child,
    input: BufWriter<ChildStdin>,
    output: BufReader<ChildStdout>,
}

impl DebianResolutionProcess {
    pub(super) fn spawn(
        executable: &Path,
        solver_inputs: &[PathBuf],
        package_index: &Path,
        architecture: &str,
    ) -> Result<Self> {
        let mut command = Command::new(executable);
        command
            .arg("--internal-resolution-worker")
            .arg("--architecture")
            .arg(architecture)
            .arg("--package-index")
            .arg(package_index)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        for input in solver_inputs {
            command.arg("--solver-input").arg(input);
        }
        let mut child = command.spawn()?;
        let input = BufWriter::new(child.stdin.take().ok_or_else(|| {
            Error::InternalError("Debian worker stdin was not piped".to_string())
        })?);
        let mut output = BufReader::new(child.stdout.take().ok_or_else(|| {
            Error::InternalError("Debian worker stdout was not piped".to_string())
        })?);
        let response = read_worker_response(&mut output)?;
        if !matches!(response, DebianResolutionWorkerResponse::Ready) {
            return Err(Error::InternalError(
                "Debian worker emitted a root before readiness".to_string(),
            ));
        }
        Ok(Self {
            child,
            input,
            output,
        })
    }

    pub(super) fn resolve(
        &mut self,
        root: &NativeParityPackageV1,
        explanation_byte_limit: u64,
    ) -> Result<NativeRootResolutionResult> {
        let request = DebianResolutionWorkerRequest {
            root: root.into(),
            explanation_byte_limit,
        };
        write_worker_message(&mut self.input, &request)?;
        classify_worker_response(read_worker_response(&mut self.output))
    }
}

fn classify_worker_response(
    response: Result<DebianResolutionWorkerResponse>,
) -> Result<NativeRootResolutionResult> {
    match response? {
        DebianResolutionWorkerResponse::Outcome {
            outcome,
            explanation,
        } => Ok(Ok(NativeRootResolutionSuccess {
            outcome,
            explanation,
        })),
        DebianResolutionWorkerResponse::Failure {
            error,
            reason,
            explanation,
        } => Ok(Err(NativeRootResolutionError::from_wire(
            error,
            reason,
            explanation,
        ))),
        DebianResolutionWorkerResponse::Ready => Err(Error::InternalError(
            "Debian worker repeated readiness".to_string(),
        )),
    }
}

impl Drop for DebianResolutionProcess {
    fn drop(&mut self) {
        let _ = self.input.flush();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn write_worker_message<T: Serialize>(writer: &mut impl Write, value: &T) -> Result<()> {
    serde_json::to_writer(&mut *writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn read_worker_response(
    reader: &mut BufReader<impl std::io::Read>,
) -> Result<DebianResolutionWorkerResponse> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Err(Error::InternalError(
            "Debian resolution worker closed its output".to_string(),
        ));
    }
    serde_json::from_str(&line).map_err(Into::into)
}

pub(super) fn debian_worker_executable() -> Result<PathBuf> {
    let current = std::env::current_exe()?;
    if current.file_stem().is_some_and(|name| {
        name.to_string_lossy()
            .starts_with("conary-debian-resolution-oracle")
    }) {
        return Ok(current);
    }
    let debug = current
        .parent()
        .and_then(Path::parent)
        .map(|parent| parent.join("conary-debian-resolution-oracle"))
        .filter(|candidate| candidate.is_file());
    debug.ok_or_else(|| {
        Error::ConfigError(
            "cannot locate conary-debian-resolution-oracle worker executable".to_string(),
        )
    })
}

pub(in crate::repository::catalog::parity::debian) fn select_debian_worker_launch(
    request: ResolutionWorkerRequest,
    workers: ResolutionWorkerCount,
    executable: Result<PathBuf>,
) -> Result<(ResolutionWorkerCount, Option<PathBuf>)> {
    if workers.get() == 1 {
        return Ok((workers, None));
    }
    match executable {
        Ok(executable) => Ok((workers, Some(executable))),
        Err(_) if request == ResolutionWorkerRequest::Automatic => {
            Ok((ResolutionWorkerCount::new(1)?, None))
        }
        Err(error) => Err(error),
    }
}

/// Run the private line-delimited apt-pkg worker protocol.
pub fn run_debian_resolution_worker(
    solver_inputs: &[PathBuf],
    package_index: &Path,
    architecture: &str,
) -> Result<()> {
    use super::super::ffi::AptResolution;

    let mut apt = AptResolution::open(solver_inputs, architecture)?;
    let package_index = PackageResolutionIndexReader::open(package_index)?;
    let policy = NativeResolutionPolicyV1 {
        architecture: architecture.to_string(),
        architecture_admission: NativeResolutionArchitectureAdmissionV1::NativeOnly,
        installed_state: NativeResolutionInstalledStateV1::Empty,
        roots: NativeResolutionRootPolicyV1::EveryExactPackage,
        positive_requirements: NativeResolutionRequirementPolicyV1::RequiredOnly,
        provider_selection: NativeResolutionProviderPolicyV1::NativePrecedence,
    };
    policy.validate()?;
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut input = stdin.lock();
    let mut output = BufWriter::new(stdout.lock());
    write_worker_message(&mut output, &DebianResolutionWorkerResponse::Ready)?;
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let request: DebianResolutionWorkerRequest = serde_json::from_str(&line)?;
        let response = match resolve_exact_root(
            &mut apt,
            &package_index,
            &request.root,
            &policy,
            request.explanation_byte_limit,
        ) {
            Ok(success) => DebianResolutionWorkerResponse::Outcome {
                outcome: success.outcome,
                explanation: success.explanation,
            },
            Err(failure) => {
                let (error, reason, explanation) = (*failure).into_wire();
                DebianResolutionWorkerResponse::Failure {
                    error,
                    reason,
                    explanation,
                }
            }
        };
        write_worker_message(&mut output, &response)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_failure_is_terminal_instead_of_a_root_failure() {
        let result = classify_worker_response(Err(Error::InternalError(
            "worker transport closed".to_string(),
        )));

        assert!(
            matches!(result, Err(Error::InternalError(message)) if message == "worker transport closed")
        );
    }
}
