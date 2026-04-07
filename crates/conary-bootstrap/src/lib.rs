// crates/conary-bootstrap/src/lib.rs

use std::future::Future;

pub fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
}

pub fn finish(
    result: anyhow::Result<()>,
    reporter: impl FnOnce(&anyhow::Error),
    failure_code: i32,
) -> i32 {
    match result {
        Ok(()) => 0,
        Err(err) => {
            reporter(&err);
            failure_code
        }
    }
}

pub fn run_with_runtime<F, Fut>(entry: F) -> anyhow::Result<()>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = anyhow::Result<()>>,
{
    tokio::runtime::Runtime::new()?.block_on(entry())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_finish_returns_zero_on_success() {
        assert_eq!(
            finish(Ok(()), |_| panic!("reporter should not run"), 101),
            0
        );
    }

    #[test]
    fn test_finish_reports_and_returns_failure_code() {
        let mut seen = None;
        let code = finish(
            Err(anyhow::anyhow!("boom")),
            |err| seen = Some(err.to_string()),
            101,
        );
        assert_eq!(code, 101);
        assert_eq!(seen.as_deref(), Some("boom"));
    }

    #[test]
    fn test_run_with_runtime_returns_async_success() {
        let result = run_with_runtime(|| async { Ok(()) });
        assert!(result.is_ok());
    }

    #[test]
    fn test_run_with_runtime_propagates_async_error() {
        let err = run_with_runtime(|| async { Err(anyhow::anyhow!("runtime boom")) })
            .expect_err("runtime helper should propagate errors");
        assert_eq!(err.to_string(), "runtime boom");
    }
}
