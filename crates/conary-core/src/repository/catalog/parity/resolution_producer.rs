// crates/conary-core/src/repository/catalog/parity/resolution_producer.rs

//! Native producer orchestration; ecosystems own staging and root projection.

use std::path::Path;

use super::resolution_parallel::{
    OrderedResolutionMetrics, RESOLUTION_WORKER_RSS_BYTES, ResolutionExplanationLimits,
    ResolutionWalkImplementationEvidenceV1, ResolutionWorkerCount, ResolutionWorkerRequest,
    resolution_walk_memory_budget_bytes, walk_ordered_parallel,
};
use super::resolution_survey::{
    NativeResolutionSurveyCollector, NativeRootResolutionResult, RootOutcomeSink,
};
use super::{
    NATIVE_RESOLUTION_ROOT_FILE_NAME, NativeParityImplementationV1, NativeParityOracleReader,
    NativeParityPackageV1, NativeResolutionArchitectureAdmissionV1,
    NativeResolutionInstalledStateV1, NativeResolutionOracleV1, NativeResolutionOracleWriter,
    NativeResolutionPolicyV1, NativeResolutionProviderPolicyV1,
    NativeResolutionRequirementPolicyV1, NativeResolutionRootPolicyV1, NativeResolutionSurveyV1,
    verify_native_parity_oracle_bundle, verify_native_resolution_oracle_bundle,
    write_native_resolution_oracle_manifest, write_native_resolution_survey,
};
use crate::error::{Error, Result};
use crate::repository::catalog::ProfileRevisionV2;

pub(super) struct ResolutionContext<'a, I> {
    pub profile: &'a ProfileRevisionV2,
    pub inputs: &'a [I],
    pub policy: &'a NativeResolutionPolicyV1,
}

pub(super) trait NativeResolutionEcosystem<'a>: Sized {
    type Input: Sync;
    type Prepared: Sync;
    type Worker;
    const LABEL: &'static str;

    fn prepare(
        context: &ResolutionContext<'_, Self::Input>,
        package_oracle: &NativeParityOracleReader,
    ) -> Result<Self::Prepared>;
    fn implementation() -> NativeParityImplementationV1;
    fn select_workers(
        _prepared: &mut Self::Prepared,
        _request: ResolutionWorkerRequest,
        workers: ResolutionWorkerCount,
    ) -> Result<ResolutionWorkerCount> {
        Ok(workers)
    }
    fn open_worker(
        context: &ResolutionContext<'_, Self::Input>,
        prepared: &Self::Prepared,
    ) -> Result<Self::Worker>;
    fn resolve_root(
        context: &ResolutionContext<'_, Self::Input>,
        worker: &mut Self::Worker,
        root: &NativeParityPackageV1,
        limits: ResolutionExplanationLimits,
    ) -> Result<NativeRootResolutionResult>;
    fn walk(
        context: &ResolutionContext<'_, Self::Input>,
        prepared: &Self::Prepared,
        package_oracle: &NativeParityOracleReader,
        sink: RootOutcomeSink<'_>,
        workers: ResolutionWorkerCount,
    ) -> Result<OrderedResolutionMetrics> {
        walk_resolution_roots::<Self>(context, prepared, package_oracle, sink, workers)
    }
}

pub(super) fn walk_resolution_roots<'a, E: NativeResolutionEcosystem<'a>>(
    context: &ResolutionContext<'_, E::Input>,
    prepared: &E::Prepared,
    package_oracle: &NativeParityOracleReader,
    mut sink: RootOutcomeSink<'_>,
    workers: ResolutionWorkerCount,
) -> Result<OrderedResolutionMetrics> {
    walk_ordered_parallel(
        package_oracle,
        workers,
        sink.explanation_limits(),
        |_| E::open_worker(context, prepared),
        |worker, root, limits| E::resolve_root(context, worker, root, limits),
        |root, result| {
            sink.root(root, result?)?;
            Ok(sink.explanation_limits())
        },
    )
}

pub(super) trait ResolutionDestination {
    type Output;
    type Collector;
    fn open(
        &self,
        profile: &ProfileRevisionV2,
        package_oracle: &NativeParityOracleReader,
        implementation: NativeParityImplementationV1,
        policy: NativeResolutionPolicyV1,
    ) -> Result<Self::Collector>;
    fn sink(collector: &mut Self::Collector) -> RootOutcomeSink<'_>;
    fn finish(
        self,
        collector: Self::Collector,
        profile: &ProfileRevisionV2,
        package_oracle: &NativeParityOracleReader,
        ecosystem: &str,
    ) -> Result<Self::Output>;
}

pub(super) struct Oracle<'a>(pub &'a Path);
pub(super) struct Survey<'a>(pub &'a Path);

impl ResolutionDestination for Oracle<'_> {
    type Output = NativeResolutionOracleV1;
    type Collector = NativeResolutionOracleWriter;

    fn open(
        &self,
        profile: &ProfileRevisionV2,
        package_oracle: &NativeParityOracleReader,
        implementation: NativeParityImplementationV1,
        policy: NativeResolutionPolicyV1,
    ) -> Result<Self::Collector> {
        std::fs::create_dir(self.0)?;
        NativeResolutionOracleWriter::create(
            self.0.join(NATIVE_RESOLUTION_ROOT_FILE_NAME),
            profile,
            package_oracle.manifest(),
            implementation,
            policy,
        )
    }

    fn sink(collector: &mut Self::Collector) -> RootOutcomeSink<'_> {
        RootOutcomeSink::Strict(collector)
    }

    fn finish(
        self,
        collector: Self::Collector,
        profile: &ProfileRevisionV2,
        package_oracle: &NativeParityOracleReader,
        ecosystem: &str,
    ) -> Result<Self::Output> {
        let manifest = collector.finish()?;
        write_native_resolution_oracle_manifest(self.0, &manifest)?;
        let reopened = verify_native_resolution_oracle_bundle(self.0, profile, package_oracle)?;
        if reopened.manifest() != &manifest {
            return Err(Error::InternalError(format!(
                "reopened {ecosystem} resolution manifest differs from produced manifest"
            )));
        }
        Ok(manifest)
    }
}

