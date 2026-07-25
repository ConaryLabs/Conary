// apps/conary/src/commands/try_session/watch.rs
//! Watch-mode try-session orchestration.

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use conary_core::ccs::{SigningKeyPair, TrustPolicy};
use conary_core::diagnostics::{
    DiagnosticEvidence, PACKAGING_JSON_SCHEMA_VERSION, PackagingCommandOutput,
    PackagingCommandStatus, PackagingDiagnostic, PackagingDiagnosticCode, PackagingEvent,
    PackagingEventKind, PackagingPhase,
};
use conary_core::runtime_root::ConaryRuntimeRoot;

use crate::commands::cook::{
    CookForTryWatchOptions, WatchCookSourcePolicy, cooked_artifact_path, run_cook_for_try_watch,
};

use super::watch_source::{
    DebounceState, WatchIdentity, WatchSourceSet, compute_watch_identity, resolve_watch_source_set,
};
use super::{
    TryRefreshRequest, TryStartRequest, TryWatchMarkerRequest, begin_try_session,
    refresh_try_session,
};

mod reporting;

use reporting::{
    WatchFailure, emit_diagnostic, emit_non_destructive_failure, emit_push, fail_before_session,
    finish_current_events, finish_watch, remove_dir_if_exists, rollback_or_fail, watch_error,
    write_optional_file,
};

const WATCH_EVENT_RECORD_LIMIT: usize = 500;
const DEFAULT_DEBOUNCE_MS: u64 = 750;
const DEFAULT_POLL_MS: u64 = 500;
const DEFAULT_WATCH_SOURCE_CACHE: &str = "/var/cache/conary/sources";

pub(super) struct TryWatchOptions<'a> {
    pub(super) db_path: &'a str,
    pub(super) target: &'a str,
    pub(super) recipe: Option<&'a str>,
    pub(super) signing_key_path: &'a std::path::Path,
    pub(super) isolated: bool,
    pub(super) json: bool,
}

struct WatchEvents {
    operation_id: String,
    next_sequence: u64,
    events: Vec<PackagingEvent>,
    diagnostics: Vec<PackagingDiagnostic>,
}

