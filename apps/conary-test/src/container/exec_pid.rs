// apps/conary-test/src/container/exec_pid.rs

use std::pin::Pin;
use tokio::sync::futures::Notified;
use tokio::sync::{Mutex, Notify};

/// Publication state for a detached exec supervisor PID.
///
/// Waiters register and enable their notification before inspecting the value,
/// so `notify_waiters` cannot land in the gap between the inspection and wait.
#[derive(Default)]
pub(super) struct ExecPid {
    value: Mutex<Option<u64>>,
    ready: Notify,
}

pub(super) struct PreparedPidWait<'a> {
    pid: &'a ExecPid,
    observed: Option<u64>,
    ready: Pin<Box<Notified<'a>>>,
}

impl ExecPid {
    pub(super) async fn publish(&self, value: u64) {
        *self.value.lock().await = Some(value);
        self.ready.notify_waiters();
    }

    pub(super) async fn current(&self) -> Option<u64> {
        *self.value.lock().await
    }

    pub(super) async fn prepare_wait(&self) -> PreparedPidWait<'_> {
        let mut ready = Box::pin(self.ready.notified());
        ready.as_mut().enable();
        let observed = self.current().await;
        PreparedPidWait {
            pid: self,
            observed,
            ready,
        }
    }
}

impl PreparedPidWait<'_> {
    pub(super) async fn resolve(self) -> Option<u64> {
        if let Some(pid) = self.observed {
            return Some(pid);
        }
        self.ready.await;
        self.pid.current().await
    }
}

#[cfg(test)]
mod tests {
    use super::ExecPid;
    use std::time::Duration;

    #[tokio::test]
    async fn notification_published_after_check_before_await_is_observed() {
        let pid = ExecPid::default();
        let wait = pid.prepare_wait().await;
        assert!(wait.observed.is_none());

        pid.publish(4242).await;

        let observed = tokio::time::timeout(Duration::from_millis(50), wait.resolve())
            .await
            .expect("enabled PID notification must not be lost");
        assert_eq!(observed, Some(4242));
    }
}