impl ResolutionDestination for Survey<'_> {
    type Output = NativeResolutionSurveyV1;
    type Collector = NativeResolutionSurveyCollector;

    fn open(
        &self,
        profile: &ProfileRevisionV2,
        package_oracle: &NativeParityOracleReader,
        implementation: NativeParityImplementationV1,
        policy: NativeResolutionPolicyV1,
    ) -> Result<Self::Collector> {
        NativeResolutionSurveyCollector::new(
            profile,
            package_oracle.manifest(),
            implementation,
            policy,
        )
    }

    fn sink(collector: &mut Self::Collector) -> RootOutcomeSink<'_> {
        RootOutcomeSink::Survey(collector)
    }

    fn finish(
        self,
        collector: Self::Collector,
        _profile: &ProfileRevisionV2,
        _package_oracle: &NativeParityOracleReader,
        _ecosystem: &str,
    ) -> Result<Self::Output> {
        let survey = collector.finish()?;
        write_native_resolution_survey(self.0, &survey)?;
        Ok(survey)
    }
}

pub(super) fn produce_resolution<'a, E: NativeResolutionEcosystem<'a>, D: ResolutionDestination>(
    profile: &ProfileRevisionV2,
    inputs: &[E::Input],
    package_oracle_directory: &Path,
    architecture: &str,
    destination: D,
    worker_request: ResolutionWorkerRequest,
) -> Result<(D::Output, ResolutionWalkImplementationEvidenceV1)> {
    let architecture = profile.require_target_architecture(architecture)?;
    let policy = NativeResolutionPolicyV1 {
        architecture: architecture.to_string(),
        architecture_admission: NativeResolutionArchitectureAdmissionV1::NativeOnly,
        installed_state: NativeResolutionInstalledStateV1::Empty,
        roots: NativeResolutionRootPolicyV1::EveryExactPackage,
        positive_requirements: NativeResolutionRequirementPolicyV1::RequiredOnly,
        provider_selection: NativeResolutionProviderPolicyV1::NativePrecedence,
    };
    policy.validate()?;
    let package_oracle = verify_native_parity_oracle_bundle(package_oracle_directory, profile)?;
    let context = ResolutionContext {
        profile,
        inputs,
        policy: &policy,
    };
    let mut prepared = E::prepare(&context, &package_oracle)?;
    let memory_budget_bytes = resolution_walk_memory_budget_bytes()?;
    let workers = worker_request.resolve(
        package_oracle.manifest().artifact.counts.packages,
        memory_budget_bytes,
        RESOLUTION_WORKER_RSS_BYTES,
    )?;
    let workers = E::select_workers(&mut prepared, worker_request, workers)?;
    let mut collector = destination.open(
        profile,
        &package_oracle,
        E::implementation(),
        policy.clone(),
    )?;
    let metrics = E::walk(
        &context,
        &prepared,
        &package_oracle,
        D::sink(&mut collector),
        workers,
    )?;
    let output = destination.finish(collector, profile, &package_oracle, E::LABEL)?;
    let evidence = ResolutionWalkImplementationEvidenceV1::new(
        workers,
        metrics.worker_load_milliseconds,
        memory_budget_bytes,
        RESOLUTION_WORKER_RSS_BYTES,
    )?;
    Ok((output, evidence))
}

// Keep the callable ecosystem entry names while defining their signatures and
// forwarding bodies once. No native behavior belongs in these wrappers.
macro_rules! resolution_producers {
    ($ecosystem:ty, $input:ident, $oracle:ident, $oracle_workers:ident, $survey:ident, $survey_workers:ident) => {
        resolution_producers!(@destination $ecosystem, $input, $oracle, $oracle_workers, Oracle, NativeResolutionOracleV1);
        resolution_producers!(@destination $ecosystem, $input, $survey, $survey_workers, Survey, NativeResolutionSurveyV1);
    };
    (@destination $ecosystem:ty, $input:ident, $automatic:ident, $workers:ident, $destination:ident, $output:ident) => {
        /// Produce native resolution evidence with capacity-derived workers.
        pub fn $automatic(
            profile: &ProfileRevisionV2,
            inputs: &[$input<'_>],
            package_oracle_directory: &Path,
            architecture: &str,
            output: &Path,
        ) -> Result<$output> {
            $workers(profile, inputs, package_oracle_directory, architecture, output, ResolutionWorkerRequest::Automatic).map(|(output, _)| output)
        }

        /// Produce native resolution evidence with the requested workers.
        pub fn $workers(
            profile: &ProfileRevisionV2,
            inputs: &[$input<'_>],
            package_oracle_directory: &Path,
            architecture: &str,
            output: &Path,
            worker_request: ResolutionWorkerRequest,
        ) -> Result<($output, ResolutionWalkImplementationEvidenceV1)> {
            produce_resolution::<$ecosystem, _>(profile, inputs, package_oracle_directory, architecture, $destination(output), worker_request)
        }
    };
}
pub(super) use resolution_producers;