impl WatchEvents {
    fn new(operation_id: impl Into<String>) -> Self {
        Self {
            operation_id: operation_id.into(),
            next_sequence: 1,
            events: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn push(
        &mut self,
        phase: PackagingPhase,
        kind: PackagingEventKind,
        message: impl Into<String>,
    ) -> PackagingEvent {
        let event = PackagingEvent {
            schema_version: PACKAGING_JSON_SCHEMA_VERSION,
            operation_id: self.operation_id.clone(),
            sequence: self.next_sequence,
            phase,
            kind,
            message: Some(message.into()),
            diagnostic: None,
            artifact: None,
            progress: None,
        };
        self.next_sequence += 1;
        self.events.push(event.clone());
        event
    }

    fn diagnostic(&mut self, diagnostic: PackagingDiagnostic) -> PackagingEvent {
        let event = PackagingEvent::diagnostic(
            self.operation_id.clone(),
            self.next_sequence,
            diagnostic.clone(),
        );
        self.next_sequence += 1;
        self.diagnostics.push(diagnostic);
        self.events.push(event.clone());
        event
    }

    fn operation_finished(&mut self, summary: impl Into<String>) -> PackagingEvent {
        self.push(
            PackagingPhase::OperationRecord,
            PackagingEventKind::OperationFinished,
            summary,
        )
    }

    #[cfg(test)]
    fn all(&self) -> &[PackagingEvent] {
        &self.events
    }

    fn into_command_output(
        self,
        status: PackagingCommandStatus,
        summary: impl Into<String>,
    ) -> PackagingCommandOutput {
        let events = crate::commands::diagnostics::bounded_watch_events(
            &self.operation_id,
            &self.events,
            WATCH_EVENT_RECORD_LIMIT,
        );
        PackagingCommandOutput {
            schema_version: PACKAGING_JSON_SCHEMA_VERSION,
            operation_id: self.operation_id,
            command: "conary try --watch".to_string(),
            status,
            diagnostics: self.diagnostics,
            events,
            artifacts: Vec::new(),
            summary: Some(summary.into()),
        }
    }
}

struct WatchRefreshState {
    last_successful_identity: WatchIdentity,
    last_attempted_identity: Option<WatchIdentity>,
    pending_identity: Option<WatchIdentity>,
    last_good_generation_id: i64,
}

impl WatchRefreshState {
    fn new(initial_identity: WatchIdentity, last_good_generation_id: i64) -> Self {
        Self {
            last_successful_identity: initial_identity,
            last_attempted_identity: None,
            pending_identity: None,
            last_good_generation_id,
        }
    }

    fn should_attempt(&self, current: &WatchIdentity) -> bool {
        current != &self.last_successful_identity
            && self.last_attempted_identity.as_ref() != Some(current)
    }

    fn record_attempt(&mut self, identity: WatchIdentity) {
        self.pending_identity = None;
        self.last_attempted_identity = Some(identity);
    }

    fn record_success(&mut self, identity: WatchIdentity, generation_id: i64) {
        self.last_successful_identity = identity.clone();
        self.pending_identity = None;
        self.last_attempted_identity = Some(identity);
        self.last_good_generation_id = generation_id;
    }

    fn record_failure(&mut self) {
        self.pending_identity = None;
        self.last_attempted_identity = None;
    }

    fn clear_pending(&mut self) {
        self.pending_identity = None;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DebouncedRefresh {
    Waiting,
    Ready,
}

fn debounce_refresh_identity(
    state: &mut WatchRefreshState,
    debounce: &mut DebounceState,
    identity: WatchIdentity,
    now: Instant,
) -> DebouncedRefresh {
    if state.pending_identity.as_ref() != Some(&identity) {
        state.pending_identity = Some(identity);
        debounce.record_wakeup(now);
        return DebouncedRefresh::Waiting;
    }

    if debounce.take_ready(now).is_some() {
        state.clear_pending();
        DebouncedRefresh::Ready
    } else {
        DebouncedRefresh::Waiting
    }
}

struct WatchLoopConfig {
    poll_interval: Duration,
    debounce: Duration,
    max_refreshes: Option<usize>,
    exit_after_ready: bool,
    ready_file: Option<PathBuf>,
    failure_file: Option<PathBuf>,
}

impl WatchLoopConfig {
    fn from_env() -> Self {
        let max_refreshes = std::env::var("CONARY_TEST_TRY_WATCH_EXIT_AFTER_REFRESHES")
            .ok()
            .and_then(|value| value.parse::<usize>().ok());
        Self {
            poll_interval: Duration::from_millis(DEFAULT_POLL_MS),
            debounce: Duration::from_millis(DEFAULT_DEBOUNCE_MS),
            max_refreshes,
            exit_after_ready: std::env::var_os("CONARY_TEST_TRY_WATCH_EXIT_AFTER_READY").is_some(),
            ready_file: std::env::var_os("CONARY_TEST_TRY_WATCH_READY_FILE").map(PathBuf::from),
            failure_file: std::env::var_os("CONARY_TEST_TRY_WATCH_FAILURE_FILE").map(PathBuf::from),
        }
    }
}

struct WatchCookRequest {
    target: String,
    recipe: Option<String>,
    output_dir: PathBuf,
    source_cache: PathBuf,
    signing_key_path: PathBuf,
    isolated: bool,
    source_policy: WatchCookSourcePolicy,
    operation_id: String,
}

enum WatchCookCompletion {
    Completed(Result<PackagingCommandOutput>),
    Cancelled,
}

pub(super) async fn cmd_try_watch(options: TryWatchOptions<'_>) -> Result<()> {
    let mut output = io::stdout();
    cmd_try_watch_with_output(options, WatchLoopConfig::from_env(), &mut output).await
}

async fn cmd_try_watch_with_output(
    options: TryWatchOptions<'_>,
    config: WatchLoopConfig,
    output: &mut impl Write,
) -> Result<()> {
    let signing_key =
        SigningKeyPair::load_from_file(options.signing_key_path).with_context(|| {
            format!(
                "failed to load try watch signing key {}",
                options.signing_key_path.display()
            )
        })?;
    let trust_policy = TrustPolicy::strict(vec![signing_key.public_key_base64()]);
    let operation_id = crate::commands::operation_records::new_operation_id("try-watch");
    let runtime_root = ConaryRuntimeRoot::from_db_path(PathBuf::from(options.db_path));
    let cook_base_dir = runtime_root
        .root()
        .join("try")
        .join("watch-cook")
        .join(&operation_id);
    let source_cache = watch_source_cache();
    let mut events = WatchEvents::new(operation_id.clone());
    let mut refresh_count = 0usize;

    fs::create_dir_all(&cook_base_dir).with_context(|| {
        format!(
            "failed to create try watch cook directory {}",
            cook_base_dir.display()
        )
    })?;

    emit_push(
        &mut events,
        PackagingPhase::TrySession,
        PackagingEventKind::WatchStarted,
        format!("Watching {}", options.target),
        options.json,
        output,
    )?;

    let source_set = match resolve_watch_source_set(Some(options.target), options.recipe) {
        Ok(source_set) => source_set,
        Err(error) => {
            return fail_before_session(
                events,
                PackagingPhase::RecipeValidation,
                PackagingDiagnosticCode::WatchSourceIdentityFailed,
                "failed to resolve watched sources",
                error,
                options.json,
                output,
            );
        }
    };
    let mut initial_identity = match compute_watch_identity(&source_set) {
        Ok(identity) => identity,
        Err(error) => {
            return fail_before_session(
                events,
                PackagingPhase::RecipeValidation,
                PackagingDiagnosticCode::WatchSourceIdentityFailed,
                "failed to compute watched source identity",
                error,
                options.json,
                output,
            );
        }
    };

    loop {
        let initial_output_dir = cook_base_dir.join(format!("initial-{refresh_count}"));
        emit_push(
            &mut events,
            PackagingPhase::Build,
            PackagingEventKind::WatchRefreshStarted,
            "Initial cook for try watch",
            options.json,
            output,
        )?;
        let cook = run_watch_cook_cancellable(
            WatchCookRequest {
                target: options.target.to_string(),
                recipe: options.recipe.map(ToOwned::to_owned),
                output_dir: initial_output_dir.clone(),
                source_cache: source_cache.clone(),
                signing_key_path: options.signing_key_path.to_path_buf(),
                isolated: options.isolated,
                source_policy: WatchCookSourcePolicy::Initial,
                operation_id: operation_id.clone(),
            },
            &mut events,
            options.json,
            output,
        )
        .await?;
        let cook_output = match cook {
            WatchCookCompletion::Completed(Ok(output)) => output,
            WatchCookCompletion::Completed(Err(error)) => {
                return fail_before_session(
                    events,
                    PackagingPhase::Build,
                    PackagingDiagnosticCode::WatchCookFailed,
                    "initial try watch cook failed",
                    error,
                    options.json,
                    output,
                );
            }
            WatchCookCompletion::Cancelled => {
                return finish_watch(
                    events,
                    PackagingCommandStatus::Succeeded,
                    "try watch cancelled before startup completed",
                    options.json,
                    output,
                );
            }
        };

        let after_source_set = match resolve_watch_source_set(Some(options.target), options.recipe)
        {
            Ok(source_set) => source_set,
            Err(error) => {
                return fail_before_session(
                    events,
                    PackagingPhase::RecipeValidation,
                    PackagingDiagnosticCode::WatchSourceIdentityFailed,
                    "failed to resolve watched sources after initial cook",
                    error,
                    options.json,
                    output,
                );
            }
        };
        let after_identity = match compute_watch_identity(&after_source_set) {
            Ok(identity) => identity,
            Err(error) => {
                return fail_before_session(
                    events,
                    PackagingPhase::RecipeValidation,
                    PackagingDiagnosticCode::WatchSourceIdentityFailed,
                    "failed to compute watched source identity after initial cook",
                    error,
                    options.json,
                    output,
                );
            }
        };
        if after_identity == initial_identity {
            let artifact_path = match cooked_artifact_path(&cook_output) {
                Ok(path) => path,
                Err(error) => {
                    return fail_before_session(
                        events,
                        PackagingPhase::Build,
                        PackagingDiagnosticCode::WatchCookFailed,
                        "initial try watch cook did not produce one CCS artifact",
                        error,
                        options.json,
                        output,
                    );
                }
            };
            let started = match begin_try_session(TryStartRequest {
                db_path: options.db_path,
                package_path: &artifact_path,
                trust_policy: &trust_policy,
                activate: false,
                command: None,
                watch_marker: Some(TryWatchMarkerRequest {
                    operation_id: &operation_id,
                }),
            }) {
                Ok(started) => started,
                Err(error) => {
                    return fail_before_session(
                        events,
                        PackagingPhase::TrySession,
                        PackagingDiagnosticCode::TryWatchUnsupported,
                        "failed to start try watch session",
                        error,
                        options.json,
                        output,
                    );
                }
            };
            emit_push(
                &mut events,
                PackagingPhase::TrySession,
                PackagingEventKind::WatchRefreshSucceeded,
                format!(
                    "Try watch session {} is active on generation {}",
                    started.session_id, started.try_generation_id
                ),
                options.json,
                output,
            )?;
            write_optional_file(&config.ready_file, "ready")?;

            if config.exit_after_ready {
                rollback_or_fail(
                    options.db_path,
                    &mut events,
                    "try watch startup check rolled back",
                    options.json,
                    output,
                )?;
                return finish_watch(
                    events,
                    PackagingCommandStatus::Succeeded,
                    "try watch startup check complete",
                    options.json,
                    output,
                );
            }

            let mut state = WatchRefreshState::new(after_identity, started.try_generation_id);
            let mut debounce = DebounceState::new(config.debounce);
            return run_refresh_loop(
                options,
                config,
                started.session_id,
                operation_id,
                cook_base_dir,
                source_cache,
                after_source_set,
                &trust_policy,
                &mut state,
                &mut debounce,
                &mut refresh_count,
                &mut events,
                output,
            )
            .await;
        }

        initial_identity = after_identity;
        emit_push(
            &mut events,
            PackagingPhase::Build,
            PackagingEventKind::WatchRefreshSkipped,
            "source changed during cook; skipping stale artifact",
            options.json,
            output,
        )?;
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_refresh_loop(
    options: TryWatchOptions<'_>,
    config: WatchLoopConfig,
    session_id: String,
    operation_id: String,
    cook_base_dir: PathBuf,
    source_cache: PathBuf,
    mut last_source_set: WatchSourceSet,
    trust_policy: &TrustPolicy,
    state: &mut WatchRefreshState,
    debounce: &mut DebounceState,
    refresh_count: &mut usize,
    events: &mut WatchEvents,
    output: &mut impl Write,
) -> Result<()> {
    if config.max_refreshes == Some(0) {
        rollback_or_fail(
            options.db_path,
            events,
            "try watch stopped before refresh",
            options.json,
            output,
        )?;
        return finish_current_events(
            events,
            PackagingCommandStatus::Succeeded,
            "try watch stopped before refresh",
            options.json,
            output,
        );
    }

    loop {
        tokio::select! {
            signal = tokio::signal::ctrl_c() => {
                signal.context("failed to listen for Ctrl-C")?;
                emit_push(
                    events,
                    PackagingPhase::TrySession,
                    PackagingEventKind::WatchCancelled,
                    "try watch cancellation requested",
                    options.json,
                    output,
                )?;
                rollback_or_fail(
                    options.db_path,
                    events,
                    "try watch cancelled and rolled back",
                    options.json,
                    output,
                )?;
                return finish_current_events(
                    events,
                    PackagingCommandStatus::Succeeded,
                    "try watch cancelled and rolled back",
                    options.json,
                    output,
                );
            }
            _ = tokio::time::sleep(config.poll_interval) => {}
        }

        let current_source_set = match resolve_watch_source_set(
            Some(options.target),
            options.recipe,
        ) {
            Ok(source_set) => source_set,
            Err(error) => {
                if watched_sources_missing(&last_source_set) {
                    emit_diagnostic(
                        events,
                        watch_error(
                            PackagingPhase::RecipeValidation,
                            PackagingDiagnosticCode::WatchSourceIdentityFailed,
                            "watched source root is no longer available",
                            &error,
                        ),
                        options.json,
                        output,
                    )?;
                    rollback_or_fail(
                        options.db_path,
                        events,
                        "try watch stopped after watched source disappeared",
                        options.json,
                        output,
                    )?;
                    finish_current_events(
                        events,
                        PackagingCommandStatus::Failed,
                        "try watch stopped after watched source disappeared",
                        options.json,
                        output,
                    )?;
                    return Err(error);
                }
                emit_non_destructive_failure(
                    events,
                    &config,
                    WatchFailure {
                        phase: PackagingPhase::RecipeValidation,
                        code: PackagingDiagnosticCode::WatchSourceIdentityFailed,
                        message: "failed to resolve watched sources; keeping last successful generation",
                        error: &error,
                    },
                    options.json,
                    output,
                )?;
                continue;
            }
        };
        let current_identity = match compute_watch_identity(&current_source_set) {
            Ok(identity) => identity,
            Err(error) => {
                if watched_sources_missing(&current_source_set) {
                    emit_diagnostic(
                        events,
                        watch_error(
                            PackagingPhase::RecipeValidation,
                            PackagingDiagnosticCode::WatchSourceIdentityFailed,
                            "watched source root is no longer available",
                            &error,
                        ),
                        options.json,
                        output,
                    )?;
                    rollback_or_fail(
                        options.db_path,
                        events,
                        "try watch stopped after watched source disappeared",
                        options.json,
                        output,
                    )?;
                    finish_current_events(
                        events,
                        PackagingCommandStatus::Failed,
                        "try watch stopped after watched source disappeared",
                        options.json,
                        output,
                    )?;
                    return Err(error);
                }
                emit_non_destructive_failure(
                    events,
                    &config,
                    WatchFailure {
                        phase: PackagingPhase::RecipeValidation,
                        code: PackagingDiagnosticCode::WatchSourceIdentityFailed,
                        message: "failed to compute watched source identity; keeping last successful generation",
                        error: &error,
                    },
                    options.json,
                    output,
                )?;
                continue;
            }
        };
        if !state.should_attempt(&current_identity) {
            state.clear_pending();
            debounce.clear();
            continue;
        }

        let now = Instant::now();
        let pending_changed = state.pending_identity.as_ref() != Some(&current_identity);
        if debounce_refresh_identity(state, debounce, current_identity.clone(), now)
            == DebouncedRefresh::Waiting
        {
            if pending_changed {
                emit_push(
                    events,
                    PackagingPhase::Build,
                    PackagingEventKind::WatchDebounced,
                    "Change detected; waiting for sources to settle",
                    options.json,
                    output,
                )?;
            }
            continue;
        }
        if pending_changed {
            emit_push(
                events,
                PackagingPhase::Build,
                PackagingEventKind::WatchDebounced,
                "Change detected; waiting for sources to settle",
                options.json,
                output,
            )?;
        }

        state.record_attempt(current_identity.clone());
        emit_push(
            events,
            PackagingPhase::Build,
            PackagingEventKind::WatchRefreshStarted,
            format!(
                "Refreshing try session from {} watched files",
                current_identity.file_count
            ),
            options.json,
            output,
        )?;
        let output_dir = cook_base_dir.join(format!("refresh-{}", *refresh_count + 1));
        let cook = run_watch_cook_cancellable(
            WatchCookRequest {
                target: options.target.to_string(),
                recipe: options.recipe.map(ToOwned::to_owned),
                output_dir: output_dir.clone(),
                source_cache: source_cache.clone(),
                signing_key_path: options.signing_key_path.to_path_buf(),
                isolated: options.isolated,
                source_policy: WatchCookSourcePolicy::Refresh,
                operation_id: operation_id.clone(),
            },
            events,
            options.json,
            output,
        )
        .await?;
        let cook_output = match cook {
            WatchCookCompletion::Completed(Ok(output)) => output,
            WatchCookCompletion::Completed(Err(error)) => {
                state.record_failure();
                emit_non_destructive_failure(
                    events,
                    &config,
                    WatchFailure {
                        phase: PackagingPhase::Build,
                        code: PackagingDiagnosticCode::WatchCookFailed,
                        message: "refresh cook failed; keeping last successful generation",
                        error: &error,
                    },
                    options.json,
                    output,
                )?;
                continue;
            }
            WatchCookCompletion::Cancelled => {
                rollback_or_fail(
                    options.db_path,
                    events,
                    "try watch cancelled and rolled back",
                    options.json,
                    output,
                )?;
                return finish_current_events(
                    events,
                    PackagingCommandStatus::Succeeded,
                    "try watch cancelled and rolled back",
                    options.json,
                    output,
                );
            }
        };

        let after_source_set = match resolve_watch_source_set(Some(options.target), options.recipe)
        {
            Ok(source_set) => source_set,
            Err(error) => {
                state.record_failure();
                emit_non_destructive_failure(
                    events,
                    &config,
                    WatchFailure {
                        phase: PackagingPhase::RecipeValidation,
                        code: PackagingDiagnosticCode::WatchSourceIdentityFailed,
                        message: "failed to resolve watched sources after cook; keeping last successful generation",
                        error: &error,
                    },
                    options.json,
                    output,
                )?;
                continue;
            }
        };
        let after_identity = match compute_watch_identity(&after_source_set) {
            Ok(identity) => identity,
            Err(error) => {
                state.record_failure();
                emit_non_destructive_failure(
                    events,
                    &config,
                    WatchFailure {
                        phase: PackagingPhase::RecipeValidation,
                        code: PackagingDiagnosticCode::WatchSourceIdentityFailed,
                        message: "failed to compute watched source identity after cook; keeping last successful generation",
                        error: &error,
                    },
                    options.json,
                    output,
                )?;
                continue;
            }
        };
        last_source_set = after_source_set.clone();
        if after_identity != current_identity {
            emit_push(
                events,
                PackagingPhase::Build,
                PackagingEventKind::WatchRefreshSkipped,
                "source changed during cook; skipping stale artifact",
                options.json,
                output,
            )?;
            continue;
        }

        let artifact_path = match cooked_artifact_path(&cook_output) {
            Ok(path) => path,
            Err(error) => {
                state.record_failure();
                emit_non_destructive_failure(
                    events,
                    &config,
                    WatchFailure {
                        phase: PackagingPhase::Build,
                        code: PackagingDiagnosticCode::WatchCookFailed,
                        message: "refresh cook did not produce one CCS artifact; keeping last successful generation",
                        error: &error,
                    },
                    options.json,
                    output,
                )?;
                continue;
            }
        };
        let refreshed = match refresh_try_session(TryRefreshRequest {
            db_path: options.db_path,
            session_id: &session_id,
            expected_try_generation_id: state.last_good_generation_id,
            package_path: &artifact_path,
            trust_policy: &trust_policy,
        }) {
            Ok(refreshed) => refreshed,
            Err(error) => {
                state.record_failure();
                let message = error.to_string();
                let code = PackagingDiagnosticCode::WatchTryRefreshFailed;
                emit_diagnostic(
                    events,
                    watch_error(
                        PackagingPhase::TrySession,
                        code,
                        "try watch refresh failed; keeping last successful generation",
                        &error,
                    ),
                    options.json,
                    output,
                )?;
                write_optional_file(&config.failure_file, &message)?;
                if message.contains("changed outside the watcher") {
                    finish_current_events(
                        events,
                        PackagingCommandStatus::Failed,
                        "try watch stopped because the session changed outside the watcher",
                        options.json,
                        output,
                    )?;
                    return Err(error);
                }
                continue;
            }
        };
        state.record_success(after_identity, refreshed.try_generation_id);
        emit_push(
            events,
            PackagingPhase::TrySession,
            PackagingEventKind::WatchRefreshSucceeded,
            format!(
                "Refreshed try session generation {}",
                refreshed.try_generation_id
            ),
            options.json,
            output,
        )?;
        *refresh_count += 1;

        if let Some(cleanup_error) = refreshed.cleanup_error {
            emit_diagnostic(
                events,
                PackagingDiagnostic::error(
                    PackagingPhase::TrySession,
                    PackagingDiagnosticCode::WatchCleanupFailed,
                    "try watch refresh committed but cleanup failed; run `conary try status` and `conary try rollback`",
                )
                .with_evidence(DiagnosticEvidence::log("cleanup error", cleanup_error)),
                options.json,
                output,
            )?;
            return finish_current_events(
                events,
                PackagingCommandStatus::Failed,
                "try watch stopped after committed cleanup failure",
                options.json,
                output,
            );
        }

        if config
            .max_refreshes
            .is_some_and(|max| *refresh_count >= max)
        {
            rollback_or_fail(
                options.db_path,
                events,
                "try watch reached configured refresh limit",
                options.json,
                output,
            )?;
            return finish_current_events(
                events,
                PackagingCommandStatus::Succeeded,
                "try watch reached configured refresh limit and rolled back",
                options.json,
                output,
            );
        }
    }
}

async fn run_watch_cook_cancellable(
    request: WatchCookRequest,
    events: &mut WatchEvents,
    json: bool,
    output: &mut impl Write,
) -> Result<WatchCookCompletion> {
    let output_dir = request.output_dir.clone();
    let mut handle = tokio::task::spawn_blocking(move || run_watch_cook(request));
    tokio::select! {
        joined = &mut handle => {
            Ok(WatchCookCompletion::Completed(
                joined.context("try watch cook task failed")?,
            ))
        }
        signal = tokio::signal::ctrl_c() => {
            signal.context("failed to listen for Ctrl-C")?;
            emit_push(
                events,
                PackagingPhase::TrySession,
                PackagingEventKind::WatchCancelled,
                "try watch cancellation requested; waiting for in-process cook to finish",
                json,
                output,
            )?;
            let _ = handle.await.context("try watch cook task failed after cancellation")?;
            remove_dir_if_exists(&output_dir)?;
            Ok(WatchCookCompletion::Cancelled)
        }
    }
}

fn run_watch_cook(request: WatchCookRequest) -> Result<PackagingCommandOutput> {
    if std::env::var_os("CONARY_TEST_TRY_WATCH_PAUSE_DURING_COOK").is_some()
        && request.source_policy == WatchCookSourcePolicy::Refresh
    {
        std::thread::sleep(Duration::from_millis(1200));
    }
    let output_dir = request.output_dir.to_string_lossy().into_owned();
    let source_cache = request.source_cache.to_string_lossy().into_owned();
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create watch cook output directory {output_dir}"))?;
    run_cook_for_try_watch(CookForTryWatchOptions {
        target: Some(request.target.as_str()),
        recipe: request.recipe.as_deref(),
        output_dir: &output_dir,
        source_cache: &source_cache,
        jobs: None,
        keep_builddir: false,
        isolated: request.isolated,
        source_policy: request.source_policy,
        operation_id: request.operation_id,
        signing_key_path: Some(&request.signing_key_path),
    })
}

fn watch_source_cache() -> PathBuf {
    std::env::var_os("CONARY_TRY_WATCH_SOURCE_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_WATCH_SOURCE_CACHE))
}

fn watched_sources_missing(source_set: &WatchSourceSet) -> bool {
    source_set.local_roots.iter().any(|path| !path.exists())
}

#[cfg(test)]
mod tests;
