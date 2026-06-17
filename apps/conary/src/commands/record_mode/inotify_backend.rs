// apps/conary/src/commands/record_mode/inotify_backend.rs

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use conary_core::recipe::recording::{
    ScopeRoot, SelectedBackend, TraceOperation, TraceScope as ReportScope,
};
use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask};
use walkdir::WalkDir;

use super::trace::{
    RawTraceEvent, TraceBackend, TraceBackendStatus, TraceDrain, TraceLimitation, TraceScope,
    TraceSession,
};
use super::types::RequestedRecordBackend;

pub(crate) struct InotifyTraceBackend;

impl InotifyTraceBackend {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl TraceBackend for InotifyTraceBackend {
    fn probe(
        &self,
        _scope: &TraceScope,
        _requested: RequestedRecordBackend,
    ) -> Result<TraceBackendStatus> {
        Ok(TraceBackendStatus::selected(
            SelectedBackend::Inotify,
            vec![TraceLimitation::IncompleteReadEvidence],
        ))
    }

    fn start(&self, scope: TraceScope) -> Result<Box<dyn TraceSession>> {
        let directory_count = count_directories(&scope)?;
        if let Some(max_user_watches) = read_max_user_watches()?
            && directory_count > max_user_watches
        {
            bail!(
                "record-mode inotify needs {directory_count} watches, exceeding max_user_watches={max_user_watches}"
            );
        }

        let mut inotify = Inotify::init().context("failed to initialize inotify")?;
        let mut watches = HashMap::new();
        for root in scope.roots() {
            add_recursive_watches(&mut inotify, &mut watches, root, &root.root)?;
        }

        Ok(Box::new(InotifyTraceSession {
            inotify,
            watches,
            buffer: vec![0; 64 * 1024],
        }))
    }
}

struct InotifyTraceSession {
    inotify: Inotify,
    watches: HashMap<WatchDescriptor, WatchedDirectory>,
    buffer: Vec<u8>,
}

#[derive(Debug, Clone)]
struct WatchedDirectory {
    scope: ReportScope,
    root: ScopeRoot,
    path: PathBuf,
}

impl TraceSession for InotifyTraceSession {
    fn drain_events(&mut self) -> Result<TraceDrain> {
        let mut drain = TraceDrain::default();
        let mut new_directories = Vec::new();
        let events = self
            .inotify
            .read_events(&mut self.buffer)
            .context("failed to read inotify events")?;

        for event in events {
            if event.mask.contains(EventMask::Q_OVERFLOW) {
                drain.event_loss = true;
                continue;
            }
            let Some(directory) = self.watches.get(&event.wd).cloned() else {
                drain.ignored_events += 1;
                continue;
            };
            let Some(name) = event.name else {
                drain.ignored_events += 1;
                continue;
            };
            let path = directory.path.join(name);
            let operation = operation_for_mask(event.mask, directory.scope);
            let observed = directory.root.scope_path(&path, operation)?;
            drain.events.push(RawTraceEvent {
                path: path.clone(),
                observed,
            });

            if event.mask.contains(EventMask::ISDIR)
                && (event.mask.contains(EventMask::CREATE)
                    || event.mask.contains(EventMask::MOVED_TO))
                && path.is_dir()
            {
                new_directories.push((directory.root, path));
            }
        }

        for (root, path) in new_directories {
            add_recursive_watches(&mut self.inotify, &mut self.watches, &root, &path)?;
        }

        Ok(drain)
    }

