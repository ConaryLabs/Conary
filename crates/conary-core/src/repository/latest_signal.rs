// conary-core/src/repository/latest_signal.rs

use crate::error::{Error, Result};
use chrono::{DateTime, Duration, Utc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LatestSignal {
    Positive { version: String },
    Fallback,
}

impl LatestSignal {
    pub fn from_repology(
        status: &str,
        version: Option<&str>,
        fetched_at: &str,
        now: DateTime<Utc>,
    ) -> Result<Self> {
        let fetched_at = DateTime::parse_from_rfc3339(fetched_at)
            .map_err(|err| {
                Error::ParseError(format!("invalid fetched_at '{}': {}", fetched_at, err))
            })?
            .with_timezone(&Utc);
        let is_recent = now.signed_duration_since(fetched_at) <= Duration::days(7);
        let has_version = version.is_some_and(|value| !value.trim().is_empty());

        if status == "newest" && has_version && is_recent {
            Ok(Self::Positive {
                version: version.unwrap().to_string(),
            })
        } else {
            Ok(Self::Fallback)
        }
    }

    pub fn is_positive(&self) -> bool {
        matches!(self, Self::Positive { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 7, 0, 0, 0).unwrap()
    }

    #[test]
    fn newest_recent_row_is_a_positive_latest_signal() {
        let signal =
            LatestSignal::from_repology("newest", Some("1.2.3"), "2026-04-07T00:00:00Z", now())
                .unwrap();
        assert!(signal.is_positive());
    }

    #[test]
    fn outdated_or_stale_rows_do_not_count_as_positive_signal() {
        assert!(
            !LatestSignal::from_repology("outdated", Some("1.2.3"), "2026-04-07T00:00:00Z", now())
                .unwrap()
                .is_positive()
        );
        assert!(
            !LatestSignal::from_repology("newest", Some("1.2.3"), "2026-03-01T00:00:00Z", now())
                .unwrap()
                .is_positive()
        );
    }
}
