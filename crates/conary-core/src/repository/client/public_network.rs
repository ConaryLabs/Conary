// conary-core/src/repository/client/public_network.rs

//! Public-address enforcement and one-resolution HTTP client pinning.

use std::net::{IpAddr, SocketAddr};

use reqwest::Client;
use url::Host;

use super::TimeoutConfig;
use crate::error::{Error, Result};
use crate::repository::error_helpers::http_client_builder_error_message;

/// Validate that a URL uses an allowed scheme (HTTP or HTTPS only).
pub fn validate_url_scheme(url: &str) -> Result<()> {
    if url.starts_with("https://") || url.starts_with("http://") {
        Ok(())
    } else {
        Err(Error::ConfigError(format!(
            "URL must use http:// or https:// scheme: {url}"
        )))
    }
}

pub(crate) fn is_file_or_local_reference(url_or_path: &str) -> bool {
    url_or_path.starts_with("file://") || !has_url_scheme(url_or_path)
}

pub(super) fn has_url_scheme(input: &str) -> bool {
    let Some(colon_index) = input.find(':') else {
        return false;
    };

    let scheme = &input[..colon_index];
    let mut bytes = scheme.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };

    first.is_ascii_alphabetic()
        && bytes.all(|byte| {
            matches!(
                byte,
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'+' | b'-' | b'.'
            )
        })
}

pub fn require_public_repository_ip(ip: IpAddr) -> Result<()> {
    let forbidden = match ip {
        IpAddr::V4(ip) => {
            ip.is_loopback() || ip.is_private() || ip.is_link_local() || ip.is_unspecified()
        }
        IpAddr::V6(ip) => {
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return require_public_repository_ip(IpAddr::V4(mapped));
            }
            let first = ip.segments()[0];
            ip.is_loopback()
                || ip.is_unspecified()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
        }
    };
    if forbidden {
        return Err(Error::ConfigError(format!(
            "repository URL resolved to private or link-local address {ip}"
        )));
    }
    Ok(())
}

pub(super) async fn pinned_client_for_url(url: &str, timeouts: &TimeoutConfig) -> Result<Client> {
    let parsed = url::Url::parse(url)
        .map_err(|error| Error::ConfigError(format!("invalid repository URL: {error}")))?;
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| Error::ConfigError("repository URL has no port".to_string()))?;

    let mut builder = Client::builder()
        .connect_timeout(timeouts.connect)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy();
    match parsed
        .host()
        .ok_or_else(|| Error::ConfigError("repository URL has no host".to_string()))?
    {
        Host::Domain(host) => {
            let addrs = tokio::net::lookup_host((host, port))
                .await
                .map_err(|error| {
                    Error::DownloadError(format!("failed to resolve '{host}': {error}"))
                })?
                .collect::<Vec<SocketAddr>>();
            if addrs.is_empty() {
                return Err(Error::DownloadError(format!(
                    "DNS resolution for '{host}' returned no addresses"
                )));
            }
            for addr in &addrs {
                require_public_repository_ip(addr.ip())?;
            }
            builder = builder.resolve_to_addrs(host, &addrs);
        }
        Host::Ipv4(ip) => require_public_repository_ip(IpAddr::V4(ip))?,
        Host::Ipv6(ip) => require_public_repository_ip(IpAddr::V6(ip))?,
    }

    builder
        .build()
        .map_err(|error| Error::InitError(http_client_builder_error_message(error)))
}
