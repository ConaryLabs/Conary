// apps/conary/src/commands/publish/target.rs

//! Exact publication-destination parsing shared by the CLI and agent service.

use anyhow::{Result, bail};
use conary_core::repository::static_repo::RepoLocation;
use url::{ParseError, Url};

const REMI_RELEASE_ROUTE_PREFIX: [&str; 3] = ["v1", "admin", "releases"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PublishDestination {
    StaticLocal(RepoLocation),
    RemiRelease(RemiReleaseDestination),
    UnsupportedHttp(Url),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemiReleaseDestination {
    endpoint: Url,
    route_slug: String,
}

impl PublishDestination {
    pub(crate) fn parse(input: &str) -> Result<Self> {
        match Url::parse(input) {
            Ok(url) => match url.scheme() {
                "http" | "https" => parse_http_destination(url),
                "file" => {
                    reject_non_path_url_authority(&url)?;
                    Ok(Self::StaticLocal(RepoLocation::parse(input)?))
                }
                scheme => bail!(
                    "publish destination scheme '{scheme}' is unsupported; use http://, https://, file://, or a local path"
                ),
            },
            Err(ParseError::RelativeUrlWithoutBase) => {
                Ok(Self::StaticLocal(RepoLocation::parse(input)?))
            }
            Err(error) => bail!("invalid publish destination URL '{input}': {error}"),
        }
    }
}

impl RemiReleaseDestination {
    pub(crate) fn endpoint(&self) -> &str {
        self.endpoint.as_str()
    }

    pub(crate) fn route_slug(&self) -> &str {
        &self.route_slug
    }
}

fn parse_http_destination(url: Url) -> Result<PublishDestination> {
    reject_non_path_url_authority(&url)?;

    let segments = url
        .path_segments()
        .ok_or_else(|| anyhow::anyhow!("publish destination URL must have a hierarchical path"))?
        .collect::<Vec<_>>();
    if segments.len() == 4
        && segments[..3] == REMI_RELEASE_ROUTE_PREFIX
        && is_canonical_route_slug(segments[3])
    {
        return Ok(PublishDestination::RemiRelease(RemiReleaseDestination {
            route_slug: segments[3].to_string(),
            endpoint: url,
        }));
    }

    Ok(PublishDestination::UnsupportedHttp(url))
}

fn reject_non_path_url_authority(url: &Url) -> Result<()> {
    if !url.username().is_empty() || url.password().is_some() {
        bail!("publish destination URL must not contain user information");
    }
    if url.query().is_some() {
        bail!("publish destination URL must not contain a query string");
    }
    if url.fragment().is_some() {
        bail!("publish destination URL must not contain a fragment");
    }
    Ok(())
}

fn is_canonical_route_slug(segment: &str) -> bool {
    let mut bytes = segment.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && bytes
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_remi_release_route_is_typed() {
        let destination =
            PublishDestination::parse("https://remi.example.invalid/v1/admin/releases/fedora")
                .unwrap();

        let PublishDestination::RemiRelease(release) = destination else {
            panic!("exact Remi release route was not recognized");
        };
        assert_eq!(release.route_slug(), "fedora");
        assert_eq!(
            release.endpoint(),
            "https://remi.example.invalid/v1/admin/releases/fedora"
        );
    }

    #[test]
    fn http_scheme_is_parsed_case_insensitively() {
        let destination =
            PublishDestination::parse("HTTPS://remi.example.invalid/v1/admin/releases/arch")
                .unwrap();

        assert!(matches!(destination, PublishDestination::RemiRelease(_)));
    }

    #[test]
    fn unrelated_release_substrings_never_select_remi() {
        for target in [
            "https://example.invalid/prefix/v1/admin/releases/fedora",
            "https://example.invalid/v1/admin/releases/fedora/suffix",
            "https://example.invalid/v1/admin/releases/",
            "https://example.invalid/v1/admin/releases/fedora/",
            "https://example.invalid/v1/admin/releases/fedora%2Fextra",
            "https://example.invalid/static?next=/v1/admin/releases/fedora",
        ] {
            let destination = PublishDestination::parse(target);
            assert!(
                !matches!(destination, Ok(PublishDestination::RemiRelease(_))),
                "{target} selected the Remi mutation route"
            );
        }
    }

    #[test]
    fn remi_release_route_rejects_non_path_url_authority() {
        for target in [
            "https://user@example.invalid/v1/admin/releases/fedora",
            "https://example.invalid/v1/admin/releases/fedora?dry-run=true",
            "https://example.invalid/v1/admin/releases/fedora#fragment",
        ] {
            assert!(PublishDestination::parse(target).is_err(), "{target}");
        }
    }

    #[test]
    fn local_and_file_destinations_remain_static() {
        for target in ["./repo", "/srv/conary/repo", "file:///srv/conary/repo"] {
            assert!(matches!(
                PublishDestination::parse(target).unwrap(),
                PublishDestination::StaticLocal(_)
            ));
        }
    }

    #[test]
    fn malformed_and_unsupported_urls_fail_closed() {
        for target in [
            "https://[",
            "ssh://example.invalid/repo",
            "file:///srv/conary/repo?mode=publish",
            "file:///srv/conary/repo#fragment",
        ] {
            assert!(PublishDestination::parse(target).is_err(), "{target}");
        }
    }
}