    fn finish(&mut self) -> Result<TraceDrain> {
        self.drain_events()
    }
}

fn count_directories(scope: &TraceScope) -> Result<usize> {
    let mut count = 0;
    for root in scope.roots() {
        for entry in WalkDir::new(&root.root).follow_links(false) {
            let entry = entry?;
            if entry.file_type().is_dir() {
                count += 1;
            }
        }
    }
    Ok(count)
}

fn read_max_user_watches() -> Result<Option<usize>> {
    let path = Path::new("/proc/sys/fs/inotify/max_user_watches");
    if !path.exists() {
        return Ok(None);
    }
    let value = fs::read_to_string(path)?;
    let value = value.trim().parse::<usize>()?;
    Ok(Some(value))
}

fn add_recursive_watches(
    inotify: &mut Inotify,
    watches: &mut HashMap<WatchDescriptor, WatchedDirectory>,
    root: &ScopeRoot,
    start: &Path,
) -> Result<()> {
    for entry in WalkDir::new(start).follow_links(false) {
        let entry = entry?;
        if entry.file_type().is_dir() {
            add_watch(inotify, watches, root, entry.path())?;
        }
    }
    Ok(())
}

fn add_watch(
    inotify: &mut Inotify,
    watches: &mut HashMap<WatchDescriptor, WatchedDirectory>,
    root: &ScopeRoot,
    path: &Path,
) -> Result<()> {
    let descriptor = inotify
        .watches()
        .add(
            path,
            WatchMask::CREATE
                | WatchMask::MODIFY
                | WatchMask::DELETE
                | WatchMask::MOVED_FROM
                | WatchMask::MOVED_TO
                | WatchMask::ATTRIB,
        )
        .with_context(|| format!("failed to add inotify watch for {}", path.display()))?;
    watches.insert(
        descriptor,
        WatchedDirectory {
            scope: root.scope,
            root: root.clone(),
            path: path.to_path_buf(),
        },
    );
    Ok(())
}

fn operation_for_mask(mask: EventMask, scope: ReportScope) -> TraceOperation {
    if mask.contains(EventMask::DELETE) || mask.contains(EventMask::MOVED_FROM) {
        return match scope {
            ReportScope::Install => TraceOperation::InstallDelete,
            ReportScope::Source => TraceOperation::SourceWrite,
            ReportScope::Work => TraceOperation::WorkWrite,
        };
    }
    if mask.contains(EventMask::CREATE)
        || mask.contains(EventMask::MOVED_TO)
        || mask.contains(EventMask::MODIFY)
        || mask.contains(EventMask::ATTRIB)
    {
        return match scope {
            ReportScope::Install => {
                if mask.contains(EventMask::CREATE) || mask.contains(EventMask::MOVED_TO) {
                    TraceOperation::InstallCreate
                } else {
                    TraceOperation::InstallModify
                }
            }
            ReportScope::Source => TraceOperation::SourceWrite,
            ReportScope::Work => TraceOperation::WorkWrite,
        };
    }
    TraceOperation::OutOfScope
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(temp: &tempfile::TempDir) -> super::super::trace::TraceScope {
        let source = temp.path().join("source");
        let work = temp.path().join("work");
        let install = temp.path().join("install");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(&install).unwrap();
        super::super::trace::TraceScope {
            source: ScopeRoot::new(ReportScope::Source, &source).unwrap(),
            work: ScopeRoot::new(ReportScope::Work, &work).unwrap(),
            install: ScopeRoot::new(ReportScope::Install, &install).unwrap(),
        }
    }

    #[test]
    fn recursive_inotify_records_create_modify_delete_in_preexisting_directory() {
        let temp = tempfile::tempdir().unwrap();
        let scope = scope(&temp);
        let install_dir = temp.path().join("install/usr/bin");
        std::fs::create_dir_all(&install_dir).unwrap();
        let backend = InotifyTraceBackend::new();
        let mut session = backend.start(scope).unwrap();

        let install_file = install_dir.join("demo");
        std::fs::write(&install_file, "one").unwrap();
        std::fs::write(&install_file, "two").unwrap();
        std::fs::remove_file(&install_file).unwrap();

        let drain = session.finish().unwrap();
        assert!(
            drain
                .events
                .iter()
                .any(|event| event.observed.path == "usr/bin/demo")
        );
        assert!(
            drain
                .events
                .iter()
                .any(|event| event.path.ends_with("usr/bin/demo"))
        );
        assert!(drain.events.iter().any(|event| {
            event.observed.path == "usr/bin/demo"
                && event.observed.operation != TraceOperation::Unknown
        }));
        assert!(!drain.event_loss);
    }

    #[test]
    fn recursive_inotify_adds_new_directory_watch_on_drain() {
        let temp = tempfile::tempdir().unwrap();
        let scope = scope(&temp);
        let install_dir = temp.path().join("install/usr/lib");
        let backend = InotifyTraceBackend::new();
        let mut session = backend.start(scope).unwrap();

        std::fs::create_dir_all(&install_dir).unwrap();
        let first = session.drain_events().unwrap();
        assert!(
            first
                .events
                .iter()
                .any(|event| event.observed.path == "usr")
        );

        let install_file = install_dir.join("demo.so");
        std::fs::write(&install_file, "library").unwrap();
        let second = session.finish().unwrap();
        assert!(
            second
                .events
                .iter()
                .any(|event| event.observed.path == "usr/lib/demo.so")
        );
    }

    #[test]
    fn probe_declares_incomplete_read_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let backend = InotifyTraceBackend::new();
        let status = backend
            .probe(&scope(&temp), RequestedRecordBackend::Inotify)
            .unwrap();

        assert_eq!(status.backend, SelectedBackend::Inotify);
        assert_eq!(
            status.limitations,
            vec![TraceLimitation::IncompleteReadEvidence]
        );
    }
}
