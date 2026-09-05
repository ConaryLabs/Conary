// crates/conary-core/src/repository/catalog/parity/resolution_producer/progress.rs

//! Live walk evidence, independent of artifact publication and worker completion.

use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const ROOT_INTERVAL: u64 = 5_000;
const TIME_INTERVAL: Duration = Duration::from_secs(60);

struct State {
    started: Instant,
    reported: Instant,
    roots_walked: u64,
    reported_roots: u64,
    total: u64,
    finished: bool,
}

impl State {
    fn due(&self, now: Instant) -> bool {
        self.roots_walked - self.reported_roots >= ROOT_INTERVAL
            || now.duration_since(self.reported) >= TIME_INTERVAL
    }

    fn report(&mut self, event: &'static str, now: Instant) {
        tracing::info!(
            event,
            roots_walked = self.roots_walked,
            total = self.total,
            elapsed_ms =
                u64::try_from(now.duration_since(self.started).as_millis()).unwrap_or(u64::MAX),
            "native resolution walk"
        );
        self.reported = now;
        self.reported_roots = self.roots_walked;
    }
}

pub(in crate::repository::catalog::parity) struct ResolutionProgress {
    state: Arc<(Mutex<State>, Condvar)>,
    heartbeat: Option<JoinHandle<()>>,
}

impl ResolutionProgress {
    pub(in crate::repository::catalog::parity) fn start(
        workers: usize,
        cpu_limit: usize,
        memory_budget_bytes: u64,
        total: u64,
    ) -> Self {
        tracing::info!(
            event = "start",
            workers,
            cpu_limit,
            memory_budget_bytes,
            root_count = total,
            "native resolution walk"
        );
        let now = Instant::now();
        let state = Arc::new((
            Mutex::new(State {
                started: now,
                reported: now,
                roots_walked: 0,
                reported_roots: 0,
                total,
                finished: false,
            }),
            Condvar::new(),
        ));
        let heartbeat_state = Arc::clone(&state);
        // Preserve the caller's tracing subscriber on the heartbeat thread too.
        let dispatch = tracing::dispatcher::get_default(Clone::clone);
        let heartbeat = std::thread::spawn(move || {
            tracing::dispatcher::with_default(&dispatch, || {
                let (lock, wake) = &*heartbeat_state;
                let mut state = lock.lock().unwrap_or_else(|error| error.into_inner());
                while !state.finished {
                    let timeout = TIME_INTERVAL.saturating_sub(state.reported.elapsed());
                    state = wake
                        .wait_timeout(state, timeout)
                        .unwrap_or_else(|error| error.into_inner())
                        .0;
                    let now = Instant::now();
                    if !state.finished && state.due(now) {
                        state.report("progress", now);
                    }
                }
            })
        });
        Self {
            state,
            heartbeat: Some(heartbeat),
        }
    }

    pub(in crate::repository::catalog::parity) fn root(&self) {
        let mut state = self
            .state
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.roots_walked += 1;
        let now = Instant::now();
        if state.due(now) {
            state.report("progress", now);
        }
    }
}

impl Drop for ResolutionProgress {
    fn drop(&mut self) {
        {
            let mut state = self
                .state
                .0
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.finished = true;
            state.report("finish", Instant::now());
            self.state.1.notify_one();
        }
        if let Some(heartbeat) = self.heartbeat.take() {
            let _ = heartbeat.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_is_due_at_either_boundary_and_resets_after_reporting() {
        let now = Instant::now();
        let mut state = State {
            started: now,
            reported: now,
            roots_walked: 4_999,
            reported_roots: 0,
            total: 20_000,
            finished: false,
        };
        assert!(!state.due(now + Duration::from_secs(59)));
        assert!(state.due(now + TIME_INTERVAL));
        state.roots_walked = 5_000;
        assert!(state.due(now));
        state.report("progress", now);
        assert!(!state.due(now));
        assert!(state.due(now + TIME_INTERVAL));
    }
}
