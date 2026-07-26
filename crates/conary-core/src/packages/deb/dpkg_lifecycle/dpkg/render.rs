// conary-core/src/packages/deb/dpkg_lifecycle/dpkg/render.rs

use super::{DpkgArchiveBackendAction, DpkgDisposition, DpkgMutationAction, DpkgReadOnlyAction};

pub(super) fn disposition_argv(disposition: &DpkgDisposition) -> (Vec<u8>, Vec<Vec<u8>>) {
    match disposition {
        DpkgDisposition::ReadOnly(action) => read_only_argv(action),
        DpkgDisposition::RecursiveMutation(action) => mutation_argv(action),
        DpkgDisposition::ArchiveBackendDelegation(action) => backend_argv(action),
    }
}

fn read_only_argv(action: &DpkgReadOnlyAction) -> (Vec<u8>, Vec<Vec<u8>>) {
    match action {
        DpkgReadOnlyAction::Verify(values) => (b"--verify".to_vec(), values.clone()),
        DpkgReadOnlyAction::Query(action) => (action.option().to_vec(), action.operands()),
        DpkgReadOnlyAction::GetSelections(values) => (b"--get-selections".to_vec(), values.clone()),
        DpkgReadOnlyAction::Audit(values) => (b"--audit".to_vec(), values.clone()),
        DpkgReadOnlyAction::YetToUnpack => (b"--yet-to-unpack".to_vec(), Vec::new()),
        DpkgReadOnlyAction::PredependencyPackage => (b"--predep-package".to_vec(), Vec::new()),
        DpkgReadOnlyAction::PrintArchitecture => (b"--print-architecture".to_vec(), Vec::new()),
        DpkgReadOnlyAction::PrintForeignArchitectures => {
            (b"--print-foreign-architectures".to_vec(), Vec::new())
        }
        DpkgReadOnlyAction::Assert(feature) => {
            let mut action = b"--assert-".to_vec();
            action.extend_from_slice(feature.as_bytes());
            (action, Vec::new())
        }
        DpkgReadOnlyAction::Validate { kind, value } => {
            let mut action = b"--validate-".to_vec();
            action.extend_from_slice(kind.as_bytes());
            (action, vec![value.clone()])
        }
        DpkgReadOnlyAction::CompareVersions {
            left,
            relation,
            right,
        } => (
            b"--compare-versions".to_vec(),
            vec![left.clone(), relation.as_bytes().to_vec(), right.clone()],
        ),
        DpkgReadOnlyAction::Help => (b"--help".to_vec(), Vec::new()),
        DpkgReadOnlyAction::ForceHelp => (b"--force-help".to_vec(), Vec::new()),
        DpkgReadOnlyAction::DebugHelp => (b"--debug=help".to_vec(), Vec::new()),
        DpkgReadOnlyAction::Version => (b"--version".to_vec(), Vec::new()),
    }
}

fn mutation_argv(action: &DpkgMutationAction) -> (Vec<u8>, Vec<Vec<u8>>) {
    match action {
        DpkgMutationAction::Install(values) => (b"--install".to_vec(), values.clone()),
        DpkgMutationAction::Unpack(values) => (b"--unpack".to_vec(), values.clone()),
        DpkgMutationAction::RecordAvailable(values) => (b"--record-avail".to_vec(), values.clone()),
        DpkgMutationAction::Configure(values) => (b"--configure".to_vec(), values.clone()),
        DpkgMutationAction::Remove(values) => (b"--remove".to_vec(), values.clone()),
        DpkgMutationAction::Purge(values) => (b"--purge".to_vec(), values.clone()),
        DpkgMutationAction::ProcessTriggers(values) => {
            (b"--triggers-only".to_vec(), values.clone())
        }
        DpkgMutationAction::SetSelections => (b"--set-selections".to_vec(), Vec::new()),
        DpkgMutationAction::ClearSelections => (b"--clear-selections".to_vec(), Vec::new()),
        DpkgMutationAction::UpdateAvailable(file) => {
            (b"--update-avail".to_vec(), file.iter().cloned().collect())
        }
        DpkgMutationAction::MergeAvailable(file) => {
            (b"--merge-avail".to_vec(), file.iter().cloned().collect())
        }
        DpkgMutationAction::ClearAvailable => (b"--clear-avail".to_vec(), Vec::new()),
        DpkgMutationAction::ForgetOldUnavailable => (b"--forget-old-unavail".to_vec(), Vec::new()),
        DpkgMutationAction::AddArchitecture(value) => {
            (b"--add-architecture".to_vec(), vec![value.clone()])
        }
        DpkgMutationAction::RemoveArchitecture(value) => {
            (b"--remove-architecture".to_vec(), vec![value.clone()])
        }
    }
}

fn backend_argv(action: &DpkgArchiveBackendAction) -> (Vec<u8>, Vec<Vec<u8>>) {
    match action {
        DpkgArchiveBackendAction::Build(values) => (b"--build".to_vec(), values.clone()),
        DpkgArchiveBackendAction::Contents(value) => (b"--contents".to_vec(), vec![value.clone()]),
        DpkgArchiveBackendAction::Control(values) => (b"--control".to_vec(), values.clone()),
        DpkgArchiveBackendAction::Extract { archive, directory } => (
            b"--extract".to_vec(),
            vec![archive.clone(), directory.clone()],
        ),
        DpkgArchiveBackendAction::VerboseExtract { archive, directory } => (
            b"--vextract".to_vec(),
            vec![archive.clone(), directory.clone()],
        ),
        DpkgArchiveBackendAction::Field(values) => (b"--field".to_vec(), values.clone()),
        DpkgArchiveBackendAction::ControlTar(value) => {
            (b"--ctrl-tarfile".to_vec(), vec![value.clone()])
        }
        DpkgArchiveBackendAction::FilesystemTar(value) => {
            (b"--fsys-tarfile".to_vec(), vec![value.clone()])
        }
        DpkgArchiveBackendAction::Info(values) => (b"--info".to_vec(), values.clone()),
    }
}
